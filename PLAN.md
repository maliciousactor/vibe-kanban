# Kilo Code CLI Integration Plan

## 1. Executive Summary

This document outlines the comprehensive integration plan for adding **Kilo Code CLI** (`@kilocode/cli`) as a new coding agent executor in the vibe-kanban project. The integration will follow the existing agent architecture patterns established by Claude Code, Gemini, and other supported agents.

Kilo Code CLI is a terminal-based AI coding assistant that supports multiple modes (Architect, Ask, Debug, Code, Orchestrator), model switching, and interactive workflows. The integration will leverage the existing ACP (Agent Client Protocol) harness that already supports Gemini and QwenCode executors, minimizing implementation complexity.

**Key Integration Points:**
- Use existing `AcpAgentHarness` for spawning and communication
- Leverage `normalize_logs` for log processing
- Add KILO_CODE to the `CodingAgent` enum
- Add default profiles and configuration
- Create frontend icon assets
- Generate TypeScript types

---

## 2. Kilo Code CLI Overview

### 2.1 Installation

```bash
npm install -g @kilocode/cli
```

### 2.2 Basic Commands

| Command | Description |
|---------|-------------|
| `kilocode` | Start interactive chat session |
| `kilocode --mode architect` | Start with specific mode |
| `kilocode --workspace /path` | Start with specific workspace |
| `kilocode --continue` | Resume last conversation |

### 2.3 Available Modes

- **Architect** - Planning and architecture design
- **Code** - General coding tasks
- **Ask** - Question-answering mode
- **Debug** - Troubleshooting and debugging
- **Orchestrator** - Complex multi-step tasks
- **Custom modes** - User-defined modes

### 2.4 Key Features

- Multiple LLM model support (switch between providers freely)
- Agent Skills (extendable capabilities via `~/.kilocode/skills/`)
- Custom Commands (`~/.kilocode/commands/`)
- Checkpoint management for state recovery
- Task history and search
- Parallel mode for concurrent work
- Auto-approval settings configuration

---

## 3. Architecture Analysis

### 3.1 Existing Agent Architecture

The vibe-kanban project uses a plugin-based executor architecture:

```
crates/executors/src/executors/
├── mod.rs           # CodingAgent enum + StandardCodingAgentExecutor trait
├── claude.rs        # Claude Code executor (custom implementation)
├── gemini.rs        # Gemini executor (uses AcpAgentHarness)
├── qwen.rs          # QwenCode executor (uses AcpAgentHarness)
├── codex/           # Codex executor (custom implementation)
├── copilot/         # Copilot executor
├── cursor/          # Cursor agent
├── opencode/        # OpenCode executor
├── droid/           # Droid executor
└── acp/             # ACP protocol utilities (harness, normalize_logs)
```

### 3.2 Why Use AcpAgentHarness

Kilo Code CLI supports the **Agent Communication Protocol (ACP)** via the `--experimental-acp` flag. This enables:
- Structured tool call handling
- Session management and forking
- Log normalization
- Approval flow integration

The `AcpAgentHarness` (`crates/executors/src/executors/acp/harness.rs`) already provides:
- Process spawning and stdio management
- ACP client initialization
- Session creation/forking
- Tool call handling
- Exit signal management

### 3.3 Integration Strategy

Following the **Gemini** and **QwenCode** pattern:

1. Create `kilo.rs` in `crates/executors/src/executors/`
2. Define `KiloCode` struct with configuration options
3. Implement `StandardCodingAgentExecutor` trait
4. Use `AcpAgentHarness` for spawning and session management
5. Use `acp::normalize_logs` for log processing

---

## 4. Implementation Steps

### 4.1 Backend Changes (Rust)

#### 4.1.1 Create KiloCode Executor Struct

**File:** `crates/executors/src/executors/kilo.rs`

