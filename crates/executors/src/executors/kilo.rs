use std::sync::Arc;

use async_trait::async_trait;
use command_group::AsyncCommandGroup;
use derivative::Derivative;
use futures::{FutureExt, StreamExt};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    process::Command,
    sync::Mutex,
};
use ts_rs::TS;
use workspace_utils::{
    log_msg::LogMsg,
    msg_store::MsgStore,
};

use crate::{
    approvals::ExecutorApprovalService,
    command::{CmdOverrides, CommandBuildError, CommandBuilder, CommandParts, apply_overrides},
    env::ExecutionEnv,
    executors::{
        AppendPrompt, AvailabilityInfo, ExecutorError,
        SpawnedChild, StandardCodingAgentExecutor,
    },
    logs::stderr_processor::normalize_stderr_logs,
    logs::utils::EntryIndexProvider,
    stdout_dup::create_stdout_pipe_writer,
};

#[derive(Derivative, Clone, Serialize, Deserialize, TS, JsonSchema)]
#[derivative(Debug, PartialEq)]
pub struct KiloCode {
    #[serde(default)]
    pub append_prompt: AppendPrompt,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub yolo: Option<bool>,
    #[serde(flatten)]
    pub cmd: CmdOverrides,
    #[serde(skip)]
    #[ts(skip)]
    #[derivative(Debug = "ignore", PartialEq = "ignore")]
    approvals: Option<Arc<dyn ExecutorApprovalService>>,
}

impl KiloCode {
    fn build_command_builder(&self) -> Result<CommandBuilder, CommandBuildError> {
        let mut builder = CommandBuilder::new("npx -y @kilocode/cli@latest");

        // Add JSON and auto flags for protocol output
        builder = builder.extend_params(["--json", "--auto"]);

        if let Some(mode) = &self.mode {
            builder = builder.extend_params(["--mode", mode.as_str()]);
        }

        if let Some(model) = &self.model {
            builder = builder.extend_params(["--model", model.as_str()]);
        }

        if self.yolo.unwrap_or(false) {
            builder = builder.extend_params(["--yolo"]);
        }

        apply_overrides(builder, &self.cmd)
    }
}

#[async_trait]
impl StandardCodingAgentExecutor for KiloCode {
    fn use_approvals(&mut self, approvals: Arc<dyn ExecutorApprovalService>) {
        self.approvals = Some(approvals);
    }

    async fn spawn(
        &self,
        current_dir: &std::path::Path,
        prompt: &str,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let command_builder = self.build_command_builder()?;
        let command_parts = command_builder.build_initial()?;
        self.spawn_internal(current_dir, prompt, command_parts, env)
            .await
    }

    async fn spawn_follow_up(
        &self,
        current_dir: &std::path::Path,
        prompt: &str,
        _session_id: &str,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let command_builder = self.build_command_builder()?;
        let command_parts = command_builder.build_follow_up(&["--continue".to_string()])?;
        self.spawn_internal(current_dir, prompt, command_parts, env)
            .await
    }

    fn normalize_logs(&self, msg_store: Arc<MsgStore>, worktree_path: &std::path::Path) {
        let entry_index_provider = EntryIndexProvider::start_from(&msg_store);

        // Process Kilo Code's JSON output from stdout
        KiloLogProcessor::process_logs(msg_store.clone(), worktree_path, entry_index_provider.clone());

        // Process stderr logs using standard stderr processor
        normalize_stderr_logs(msg_store, entry_index_provider);
    }

    fn default_mcp_config_path(&self) -> Option<std::path::PathBuf> {
        // Return standard MCP config path for Kilo Code
        dirs::home_dir().map(|home| home.join(".kilocode").join("mcp_config.json"))
    }

    fn get_availability_info(&self) -> AvailabilityInfo {
        // Check for ~/.kilocode/installation_id or ~/.kilocode/config.json
        let config_path = self.default_mcp_config_path();
        let mcp_config_found = config_path.as_ref().map(|p| p.exists()).unwrap_or(false);

        let installation_indicator_found = dirs::home_dir()
            .map(|home| home.join(".kilocode").join("installation_id").exists())
            .unwrap_or(false);

        let config_json_found = dirs::home_dir()
            .map(|home| home.join(".kilocode").join("config.json").exists())
            .unwrap_or(false);

        if mcp_config_found || installation_indicator_found || config_json_found {
            AvailabilityInfo::InstallationFound
        } else {
            AvailabilityInfo::NotFound
        }
    }
}

