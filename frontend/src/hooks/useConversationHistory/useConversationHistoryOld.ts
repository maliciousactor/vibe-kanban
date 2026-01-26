import {
  CommandExitStatus,
  ExecutionProcess,
  ExecutionProcessStatus,
  NormalizedEntry,
  PatchType,
  ToolStatus,
} from 'shared/types';
import { useExecutionProcessesContext } from '@/contexts/ExecutionProcessesContext';
import { useCallback, useEffect, useMemo, useRef } from 'react';
import { streamJsonPatchEntries } from '@/utils/streamJsonPatchEntries';
import type {
  AddEntryType,
  ExecutionProcessStateStore,
  OnEntriesUpdated,
  PatchTypeWithKey,
  UseConversationHistoryParams,
  UseConversationHistoryResult,
} from './types';
import {
  makeLoadingPatch,
  MIN_INITIAL_ENTRIES,
  nextActionPatch,
  REMAINING_BATCH_SIZE,
} from './constants';

export const useConversationHistoryOld = ({
  attempt,
  onEntriesUpdated,
}: UseConversationHistoryParams): UseConversationHistoryResult => {
  const { executionProcessesVisible: executionProcessesRaw } =
    useExecutionProcessesContext();
  const executionProcesses = useRef<ExecutionProcess[]>(executionProcessesRaw);
  const displayedExecutionProcesses = useRef<ExecutionProcessStateStore>({});
  const loadedInitialEntries = useRef(false);
  const streamingProcessIdsRef = useRef<Set<string>>(new Set());
  const onEntriesUpdatedRef = useRef<OnEntriesUpdated | null>(null);
  // Track WebSocket controllers for cleanup (React Strict Mode race condition fix)
  const wsControllersRef = useRef<Set<{ close: () => void }>>(new Set());
  // Track active async operation to prevent duplicates in React Strict Mode
  const activeAsyncOpRef = useRef<{ id: string; cancelled: boolean } | null>(null);

  const mergeIntoDisplayed = (
    mutator: (state: ExecutionProcessStateStore) => void
  ) => {
    const state = displayedExecutionProcesses.current;
    mutator(state);
  };
  useEffect(() => {
    onEntriesUpdatedRef.current = onEntriesUpdated;
  }, [onEntriesUpdated]);

  // Keep executionProcesses up to date
  useEffect(() => {
    executionProcesses.current = executionProcessesRaw.filter(
      (ep) =>
        ep.run_reason === 'setupscript' ||
        ep.run_reason === 'cleanupscript' ||
        ep.run_reason === 'codingagent'
    );
  }, [executionProcessesRaw]);

  const loadEntriesForHistoricExecutionProcess = (
    executionProcess: ExecutionProcess
  ) => {
    let url = '';
    if (executionProcess.executor_action.typ.type === 'ScriptRequest') {
      url = `/api/execution-processes/${executionProcess.id}/raw-logs/ws`;
    } else {
      url = `/api/execution-processes/${executionProcess.id}/normalized-logs/ws`;
    }

    const controllers = wsControllersRef.current;
    // Create a placeholder controller that will be populated after streamJsonPatchEntries returns
    // This ensures cleanup can close it even if it hasn't been fully initialized yet
    const placeholder: { close: () => void; onReady?: (closeFn: () => void) => void } = {
      close: () => {}, // Will be replaced with actual close
      onReady: undefined,
    };
    controllers.add(placeholder);

    return new Promise<PatchType[]>((resolve) => {
      // Add timeout to prevent hanging forever
      const timeoutId = setTimeout(() => {
        console.warn(`[loadEntriesForHistoricExecutionProcess] Timeout for process ${executionProcess.id}, resolving with current entries`);
        placeholder.close();
        controllers.delete(placeholder);
        // Return whatever entries we have so far
        resolve(controller.getEntries());
      }, 10000); // 10 second timeout
      
      const controller = streamJsonPatchEntries<PatchType>(url, {
        onControllerReady: (streamController) => {
          // Replace placeholder's close function with the actual controller's close
          placeholder.close = () => streamController.close();
          if (placeholder.onReady) {
            placeholder.onReady(() => streamController.close());
          }
        },
        onFinished: (allEntries) => {
          clearTimeout(timeoutId);
          controller.close();
          controllers.delete(placeholder);
          resolve(allEntries);
        },
        onError: (err) => {
          clearTimeout(timeoutId);
          console.warn(
            `Error loading entries for historic execution process ${executionProcess.id}`,
            err
          );
          controller.close();
          controllers.delete(placeholder);
          resolve([]);
        },
      });
      // Update placeholder with actual close function immediately
      placeholder.close = () => controller.close();
    });
  };

  const getLiveExecutionProcess = (
    executionProcessId: string
  ): ExecutionProcess | undefined => {
    return executionProcesses?.current.find(
      (executionProcess) => executionProcess.id === executionProcessId
    );
  };

  const patchWithKey = (
    patch: PatchType,
    executionProcessId: string,
    index: number | 'user'
  ) => {
    return {
      ...patch,
      patchKey: `${executionProcessId}:${index}`,
      executionProcessId,
    };
  };

  const flattenEntries = (
    executionProcessState: ExecutionProcessStateStore
  ): PatchTypeWithKey[] => {
    return Object.values(executionProcessState)
      .filter(
        (p) =>
          p.executionProcess.executor_action.typ.type ===
            'CodingAgentFollowUpRequest' ||
          p.executionProcess.executor_action.typ.type ===
            'CodingAgentInitialRequest' ||
          p.executionProcess.executor_action.typ.type === 'ReviewRequest'
      )
      .sort(
        (a, b) =>
          new Date(
            a.executionProcess.created_at as unknown as string
          ).getTime() -
          new Date(b.executionProcess.created_at as unknown as string).getTime()
      )
      .flatMap((p) => p.entries);
  };

  const getActiveAgentProcesses = (): ExecutionProcess[] => {
    return (
      executionProcesses?.current.filter(
        (p) =>
          p.status === ExecutionProcessStatus.running &&
          p.run_reason !== 'devserver'
      ) ?? []
    );
  };

  const flattenEntriesForEmit = useCallback(
    (executionProcessState: ExecutionProcessStateStore): PatchTypeWithKey[] => {
      // Flags to control Next Action bar emit
      let hasPendingApproval = false;
      let hasRunningProcess = false;
      let lastProcessFailedOrKilled = false;
      let needsSetup = false;
      let setupHelpText: string | undefined;

      // Create user messages + tool calls for setup/cleanup scripts
      const allEntries = Object.values(executionProcessState)
        .sort(
          (a, b) =>
            new Date(
              a.executionProcess.created_at as unknown as string
            ).getTime() -
            new Date(
              b.executionProcess.created_at as unknown as string
            ).getTime()
        )
        .flatMap((p, index) => {
          const entries: PatchTypeWithKey[] = [];
          if (
            p.executionProcess.executor_action.typ.type ===
              'CodingAgentInitialRequest' ||
            p.executionProcess.executor_action.typ.type ===
              'CodingAgentFollowUpRequest' ||
            p.executionProcess.executor_action.typ.type === 'ReviewRequest'
          ) {
            // New user message
            const actionType = p.executionProcess.executor_action.typ;
            const userNormalizedEntry: NormalizedEntry = {
              entry_type: {
                type: 'user_message',
              },
              content: actionType.prompt,
              timestamp: null,
            };
            const userPatch: PatchType = {
              type: 'NORMALIZED_ENTRY',
              content: userNormalizedEntry,
            };
            const userPatchTypeWithKey = patchWithKey(
              userPatch,
              p.executionProcess.id,
              'user'
            );
            entries.push(userPatchTypeWithKey);

            // Remove user messages (replaced with custom one) and token usage info (displayed separately)
            const filteredEntries = p.entries.filter(
              (e) =>
                e.type !== 'NORMALIZED_ENTRY' ||
                (e.content.entry_type.type !== 'user_message' &&
                  e.content.entry_type.type !== 'token_usage_info')
            );

            const hasPendingApprovalEntry = filteredEntries.some((entry) => {
              if (entry.type !== 'NORMALIZED_ENTRY') return false;
              const entryType = entry.content.entry_type;
              return (
                entryType.type === 'tool_use' &&
                entryType.status.status === 'pending_approval'
              );
            });

            if (hasPendingApprovalEntry) {
              hasPendingApproval = true;
            }

            entries.push(...filteredEntries);

            const liveProcessStatus = getLiveExecutionProcess(
              p.executionProcess.id
            )?.status;
            const isProcessRunning =
              liveProcessStatus === ExecutionProcessStatus.running;
            const processFailedOrKilled =
              liveProcessStatus === ExecutionProcessStatus.failed ||
              liveProcessStatus === ExecutionProcessStatus.killed;

            if (isProcessRunning) {
              hasRunningProcess = true;
            }

            if (
              processFailedOrKilled &&
              index === Object.keys(executionProcessState).length - 1
            ) {
              lastProcessFailedOrKilled = true;

              // Check if this failed process has a SetupRequired entry
              const hasSetupRequired = filteredEntries.some((entry) => {
                if (entry.type !== 'NORMALIZED_ENTRY') return false;
                if (
                  entry.content.entry_type.type === 'error_message' &&
                  entry.content.entry_type.error_type.type === 'setup_required'
                ) {
                  setupHelpText = entry.content.content;
                  return true;
                }
                return false;
              });

              if (hasSetupRequired) {
                needsSetup = true;
              }
            }

            if (isProcessRunning && !hasPendingApprovalEntry) {
              entries.push(makeLoadingPatch(p.executionProcess.id));
            }
          } else if (
            p.executionProcess.executor_action.typ.type === 'ScriptRequest'
          ) {
            // Add setup and cleanup script as a tool call
            let toolName = '';
            switch (p.executionProcess.executor_action.typ.context) {
              case 'SetupScript':
                toolName = 'Setup Script';
                break;
              case 'CleanupScript':
                toolName = 'Cleanup Script';
                break;
              case 'ToolInstallScript':
                toolName = 'Tool Install Script';
                break;
              default:
                return [];
            }

            const executionProcess = getLiveExecutionProcess(
              p.executionProcess.id
            );

            if (executionProcess?.status === ExecutionProcessStatus.running) {
              hasRunningProcess = true;
            }

            if (
              (executionProcess?.status === ExecutionProcessStatus.failed ||
                executionProcess?.status === ExecutionProcessStatus.killed) &&
              index === Object.keys(executionProcessState).length - 1
            ) {
              lastProcessFailedOrKilled = true;
            }

            const exitCode = Number(executionProcess?.exit_code) || 0;
            const exit_status: CommandExitStatus | null =
              executionProcess?.status === 'running'
                ? null
                : {
                    type: 'exit_code',
                    code: exitCode,
                  };

            const toolStatus: ToolStatus =
              executionProcess?.status === ExecutionProcessStatus.running
                ? { status: 'created' }
                : exitCode === 0
                  ? { status: 'success' }
                  : { status: 'failed' };

            const output = p.entries.map((line) => line.content).join('\n');

            const toolNormalizedEntry: NormalizedEntry = {
              entry_type: {
                type: 'tool_use',
                tool_name: toolName,
                action_type: {
                  action: 'command_run',
                  command: p.executionProcess.executor_action.typ.script,
                  result: {
                    output,
                    exit_status,
                  },
                },
                status: toolStatus,
              },
              content: toolName,
              timestamp: null,
            };
            const toolPatch: PatchType = {
              type: 'NORMALIZED_ENTRY',
              content: toolNormalizedEntry,
            };
            const toolPatchWithKey: PatchTypeWithKey = patchWithKey(
              toolPatch,
              p.executionProcess.id,
              0
            );

            entries.push(toolPatchWithKey);
          }

          return entries;
        });

      // Emit the next action bar if no process running
      if (!hasRunningProcess && !hasPendingApproval) {
        allEntries.push(
          nextActionPatch(
            lastProcessFailedOrKilled,
            Object.keys(executionProcessState).length,
            needsSetup,
            setupHelpText
          )
        );
      }

      return allEntries;
    },
    []
  );

  const emitEntries = useCallback(
    (
      executionProcessState: ExecutionProcessStateStore,
      addEntryType: AddEntryType,
      loading: boolean
    ) => {
      const entries = flattenEntriesForEmit(executionProcessState);
      let modifiedAddEntryType = addEntryType;

      // Modify so that if add entry type is 'running' and last entry is a plan, emit special plan type
      if (entries.length > 0) {
        const lastEntry = entries[entries.length - 1];
        if (
          lastEntry.type === 'NORMALIZED_ENTRY' &&
          lastEntry.content.entry_type.type === 'tool_use' &&
          lastEntry.content.entry_type.tool_name === 'ExitPlanMode'
        ) {
          modifiedAddEntryType = 'plan';
        }
      }

      onEntriesUpdatedRef.current?.(entries, modifiedAddEntryType, loading);
    },
    [flattenEntriesForEmit]
  );

  // This emits its own events as they are streamed
  const loadRunningAndEmit = useCallback(
    (executionProcess: ExecutionProcess): Promise<void> => {
      return new Promise((resolve, reject) => {
        let url = '';
        if (executionProcess.executor_action.typ.type === 'ScriptRequest') {
          url = `/api/execution-processes/${executionProcess.id}/raw-logs/ws`;
        } else {
          url = `/api/execution-processes/${executionProcess.id}/normalized-logs/ws`;
        }
        const controller = streamJsonPatchEntries<PatchType>(url, {
          onEntries(entries) {
            const patchesWithKey = entries.map((entry, index) =>
              patchWithKey(entry, executionProcess.id, index)
            );
            mergeIntoDisplayed((state) => {
              state[executionProcess.id] = {
                executionProcess,
                entries: patchesWithKey,
              };
            });
            emitEntries(displayedExecutionProcesses.current, 'running', false);
          },
          onFinished: () => {
            emitEntries(displayedExecutionProcesses.current, 'running', false);
            controller.close();
            resolve();
          },
          onError: () => {
            controller.close();
            reject();
          },
        });
      });
    },
    [emitEntries]
  );

  // Sometimes it can take a few seconds for the stream to start, wrap the loadRunningAndEmit method
  const loadRunningAndEmitWithBackoff = useCallback(
    async (executionProcess: ExecutionProcess) => {
      for (let i = 0; i < 20; i++) {
        try {
          await loadRunningAndEmit(executionProcess);
          break;
        } catch (_) {
          await new Promise((resolve) => setTimeout(resolve, 500));
        }
      }
    },
    [loadRunningAndEmit]
  );

  const loadInitialEntries =
    useCallback(async (): Promise<ExecutionProcessStateStore> => {
      const localDisplayedExecutionProcesses: ExecutionProcessStateStore = {};

      if (!executionProcesses?.current) return localDisplayedExecutionProcesses;

      for (const executionProcess of [
        ...executionProcesses.current,
      ].reverse()) {
        if (executionProcess.status === ExecutionProcessStatus.running)
          continue;

        const entries =
          await loadEntriesForHistoricExecutionProcess(executionProcess);
        const entriesWithKey = entries.map((e, idx) =>
          patchWithKey(e, executionProcess.id, idx)
        );

        localDisplayedExecutionProcesses[executionProcess.id] = {
          executionProcess,
          entries: entriesWithKey,
        };

        if (
          flattenEntries(localDisplayedExecutionProcesses).length >
          MIN_INITIAL_ENTRIES
        ) {
          break;
        }
      }

      return localDisplayedExecutionProcesses;
    }, [executionProcesses]);

  const loadRemainingEntriesInBatches = useCallback(
    async (batchSize: number): Promise<boolean> => {
      if (!executionProcesses?.current) return false;

      let anyUpdated = false;
      for (const executionProcess of [
        ...executionProcesses.current,
      ].reverse()) {
        const current = displayedExecutionProcesses.current;
        if (
          current[executionProcess.id] ||
          executionProcess.status === ExecutionProcessStatus.running
        )
          continue;

        const entries =
          await loadEntriesForHistoricExecutionProcess(executionProcess);
        const entriesWithKey = entries.map((e, idx) =>
          patchWithKey(e, executionProcess.id, idx)
        );

        mergeIntoDisplayed((state) => {
          state[executionProcess.id] = {
            executionProcess,
            entries: entriesWithKey,
          };
        });

        if (
          flattenEntries(displayedExecutionProcesses.current).length > batchSize
        ) {
          anyUpdated = true;
          break;
        }
        anyUpdated = true;
      }
      return anyUpdated;
    },
    [executionProcesses]
  );

  const ensureProcessVisible = useCallback((p: ExecutionProcess) => {
    mergeIntoDisplayed((state) => {
      if (!state[p.id]) {
        state[p.id] = {
          executionProcess: {
            id: p.id,
            created_at: p.created_at,
            updated_at: p.updated_at,
            executor_action: p.executor_action,
          },
          entries: [],
        };
      }
    });
  }, []);

  const idListKey = useMemo(
    () => executionProcessesRaw?.map((p) => p.id).join(','),
    [executionProcessesRaw]
  );

  const idStatusKey = useMemo(
    () => executionProcessesRaw?.map((p) => `${p.id}:${p.status}`).join(','),
    [executionProcessesRaw]
  );

  // Initial load when attempt changes
  useEffect(() => {
    console.log('[useConversationHistoryOld] Initial load effect running, attempt.id:', attempt?.id);
    console.log('[useConversationHistoryOld] executionProcesses count:', executionProcesses?.current.length);
    console.log('[useConversationHistoryOld] loadedInitialEntries:', loadedInitialEntries.current);
    
    // Track the active async operation to prevent duplicates
    const asyncOpId = Date.now() + '-' + Math.random();
    const activeAsyncOp = { id: asyncOpId, cancelled: false };
    
    // Store the active operation in a ref so we can cancel it
    const prevActiveOp = activeAsyncOpRef.current;
    activeAsyncOpRef.current = activeAsyncOp;
    
    (async () => {
      // Check if this operation has been superseded
      if (prevActiveOp && prevActiveOp !== activeAsyncOp) {
        prevActiveOp.cancelled = true;
      }
      
      // Wait for execution processes to load (WebSocket might not have connected yet)
      let waitCount = 0;
      const maxWait = 50; // Wait up to 5 seconds (50 * 100ms)
      while (executionProcesses?.current.length === 0 && waitCount < maxWait) {
        await new Promise((resolve) => setTimeout(resolve, 100));
        waitCount++;
      }
      
      console.log('[useConversationHistoryOld] After waiting, executionProcesses count:', executionProcesses?.current.length, 'waitCount:', waitCount);
      
      if (activeAsyncOp.cancelled) {
        console.log('[useConversationHistoryOld] Cancelled after waiting');
        return;
      }
      
      // Already loaded or still no processes after waiting
      if (loadedInitialEntries.current) {
        console.log('[useConversationHistoryOld] Early return - already loaded');
        return;
      }
      
      if (executionProcesses?.current.length === 0) {
        console.log('[useConversationHistoryOld] No execution processes after waiting - emitting empty state');
        emitEntries(displayedExecutionProcesses.current, 'initial', false);
        loadedInitialEntries.current = true;
        return;
      }

      console.log('[useConversationHistoryOld] Proceeding to load initial entries');

      // Initial entries
      console.log('[useConversationHistoryOld] Calling loadInitialEntries...');
      const allInitialEntries = await loadInitialEntries();
      console.log('[useConversationHistoryOld] loadInitialEntries returned, entries count:', Object.keys(allInitialEntries).length);
      if (activeAsyncOp.cancelled) {
        console.log('[useConversationHistoryOld] Cancelled after loadInitialEntries');
        return;
      }
      
      console.log('[useConversationHistoryOld] Merging entries into displayed...');
      mergeIntoDisplayed((state) => {
        Object.assign(state, allInitialEntries);
      });
      console.log('[useConversationHistoryOld] Emitting entries...');
      emitEntries(displayedExecutionProcesses.current, 'initial', false);
      loadedInitialEntries.current = true;
      console.log('[useConversationHistoryOld] Initial load complete');

      // Then load the remaining in batches
      while (
        !activeAsyncOp.cancelled &&
        (await loadRemainingEntriesInBatches(REMAINING_BATCH_SIZE))
      ) {
        if (activeAsyncOp.cancelled) return;
      }
      await new Promise((resolve) => setTimeout(resolve, 100));
      emitEntries(displayedExecutionProcesses.current, 'historic', false);
    })();
    return () => {
      // Don't close WebSockets in cleanup - let the async functions complete naturally
      // Closing WebSockets here would prevent them from receiving messages
      // The async functions will stop when the WebSocket receives a 'finished' message
      // or when the component unmounts and the WebSocket is garbage collected
    };
  }, [
    attempt.id,
    idListKey,
    loadInitialEntries,
    loadRemainingEntriesInBatches,
    emitEntries,
  ]); // include idListKey so new processes trigger reload

  useEffect(() => {
    const activeProcesses = getActiveAgentProcesses();
    if (activeProcesses.length === 0) return;

    for (const activeProcess of activeProcesses) {
      if (!displayedExecutionProcesses.current[activeProcess.id]) {
        const runningOrInitial =
          Object.keys(displayedExecutionProcesses.current).length > 1
            ? 'running'
            : 'initial';
        ensureProcessVisible(activeProcess);
        emitEntries(
          displayedExecutionProcesses.current,
          runningOrInitial,
          false
        );
      }

      if (
        activeProcess.status === ExecutionProcessStatus.running &&
        !streamingProcessIdsRef.current.has(activeProcess.id)
      ) {
        streamingProcessIdsRef.current.add(activeProcess.id);
        loadRunningAndEmitWithBackoff(activeProcess).finally(() => {
          streamingProcessIdsRef.current.delete(activeProcess.id);
        });
      }
    }
  }, [
    attempt.id,
    idStatusKey,
    emitEntries,
    ensureProcessVisible,
    loadRunningAndEmitWithBackoff,
  ]);

  // If an execution process is removed, remove it from the state
  useEffect(() => {
    if (!executionProcessesRaw) return;

    const removedProcessIds = Object.keys(
      displayedExecutionProcesses.current
    ).filter((id) => !executionProcessesRaw.some((p) => p.id === id));

    if (removedProcessIds.length > 0) {
      mergeIntoDisplayed((state) => {
        removedProcessIds.forEach((id) => {
          delete state[id];
        });
      });
    }
  }, [attempt.id, idListKey, executionProcessesRaw]);

  // Reset state when attempt changes
  useEffect(() => {
    displayedExecutionProcesses.current = {};
    loadedInitialEntries.current = false;
    streamingProcessIdsRef.current.clear();
    emitEntries(displayedExecutionProcesses.current, 'initial', true);
  }, [attempt.id, emitEntries]);

  return {};
};