```rust
use std::{path::Path, sync::Arc};

use async_trait::async_trait;
use derivative::Derivative;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use workspace_utils::msg_store::MsgStore;

pub use super::acp::AcpAgentHarness;
use crate::{
    approvals::ExecutorApprovalService,
    command::{CmdOverrides, CommandBuildError, CommandBuilder, apply_overrides},
    env::ExecutionEnv,
    executors::{
        AppendPrompt, AvailabilityInfo, ExecutorError, SpawnedChild, StandardCodingAgentExecutor,
    },
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
    pub approvals: Option<Arc<dyn ExecutorApprovalService>>,
}

impl KiloCode {
    fn build_command_builder(&self) -> Result<CommandBuilder, CommandBuildError> {
        let mut builder = CommandBuilder::new("npx -y @kilocode/cli@latest");

        if let Some(mode) = &self.mode {
            builder = builder.extend_params(["--mode", mode.as_str()]);
        }

        if let Some(model) = &self.model {
            builder = builder.extend_params(["--model", model.as_str()]);
        }

        if self.yolo.unwrap_or(false) {
            builder = builder.extend_params(["--yolo"]);
        }

        builder = builder.extend_params(["--experimental-acp"]);

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
        current_dir: &Path,
        prompt: &str,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let harness = AcpAgentHarness::new();
        let combined_prompt = self.append_prompt.combine_prompt(prompt);
        let kilo_command = self.build_command_builder()?.build_initial()?;
        let approvals = if self.yolo.unwrap_or(false) {
            None
        } else {
            self.approvals.clone()
        };
        harness
            .spawn_with_command(
                current_dir,
                combined_prompt,
                kilo_command,
                env,
                &self.cmd,
                approvals,
            )
            .await
    }

    async fn spawn_follow_up(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: &str,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let harness = AcpAgentHarness::new();
        let combined_prompt = self.append_prompt.combine_prompt(prompt);
        let kilo_command = self.build_command_builder()?.build_follow_up(&[])?;
        let approvals = if self.yolo.unwrap_or(false) {
            None
        } else {
            self.approvals.clone()
        };
        harness
            .spawn_follow_up_with_command(
                current_dir,
                combined_prompt,
                session_id,
                kilo_command,
                env,
                &self.cmd,
                approvals,
            )
            .await
    }

    fn normalize_logs(&self, msg_store: Arc<MsgStore>, worktree_path: &Path) {
        super::acp::normalize_logs(msg_store, worktree_path);
    }

    fn default_mcp_config_path(&self) -> Option<std::path::PathBuf> {
        dirs::home_dir().map(|home| home.join(".kilocode").join("config.json"))
    }

    fn get_availability_info(&self) -> AvailabilityInfo {
        // Check for installation indicator
        let installation_indicator = dirs::home_dir()
            .map(|home| home.join(".kilocode").join("installation_id"))
            .unwrap_or_default();

        if installation_indicator.exists() {
            if let Ok(metadata) = std::fs::metadata(&installation_indicator) {
                if let Ok(modified) = metadata.modified() {
                    if let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH) {
                        return AvailabilityInfo::LoginDetected {
                            last_auth_timestamp: duration.as_secs() as i64,
                        };
                    }
                }
            }
            return AvailabilityInfo::InstallationFound;
        }

        AvailabilityInfo::NotFound
    }
}
```

#### 4.1.2 Update mod.rs to Register KiloCode

**File:** `crates/executors/src/executors/mod.rs`

1. Add module declaration (around line 42):
```rust
pub mod kilo;
```

2. Add to imports (around line 24-26):
```rust
use crate::executors::{
    amp::Amp, claude::ClaudeCode, codex::Codex, copilot::Copilot, cursor::CursorAgent,
    droid::Droid, gemini::Gemini, kilo::KiloCode, opencode::Opencode, qwen::QwenCode,
},
```