impl KiloCode {
    /// Format Kilo Code JSON message for display
    fn format_kilo_message(json_value: &serde_json::Value) -> String {
        // Extract message type and content from Kilo's JSON protocol
        // The exact format depends on Kilo's output structure
        if let Some(msg_type) = json_value.get("type").and_then(|v| v.as_str()) {
            match msg_type {
                "assistant_message" | "message" => {
                    if let Some(content) = json_value.get("content").and_then(|v| v.as_str()) {
                        return content.to_string();
                    }
                }
                "tool_call" => {
                    if let Some(tool_name) = json_value.get("name").and_then(|v| v.as_str()) {
                        return format!("[Tool: {tool_name}]");
                    }
                }
                "tool_result" => {
                    if let Some(tool_name) = json_value.get("name").and_then(|v| v.as_str()) {
                        return format!("[Tool Result: {tool_name}]");
                    }
                }
                "error" => {
                    if let Some(error_msg) = json_value.get("message").and_then(|v| v.as_str()) {
                        return format!("[Error] {error_msg}");
                    }
                }
                _ => {}
            }
        }

        // Fallback: serialize the whole JSON as a system message
        serde_json::to_string_pretty(json_value).unwrap_or_else(|_| "[Unknown Kilo message]".to_string())
    }

    async fn spawn_internal(
        &self,
        current_dir: &std::path::Path,
        prompt: &str,
        command_parts: CommandParts,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let (program_path, args) = command_parts.into_resolved().await?;
        let program_path_display = program_path.display().to_string();
        let combined_prompt = self.append_prompt.combine_prompt(prompt);

        let mut command = Command::new(&program_path);
        command
            .kill_on_drop(true)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .current_dir(current_dir)
            .env("NPM_CONFIG_LOGLEVEL", "error")
            .env("NODE_NO_WARNINGS", "1")
            .env("NO_COLOR", "1")
            .args(&args);

        env.clone()
            .with_profile(&self.cmd)
            .apply_to_command(&mut command);

        let mut child = command.group_spawn()?;
        let child_stdout = child
            .inner()
            .stdout
            .take()
            .ok_or_else(|| ExecutorError::Io(std::io::Error::other("Kilo Code missing stdout")))?;
        let child_stdin = child
            .inner()
            .stdin
            .take()
            .ok_or_else(|| ExecutorError::Io(std::io::Error::other("Kilo Code missing stdin")))?;

        let new_stdout = create_stdout_pipe_writer(&mut child)?;

        tracing::debug!(program = %program_path_display, args = ?args, "Starting Kilo Code executor");

        // Create interrupt channel for graceful shutdown
        let (interrupt_tx, interrupt_rx) = tokio::sync::oneshot::channel::<()>();
        // Spawn task to handle Kilo's JSON output and forward logs
        let prompt_clone = combined_prompt.clone();
        tokio::spawn(async move {
            let log_writer = LogWriter::new(new_stdout);
            tracing::debug!("Kilo executor: LogWriter created, sending prompt");
            let mut stdin = child_stdin;

            // Send the prompt to stdin immediately
            if let Err(e) = stdin.write_all(prompt_clone.as_bytes()).await {
                let _ = log_writer
                    .log_raw(&format!("Error: Failed to write prompt - {e}"))
                    .await;
                return;
            }
            if let Err(e) = stdin.write_all(b"\n").await {
                let _ = log_writer
                    .log_raw(&format!("Error: Failed to write newline - {e}"))
                    .await;
                return;
            }
            if let Err(e) = stdin.flush().await {
                let _ = log_writer
                    .log_raw(&format!("Error: Failed to flush stdin - {e}"))
                    .await;
                return;
            }
            drop(stdin); // Close stdin to signal EOF - required for Kilo CLI to produce output

            // Process Kilo's JSON output from stdout
            let mut stdout_reader = BufReader::new(child_stdout);
            let mut line = String::new();

            // FUSE THE RECEIVER - Critical fix!
            let mut interrupt_rx = interrupt_rx.fuse();

            loop {
                tokio::select! {
                    _ = &mut interrupt_rx => {
                        tracing::debug!("Kilo executor interrupted, stopping log forwarding");
                        break;
                    }
                    result = stdout_reader.read_line(&mut line) => {
                        match result {
                            Ok(0) => {
                                // EOF - child process ended
                                tracing::debug!("Kilo Code stdout closed");
                                break;
                            }
                            Ok(_) => {
                                let trimmed = line.trim();
                                tracing::debug!(output = trimmed, "Kilo executor received stdout");
                                if !trimmed.is_empty() {
                                    // Try to parse as JSON first
                                    match serde_json::from_str::<serde_json::Value>(trimmed) {
                                        Ok(json_value) => {
                                            // Parse Kilo's JSON protocol and format for display
                                            let formatted = Self::format_kilo_message(&json_value);
                                            if let Err(e) = log_writer.log_raw(&formatted).await {
                                                tracing::error!("Failed to write log: {e}");
                                                break;
                                            }
                                        }
                                        Err(_) => {
                                            // Non-JSON output - forward as-is
                                            if let Err(e) = log_writer.log_raw(trimmed).await {
                                                tracing::error!("Failed to write raw output: {e}");
                                                break;
                                            }
                                        }
                                    }
                                }
                                line.clear();
                            }
                            Err(e) => {
                                tracing::error!("Error reading from Kilo stdout: {e}");
                                let _ = log_writer.log_raw(&format!("Error reading output: {e}")).await;
                                break;
                            }
                        }
                    }
                }
            }
        });

        Ok(SpawnedChild {
            child,
            exit_signal: None,
            interrupt_sender: Some(interrupt_tx),
        })
    }
}