3. Add to `CodingAgent` enum (around line 105):
```rust
#[enum_dispatch]
#[derive(
    Debug, Clone, Serialize, Deserialize, PartialEq, TS, Display, EnumDiscriminants, VariantNames,
)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[strum(serialize_all = "SCREAMING_SNAKE_CASE")]
#[strum_discriminants(
    name(BaseCodingAgent),
    derive(EnumString, Hash, strum_macros::Display, Serialize, Deserialize, TS, Type),
    strum(serialize_all = "SCREAMING_SNAKE_CASE"),
    ts(use_ts_enum),
    serde(rename_all = "SCREAMING_SNAKE_CASE"),
    sqlx(type_name = "TEXT", rename_all = "SCREAMING_SNAKE_CASE")
)]
pub enum CodingAgent {
    ClaudeCode,
    Amp,
    Gemini,
    Codex,
    Opencode,
    #[serde(alias = "CURSOR")]
    #[strum_discriminants(serde(alias = "CURSOR"))]
    #[strum_discriminants(strum(serialize = "CURSOR", serialize = "CURSOR_AGENT"))]
    CursorAgent,
    QwenCode,
    Copilot,
    Droid,
    KiloCode,  // <-- ADD THIS LINE
    #[cfg(feature = "qa-mode")]
    QaMock(QaMockExecutor),
}
```

4. Add MCP config handler in `CodingAgent::get_mcp_config()` (around line 158):
```rust
_ => McpConfig::new(
    vec!["mcpServers".to_string()],
    serde_json::json!({
        "mcpServers": {}
    }),
    self.preconfigured_mcp(),
    false,
),
```

5. Add to capabilities (around line 173):
```rust
pub fn capabilities(&self) -> Vec<BaseAgentCapability> {
    match self {
        Self::ClaudeCode(_)
        | Self::Amp(_)
        | Self::Gemini(_)
        | Self::QwenCode(_)
        | Self::KiloCode(_)  // <-- ADD THIS
        | Self::Droid(_)
        | Self::Opencode(_) => vec![BaseAgentCapability::SessionFork],
        // ... rest unchanged
    }
}
```

### 4.2 Configuration Changes

#### 4.2.1 Update default_profiles.json

**File:** `crates/executors/default_profiles.json`

Add the KILO_CODE section:

```json
{
  "executors": {
    // ... existing executors ...
    
    "KILO_CODE": {
      "DEFAULT": {
        "KILO_CODE": {
          "yolo": true
        }
      },
      "CODE": {
        "KILO_CODE": {
          "mode": "code",
          "yolo": true
        }
      },
      "ARCHITECT": {
        "KILO_CODE": {
          "mode": "architect",
          "yolo": true
        }
      },
      "DEBUG": {
        "KILO_CODE": {
          "mode": "debug",
          "yolo": true
        }
      },
      "ORCHESTRATOR": {
        "KILO_CODE": {
          "mode": "orchestrator",
          "yolo": true
        }
      },
      "APPROVALS": {
        "KILO_CODE": {
          "mode": "code",
          "yolo": false
        }
      }
    }
  }
}
```

### 4.3 Type Generation

#### 4.3.1 Update generate_types.rs

**File:** `crates/server/src/bin/generate_types.rs`

1. Add KiloCode declaration (around line 192):
```rust
executors::executors::qwen::QwenCode::decl(),
executors::executors::kilo::KiloCode::decl(),  // <-- ADD THIS
executors::executors::droid::Droid::decl(),
```

2. Add to JSON schemas (around line 297):
```rust
(
    "droid",
    generate_json_schema::<executors::executors::droid::Droid>()?,
),
(
    "kilo_code",  // <-- ADD THIS
    generate_json_schema::<executors::executors::kilo::KiloCode>()?,
),
```

### 4.4 Frontend Changes

#### 4.4.1 Create Agent Icon SVGs

**Files:** 
- `frontend/public/agents/kilo-dark.svg`
- `frontend/public/agents/kilo-light.svg`

Create SVG icons following the pattern of existing agents (e.g., `claude-dark.svg`, `claude-light.svg`).

**kilo-dark.svg example:**
```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#10b981" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
  <!-- Kilo Code logo design -->
  <circle cx="12" cy="12" r="10" />
  <path d="M8 12l3 3 5-5" />
</svg>
```

#### 4.4.2 Update AgentIcon.tsx

**File:** `frontend/src/components/agents/AgentIcon.tsx`

Add KILO_CODE case:

```tsx
export function getAgentName(
  agent: BaseCodingAgent | null | undefined
): string {
  if (!agent) return 'Agent';
  switch (agent) {
    case BaseCodingAgent.CLAUDE_CODE:
      return 'Claude Code';
    case BaseCodingAgent.AMP:
      return 'AMP';
    case BaseCodingAgent.GEMINI:
      return 'Gemini';
    case BaseCodingAgent.CODEX:
      return 'Codex';
    case BaseCodingAgent.OPENCODE:
      return 'OpenCode';
    case BaseCodingAgent.CURSOR_AGENT:
      return 'Cursor';
    case BaseCodingAgent.QWEN_CODE:
      return 'Qwen';
    case BaseCodingAgent.COPILOT:
      return 'Copilot';
    case BaseCodingAgent.DROID:
      return 'Droid';
    case BaseCodingAgent.KILO_CODE:  // <-- ADD THIS
      return 'Kilo Code';
  }
}

// ... in the AgentIcon component switch statement:
switch (agent) {
    case BaseCodingAgent.CLAUDE_CODE:
      iconPath = `/agents/claude${suffix}.svg`;
      break;
    // ... existing cases ...
    case BaseCodingAgent.DROID:
      iconPath = `/agents/droid${suffix}.svg`;
      break;
    case BaseCodingAgent.KILO_CODE:  // <-- ADD THIS
      iconPath = `/agents/kilo${suffix}.svg`;
      break;
    default:
      return null;
  }
```

---

## 5. Command Pattern

### 5.1 Base Command

```bash
npx -y @kilocode/cli@latest --experimental-acp
```

### 5.2 With Mode

```bash
npx -y @kilocode/cli@latest --mode code --experimental-acp
```

### 5.3 With Model

```bash
npx -y @kilocode/cli@latest --model anthropic/claude-sonnet-4 --experimental-acp
```

### 5.4 Full Command with Options

```bash
npx -y @kilocode/cli@latest \
  --mode code \
  --model anthropic/claude-sonnet-4 \
  --yolo \
  --experimental-acp
```

### 5.5 Interactive Mode (Default)

Without any flags, Kilo Code CLI runs in interactive mode. The `--experimental-acp` flag enables structured communication.

---

## 6. Log Normalization Strategy

### 6.1 Using ACP Log Processor

Kilo Code CLI outputs logs in ACP format when `--experimental-acp` is used. The existing `acp::normalize_logs` function handles:

1. **Session Events** - Session start/ID tracking
2. **Message Events** - Assistant messages (streaming text)
3. **Thought Events** - Thinking/reasoning content
4. **Tool Call Events** - File reads, edits, command execution
5. **Plan Events** - Todo list management
6. **Permission Requests** - Tool approval workflows
7. **Error Events** - Error messages

### 6.2 Log Entry Mapping

| ACP Event | NormalizedEntry Type |
|-----------|---------------------|
| SessionStart | Session tracking |
| Message (Text) | AssistantMessage |
| Thought | Thinking |
| ToolCall (read) | FileRead |
| ToolCall (edit) | FileEdit |
| ToolCall (execute) | CommandRun |
| ToolCall (search) | Search |
| ToolCall (fetch) | WebFetch |
| Plan | TodoManagement |
| Error | ErrorMessage |

### 6.3 Stderr Processing