/// Log writer for Kilo Code executor
#[derive(Clone)]
struct LogWriter {
    writer: Arc<Mutex<tokio::io::BufWriter<Box<dyn AsyncWrite + Send + Unpin>>>>,
}

impl LogWriter {
    pub fn new(writer: impl AsyncWrite + Send + Unpin + 'static) -> Self {
        Self {
            writer: Arc::new(Mutex::new(tokio::io::BufWriter::new(Box::new(writer)))),
        }
    }

    pub async fn log_raw(&self, raw: &str) -> Result<(), ExecutorError> {
        let mut guard = self.writer.lock().await;
        guard
            .write_all(raw.as_bytes())
            .await
            .map_err(ExecutorError::Io)?;
        guard.write_all(b"\n").await.map_err(ExecutorError::Io)?;
        guard.flush().await.map_err(ExecutorError::Io)?;
        Ok(())
    }
}

/// Handles log processing and interpretation for Kilo Code executor
struct KiloLogProcessor;

impl KiloLogProcessor {
    /// Process raw logs and convert them to normalized entries with patches
    pub fn process_logs(
        msg_store: Arc<MsgStore>,
        _worktree_path: &std::path::Path,
        entry_index_provider: EntryIndexProvider,
    ) {
        tokio::spawn(async move {
            let mut stream = msg_store.history_plus_stream();
            let mut buffer = String::new();

            while let Some(Ok(msg)) = stream.next().await {
                let chunk = match msg {
                    LogMsg::Stdout(x) => x,
                    LogMsg::JsonPatch(_)
                    | LogMsg::SessionId(_)
                    | LogMsg::Stderr(_)
                    | LogMsg::Ready
                    | LogMsg::Finished => continue,
                };

                buffer.push_str(&chunk);

                // Process complete JSON lines
                for line in buffer
                    .split_inclusive('\n')
                    .filter(|l| l.ends_with('\n'))
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
                {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    // Try to parse as JSON first
                    match serde_json::from_str::<serde_json::Value>(trimmed) {
                        Ok(json_value) => {
                            // Parse Kilo Code's JSON protocol
                            let patches =
                                Self::normalize_entries(&json_value, &entry_index_provider);
                            for patch in patches {
                                msg_store.push_patch(patch);
                            }
                        }
                        Err(_) => {
                            // Handle non-JSON output as raw system message
                            // (this shouldn't normally happen with --json flag)
                        }
                    }
                }
            }
        });
    }

    /// Normalize JSON entries from Kilo Code
    fn normalize_entries(
        json_value: &serde_json::Value,
        entry_index_provider: &EntryIndexProvider,
    ) -> Vec<json_patch::Patch> {
        let mut patches = Vec::new();

        // Kilo Code's JSON protocol format is not yet fully documented
        // For now, we'll pass through raw JSON messages as system messages
        // This can be enhanced once protocol is better understood

        let entry = crate::logs::NormalizedEntry {
            timestamp: None,
            entry_type: crate::logs::NormalizedEntryType::SystemMessage,
            content: serde_json::to_string(json_value).unwrap_or_else(|_| "Unknown".to_string()),
            metadata: None,
        };

        let patch_id = entry_index_provider.next();
        let patch = crate::logs::utils::patch::ConversationPatch::add_normalized_entry(
            patch_id, entry,
        );
        patches.push(patch);

        patches
    }
}