Standard stderr logs are processed using `stderr_processor::normalize_stderr_logs`, which handles:
- Warning messages
- Progress indicators
- Debug output
- Error traces

---

## 7. Availability Detection

### 7.1 Detection Strategy

Kilo Code CLI availability is detected by checking for installation indicators:

1. **Primary**: Check for `~/.kilocode/installation_id` file
2. **Fallback**: Check for `~/.kilocode/config.json` configuration

### 7.2 Availability States

| State | Condition |
|-------|-----------|
| `LoginDetected` | Installation ID file exists with valid modification timestamp |
| `InstallationFound` | Config file exists but no login timestamp |
| `NotFound` | No installation indicators found |

### 7.3 Implementation

```rust
fn get_availability_info(&self) -> AvailabilityInfo {
    // Check for installation indicator
    let installation_indicator = dirs::home_dir()
        .map(|home| home.join(".kilocode").join("installation_id"))
        .unwrap_or_default();

    if installation_indicator.exists() {
        if let Ok(metadata) = std::fs::metadata(&installation_indicator) {
            if let Ok(modified) = metadata.modified() {
                if let Ok(duration) = modified.duration_since(std::time::UNIX_EPOCH) {
                    return AvailabilityInfo::LoginDetected {
                        last_auth_timestamp: duration.as_secs() as i64,
                    };
                }
            }
        }
        return AvailabilityInfo::InstallationFound;
    }

    AvailabilityInfo::NotFound
}
```

---

## 8. Testing Strategy

### 8.1 Backend Testing

#### 8.1.1 Unit Tests

Add tests in `crates/executors/src/executors/kilo.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::executors::StandardCodingAgentExecutor;

    #[tokio::test]
    async fn test_build_command_builder() {
        let kilo = KiloCode {
            mode: Some("code".to_string()),
            model: Some("claude-sonnet-4".to_string()),
            yolo: Some(true),
            ..Default::default()
        };
        
        let builder = kilo.build_command_builder().unwrap();
        // Verify command structure
    }

    #[test]
    fn test_availability_detection() {
        let kilo = KiloCode::default();
        let info = kilo.get_availability_info();
        // Test based on actual installation state
    }
}
```

#### 8.1.2 Integration Tests

Test full spawn flow:
```bash
cargo test --package executors --lib executors::kilo::tests
```

### 8.2 Frontend Testing

#### 8.2.1 Component Tests

Verify agent icon rendering:
```bash
cd frontend && pnpm run test -- --run AgentIcon
```

#### 8.2.2 Type Checks

```bash
pnpm run check
```

### 8.3 E2E Testing

1. Install Kilo Code CLI: `npm install -g @kilocode/cli`
2. Run dev environment: `pnpm run dev:qa`
3. Test agent selection and spawning in the UI

### 8.4 Test Cases

| Test | Description |
|------|-------------|
| Spawn | Agent spawns with prompt |
| Follow-up | Session continuation works |
| Log Normalization | Output is properly formatted |
| Availability | Detection returns correct state |
| Mode Switching | Different modes work correctly |
| MCP Integration | MCP servers can be configured |

---

## 9. Potential Challenges

### 9.1 ACP Protocol Compatibility

**Challenge**: Kilo Code CLI's ACP implementation may differ slightly from Gemini/Qwen.

**Mitigation**: 
- Test thoroughly with `--experimental-acp` flag
- Adjust `AcpEvent` parsing if needed
- Use existing error handling for protocol mismatches

### 9.2 Session Management

**Challenge**: Kilo Code CLI session format may differ from ACP standard.

**Mitigation**:
- Verify session ID extraction in `normalize_logs`
- Test session fork/resume functionality
- Fall back to prompt-based continuation if needed

### 9.3 Tool Call Mapping

**Challenge**: Kilo Code CLI tool names may not match expected types.

**Mitigation**:
- Review `map_to_action_type` function for edge cases
- Add logging for unrecognized tool calls
- Use "Other" action type for unknown tools

### 9.4 Performance

**Challenge**: ACP overhead may slow down communication.

**Mitigation**:
- Use existing benchmark tests
- Profile with large prompts
- Optimize log processing if needed

### 9.5 Installation Variability

**Challenge**: Users may have different installation methods (npm, manual, etc.).

**Mitigation**:
- Check multiple installation paths
- Provide clear setup instructions
- Support `base_command_override` for custom paths

---

## 10. Verification Checklist

### 10.1 Code Changes

- [ ] Created `crates/executors/src/executors/kilo.rs`
- [ ] Added `pub mod kilo;` to `mod.rs`
- [ ] Added `KiloCode` to `CodingAgent` enum
- [ ] Added to imports in `mod.rs`
- [ ] Added MCP config handler
- [ ] Added capabilities entry
- [ ] Updated `default_profiles.json`
- [ ] Updated `generate_types.rs`

### 10.2 Frontend Changes

- [ ] Created `kilo-dark.svg`
- [ ] Created `kilo-light.svg`
- [ ] Updated `getAgentName()` in `AgentIcon.tsx`
- [ ] Added icon path switch case in `AgentIcon.tsx`

### 10.3 Type Generation

- [ ] Ran `pnpm run generate-types`
- [ ] Verified `shared/types.ts` contains KILO_CODE
- [ ] Verified JSON schema generated

### 10.4 Testing

- [ ] `cargo check --package executors` passes
- [ ] `cargo test --package executors --lib` passes
- [ ] `pnpm run check` passes (frontend)
- [ ] Agent icon renders correctly
- [ ] Agent can be selected in UI
- [ ] Agent spawns successfully
- [ ] Log output is properly normalized
- [ ] Follow-up sessions work

### 10.5 Documentation

- [ ] Updated AGENTS.md with Kilo Code CLI info
- [ ] Added setup instructions for users
- [ ] Documented configuration options

---

## 11. Implementation Order

1. **Phase 1: Core Backend**
   - Create `kilo.rs` executor struct
   - Implement `StandardCodingAgentExecutor` trait
   - Add to `CodingAgent` enum

2. **Phase 2: Configuration**
   - Update `default_profiles.json`
   - Update `generate_types.rs`
   - Run type generation

3. **Phase 3: Frontend**
   - Create SVG icons
   - Update `AgentIcon.tsx`

4. **Phase 4: Testing**
   - Unit tests
   - Integration tests
   - E2E verification

5. **Phase 5: Documentation**
   - Update AGENTS.md
   - User setup instructions

---

## 12. Rollback Plan

If issues are discovered after integration:

1. **Disable via feature flag**: Add `kilo_code` feature to `Cargo.toml`
2. **Remove from enum**: Comment out `KiloCode` in `CodingAgent`
3. **Remove profiles**: Delete KILO_CODE from `default_profiles.json`
4. **Frontend**: Revert `AgentIcon.tsx` changes
5. **Types**: Regenerate types without KiloCode

---

## 13. Future Enhancements

Post-integration enhancements to consider:

1. **Mode-specific prompts**: Customize prompts per mode
2. **Skills integration**: Load Kilo Code skills from `.kilocode/skills/`
3. **Custom commands**: Support Kilo Code custom commands
4. **Checkpoint management**: Integrate with Kilo Code checkpoint system
5. **Parallel mode**: Support concurrent Kilo Code instances
6. **Telemetry**: Track Kilo Code-specific metrics

---

## 14. References

- [Kilo Code CLI Documentation](docs/KILO.md)
- [Gemini Executor Implementation](crates/executors/src/executors/gemini.rs)
- [ACP Harness](crates/executors/src/executors/acp/harness.rs)
- [ACP Log Normalization](crates/executors/src/executors/acp/normalize_logs.rs)
- [Claude Executor](crates/executors/src/executors/claude.rs) - Full custom implementation reference
