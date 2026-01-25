# Agent Integration Guide

This guide documents how to integrate a new coding agent into the vibe-kanban executor system. The system is designed to support multiple agent implementations through a common trait-based interface.

## Architecture Overview

### Core Components

```
┌─────────────────────────────────────────────────────────────────┐
│                      vibe-kanban Server                         │
├─────────────────────────────────────────────────────────────────┤
│  ExecutorAction → CodingAgentInitialRequest/CodingAgentFollowUp │
└───────────────────────────┬─────────────────────────────────────┘
                            │
                            ▼
┌─────────────────────────────────────────────────────────────────┐
│                   StandardCodingAgentExecutor                    │
│                          (Trait)                                │
├─────────────────────────────────────────────────────────────────┤
│  ClaudeCode │ Copilot │ Cursor │ Gemini │ Codex │ AMP │ ...     │
└─────────────────────────────────────────────────────────────────┘
```

### Module Structure

```
crates/executors/src/
├── lib.rs                 # Main module exports
├── command.rs             # Command building and execution (CmdOverrides, CommandBuilder)
├── env.rs                 # Execution environment (env vars, repo context)
├── profile.rs             # Profile configuration management
├── mcp_config.rs          # MCP server configuration handling
├── stdout_dup.rs          # Stdout pipe handling for log duplication
├── approvals/             # Executor approval service for tool execution control
├── logs/                  # Log normalization
│   ├── mod.rs
│   ├── plain_text_processor.rs
│   ├── stderr_processor.rs
│   └── utils/
├── actions/               # Action types for agent invocation
│   ├── mod.rs
│   ├── coding_agent_initial.rs
│   ├── coding_agent_follow_up.rs
│   └── review.rs
└── executors/             # Agent implementations
    ├── mod.rs             # Main executor module, CodingAgent enum, trait definition
    ├── claude.rs          # Claude Code implementation (custom protocol)
    ├── copilot.rs         # GitHub Copilot implementation
    ├── cursor.rs          # Cursor Agent implementation
    ├── gemini.rs          # Google Gemini CLI (ACP protocol)
    ├── amp.rs             # AMP (Sourcegraph) - stdio with threads fork
    ├── codex.rs           # OpenAI Codex
    ├── droid.rs           # Gemini CLI (Android)
    ├── opencode.rs        # OpenCode AI
    ├── qwen.rs            # Qwen Code (ACP protocol)
    ├── utils/             # Shared utilities
    └── acp/               # ACP protocol harness (for Gemini, Qwen)
        ├── mod.rs
        ├── harness.rs     # AcpAgentHarness for ACP-based agents
        ├── client.rs      # ACP client implementation
        └── session.rs     # Session management
```

## The Core Trait: `StandardCodingAgentExecutor`

All agents must implement the [`StandardCodingAgentExecutor`](crates/executors/src/executors/mod.rs:211) async trait, annotated with `#[enum_dispatch(CodingAgent)]`. This is the primary interface for agent integration.

### Required Methods

```rust
#[async_trait]
#[enum_dispatch(CodingAgent)]
pub trait StandardCodingAgentExecutor {
    /// Spawn a new agent session with the given prompt
    async fn spawn(
        &self,
        current_dir: &Path,
        prompt: &str,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError>;

    /// Spawn a follow-up in an existing session
    async fn spawn_follow_up(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: &str,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError>;

    /// Normalize raw agent logs into standard format
    fn normalize_logs(&self, raw_logs_event_store: Arc<MsgStore>, worktree_path: &Path);

    /// Return the default MCP config path for this agent
    fn default_mcp_config_path(&self) -> Option<PathBuf>;
}
```

### Optional Methods

```rust
trait StandardCodingAgentExecutor {
    /// Inject approval service for tool execution control
    fn use_approvals(&mut self, approvals: Arc<dyn ExecutorApprovalService>) {}

    /// Get available slash commands (returns patch stream)
    async fn available_slash_commands(
        &self,
        workdir: &Path,
    ) -> Result<BoxStream<'static, json_patch::Patch>, ExecutorError> {
        Ok(Box::pin(futures::stream::once(async move {
            patch::slash_commands(Vec::new(), false, None)
        })))
    }

    /// Handle spawn_review (defaults to spawn or spawn_follow_up)
    async fn spawn_review(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: Option<&str>,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        match session_id {
            Some(id) => self.spawn_follow_up(current_dir, prompt, id, env).await,
            None => self.spawn(current_dir, prompt, env).await,
        }
    }

    /// Get setup action for agents requiring login/install
    async fn get_setup_helper_action(&self) -> Result<ExecutorAction, ExecutorError> {
        Err(ExecutorError::SetupHelperNotSupported)
    }

    /// Check agent availability (login status, installation)
    fn get_availability_info(&self) -> AvailabilityInfo {
        let config_exists = self
            .default_mcp_config_path()
            .map(|p| p.exists())
            .unwrap_or(false);

        if config_exists {
            AvailabilityInfo::InstallationFound
        } else {
            AvailabilityInfo::NotFound
        }
    }
}
```

## Agent Registration

### 1. Add to the `CodingAgent` Enum

In [`executors/mod.rs`](executors/mod.rs:105):

```rust
#[enum_dispatch]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, Display, EnumDiscriminants, VariantNames)]
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
    #[cfg(feature = "qa-mode")]
    QaMock(QaMockExecutor),
    YourNewAgent,  // <-- Add here
}
```

**Note:** Use `#[serde(alias = "...")]` and `#[strum_discriminants(...)]` for backward compatibility (e.g., CursorAgent accepts both `CURSOR_AGENT` and legacy `CURSOR`).

**Feature-gated variants:** Use `#[cfg(feature = "...")]` on the variant and add corresponding import.

### 2. Add Module Declaration

In [`executors/mod.rs`](executors/mod.rs:31-43):

```rust
pub mod acp;
pub mod amp;
pub mod claude;
pub mod codex;
pub mod copilot;
pub mod cursor;
pub mod droid;
pub mod gemini;
pub mod opencode;
pub mod qwen;
pub mod utils;

// Conditionally compiled modules
#[cfg(feature = "qa-mode")]
pub mod qa_mock;
```

### 3. Import and Match in Trait Implementations

Update `CodingAgent::get_mcp_config()`, `CodingAgent::capabilities()`, and `CodingAgent::supports_mcp()` to handle your new agent.

### 4. Conditionally Compile (if needed)

If your agent is gated behind a feature flag, add the feature to `crates/executors/Cargo.toml`:

```toml
[features]
qa-mode = []
your-feature = []
```

And add the import in [`executors/mod.rs`](executors/mod.rs:16-17):

```rust
#[cfg(feature = "your-feature")]
use crate::executors::your_agent::YourAgent;
```

## Agent Implementation Structure

### Basic Implementation Template

```rust
// crates/executors/src/executors/your_agent.rs

use std::{path::Path, process::Stdio, sync::Arc};
use async_trait::async_trait;
use command_group::AsyncCommandGroup;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::{io::AsyncWriteExt, process::Command};
use ts_rs::TS;
use workspace_utils::msg_store::MsgStore;

use crate::{
    command::{CmdOverrides, CommandBuildError, CommandBuilder, apply_overrides},
    env::ExecutionEnv,
    executors::{
        AppendPrompt, AvailabilityInfo, ExecutorError, SpawnedChild, 
        StandardCodingAgentExecutor,
    },
    logs::{stderr_processor::normalize_stderr_logs, utils::EntryIndexProvider},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, JsonSchema)]
pub struct YourAgent {
    #[serde(default)]
    pub append_prompt: AppendPrompt,
    
    // Your agent-specific configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub your_option: Option<String>,
    
    #[serde(flatten)]
    pub cmd: CmdOverrides,
}

impl YourAgent {
    fn build_command_builder(&self) -> Result<CommandBuilder, CommandBuildError> {
        let mut builder = CommandBuilder::new("npx -y @your-org/your-agent@version")
            .params(["--your-flags"]);
        
        // Add your agent-specific parameters
        if let Some(opt) = &self.your_option {
            builder = builder.extend_params(["--option", opt]);
        }
        
        apply_overrides(builder, &self.cmd)
    }
}

#[async_trait]
impl StandardCodingAgentExecutor for YourAgent {
    async fn spawn(
        &self,
        current_dir: &Path,
        prompt: &str,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        let command_parts = self.build_command_builder()?.build_initial()?;
        let (executable_path, args) = command_parts.into_resolved().await?;

        let combined_prompt = self.append_prompt.combine_prompt(prompt);

        let mut command = Command::new(executable_path);
        command
            .kill_on_drop(true)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(current_dir)
            .env("NPM_CONFIG_LOGLEVEL", "error")
            .args(&args);

        env.clone()
            .with_profile(&self.cmd)
            .apply_to_command(&mut command);

        let mut child = command.group_spawn()?;

        // Feed prompt to stdin
        if let Some(mut stdin) = child.inner().stdin.take() {
            stdin.write_all(combined_prompt.as_bytes()).await?;
            stdin.shutdown().await?;
        }

        Ok(child.into())
    }

    async fn spawn_follow_up(
        &self,
        current_dir: &Path,
        prompt: &str,
        session_id: &str,
        env: &ExecutionEnv,
    ) -> Result<SpawnedChild, ExecutorError> {
        // Build command with session continuation flags
        let command_parts = self.build_command_builder()?.build_follow_up(&[
            "--resume".to_string(),
            session_id.to_string(),
        ])?;
        
        // ... (same spawn pattern as above)
        
        Ok(child.into())
    }

    fn normalize_logs(&self, msg_store: Arc<MsgStore>, worktree_path: &Path) {
        // Normalize agent-specific logs to standard format
        normalize_stderr_logs(msg_store, worktree_path);
    }

    fn default_mcp_config_path(&self) -> Option<PathBuf> {
        // Return the agent's MCP config location
        dirs::home_dir().map(|home| home.join(".your-agent").join("mcp.json"))
    }

    fn get_availability_info(&self) -> AvailabilityInfo {
        // Check if agent is installed/logged in
        let config_exists = self
            .default_mcp_config_path()
            .map(|p| p.exists())
            .unwrap_or(false);

        if config_exists {
            AvailabilityInfo::InstallationFound
        } else {
            AvailabilityInfo::NotFound
        }
    }
}
```

## Communication Patterns

### Pattern 1: Simple Stdio (Most Common)

Agents communicate via stdin/stdout pipes with JSON streaming:

```rust
// Prompt is written to stdin, output is read from stdout
stdin.write_all(combined_prompt.as_bytes()).await?;
stdout = child.inner().stdout.take();
```

### Pattern 2: ACP Protocol (Agent Client Protocol)

For agents supporting the [ACP protocol](https://github.com/agentprotocol/spec) (Gemini, Qwen), use the `AcpAgentHarness`:

```rust
use crate::executors::acp::AcpAgentHarness;

let harness = AcpAgentHarness::new()
    .with_session_namespace("your_namespace");

// For initial spawn:
harness.spawn_with_command(
    current_dir,
    prompt,
    command_parts,
    env,
    &self.cmd,
    approvals_service,
).await

// For follow-up (session continuation):
harness.spawn_follow_up_with_command(
    current_dir,
    prompt,
    session_id,
    command_parts,
    env,
    &self.cmd,
    approvals_service,
).await
```

The ACP harness handles:
- Session creation and forking via `SessionManager`
- Prompt/response handling using the protocol
- Event persistence to session files
- Exit signaling (`ExecutorExitSignal` in `SpawnedChild`)
- Approval service integration for tool execution control

See [`executors/acp/harness.rs`](crates/executors/src/executors/acp/harness.rs) for full implementation.

### Pattern 3: Custom Protocol (Claude Code)

Claude Code uses a custom JSON protocol over stdio with a control channel:

```rust
// Output format: stream-json, Input format: stream-json
builder = builder.extend_params([
    "--output-format=stream-json",
    "--input-format=stream-json",
    "--include-partial-messages",
]);
```

Claude Code's implementation uses a `ProtocolPeer` for control messages and `ClaudeAgentClient` for SDK integration. See [`executors/claude/`](crates/executors/src/executors/claude/) for the full implementation.

## Log Normalization

Agents output logs in various formats. The system normalizes them to a standard format defined in [`logs/mod.rs`](crates/executors/src/logs/mod.rs):

```rust
pub enum NormalizedEntryType {
    UserMessage,
    AssistantMessage,
    ToolUse {
        tool_name: String,
        action_type: ActionType,
        status: ToolStatus,
    },
    ToolResult {
        tool_use_id: String,
        output: String,
        status: ToolStatus,
    },
    ErrorMessage { error_type: NormalizedEntryError },
    SystemMessage,
    Thinking,
    TodoList { items: Vec<TodoItem> },
    FileChange { changes: Vec<FileChange> },
}
```

### Implement Log Normalization

Most agents normalize both stdout and stderr:

```rust
fn normalize_logs(&self, msg_store: Arc<MsgStore>, worktree_path: &Path) {
    let entry_index_provider = EntryIndexProvider::start_from(&msg_store);

    // Process stdout logs
    Self::process_stdout_logs(
        msg_store.clone(),
        worktree_path,
        entry_index_provider.clone(),
    );

    // Process stderr logs
    normalize_stderr_logs(msg_store, entry_index_provider);
}
```

Common patterns:
- **Claude**: Uses `ClaudeLogProcessor::process_logs()` for JSON stream parsing
- **Copilot**: Uses `PlainTextLogProcessor` for plain text output
- **AMP**: Reuses Claude's log processor with `HistoryStrategy::AmpResume`

See [`logs/stderr_processor.rs`](crates/executors/src/logs/stderr_processor.rs) for the stderr processor implementation.

## Configuration System

### Profile Configuration (`default_profiles.json`)

Add your agent to [`default_profiles.json`](default_profiles.json):

```json
{
  "executors": {
    "YOUR_AGENT": {
      "DEFAULT": {
        "YOUR_AGENT": {
          "your_option": "default_value"
        }
      },
      "VARIANT_NAME": {
        "YOUR_AGENT": {
          "your_option": "variant_value"
        }
      }
    }
  }
}
```

### Configuration Loading

Profiles are loaded via [`ExecutorConfigs::load()`](crates/executors/src/profile.rs:210):

1. Load defaults from embedded `default_profiles.json`
2. Merge with user overrides from `profiles.json` (via `workspace_utils::assets::profiles_path()`)
3. Canonicalize variant keys (SCREAMING_SNAKE_CASE)
4. Cache the result globally using `LazyLock<RwLock>`

Key functions:
- `ExecutorConfigs::get_cached()` - Get cached profiles
- `ExecutorConfigs::reload()` - Reload from disk
- `ExecutorConfigs::save_overrides()` - Save user overrides
- `ExecutorConfigs::get_recommended_executor_profile()` - Find available agent

### Configuration Options

Common options inherited from [`CmdOverrides`](crates/executors/src/command.rs:44):

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, JsonSchema, Default)]
pub struct CmdOverrides {
    /// Override the base command with a custom command
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_command_override: Option<String>,
    
    /// Additional parameters to append to the base command
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub additional_params: Option<Vec<String>>,
    
    /// Environment variables to set when running the executor
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
}
```

### CommandBuilder

```rust
pub struct CommandBuilder {
    pub base: String,           // Base executable (e.g., "npx -y @anthropic/claude-code@latest")
    pub params: Option<Vec<String>>, // Optional parameters
}

impl CommandBuilder {
    pub fn new<S: Into<String>>(base: S) -> Self { ... }
    pub fn params<I>(self, params: I) -> Self where I: IntoIterator { ... }
    pub fn extend_params<I>(self, more: I) -> Self where I: IntoIterator { ... }
    pub fn build_initial(&self) -> Result<CommandParts, CommandBuildError> { ... }
    pub fn build_follow_up(&self, additional_args: &[String]) -> Result<CommandParts, CommandBuildError> { ... }
}

pub struct CommandParts {
    pub program: String,
    pub args: Vec<String>,
}

impl CommandParts {
    pub async fn into_resolved(self) -> Result<(PathBuf, Vec<String>), ExecutorError> { ... }
}
```

## SpawnedChild Structure

The `SpawnedChild` struct is returned by `spawn()` and `spawn_follow_up()`:

```rust
pub struct SpawnedChild {
    pub child: AsyncGroupChild,  // The spawned process group
    /// Executor → Container: signals when executor wants to exit
    pub exit_signal: Option<ExecutorExitSignal>,
    /// Container → Executor: signals when container wants to interrupt
    pub interrupt_sender: Option<InterruptSender>,
}

pub type ExecutorExitSignal = tokio::sync::oneshot::Receiver<ExecutorExitResult>;
pub type InterruptSender = tokio::sync::oneshot::Sender<()>;

pub enum ExecutorExitResult {
    Success,   // Process completed successfully (exit code 0)
    Failure,   // Process should be marked as failed (non-zero exit)
}
```

**Important patterns:**
- Use `child.inner().stdin.take()` to access stdin
- Use `command.group_spawn()` to spawn with process group
- Set `kill_on_drop(true)` for proper cleanup
- The `exit_signal` is used by ACP-based agents to signal completion

## MCP Server Configuration

Each agent may have different MCP configuration paths and formats. Implement `default_mcp_config_path()` to return the agent's config location.

The system handles different config formats:
- **JSON**: Standard JSON
- **JSONC**: JSON with comments (preserved during writes)
- **TOML**: TOML format

Agent-specific MCP paths:
- **Claude**: `~/.claude.json`
- **Copilot**: `~/.copilot/mcp-config.json`
- **AMP**: `~/.config/amp/settings.json`
- **Opencode**: `~/.config/opencode/config.json`

## Agent Capabilities

Declare agent capabilities in `CodingAgent::capabilities()`:

```rust
pub enum BaseAgentCapability {
    SessionFork,      // Supports session continuation
    SetupHelper,      // Needs setup/login before use
}
```

## Availability Detection

Implement `get_availability_info()` to indicate if the agent is available:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum AvailabilityInfo {
    LoginDetected { last_auth_timestamp: i64 },
    InstallationFound,
    NotFound,
}

impl AvailabilityInfo {
    pub fn is_available(&self) -> bool {
        matches!(
            self,
            AvailabilityInfo::LoginDetected { .. } | AvailabilityInfo::InstallationFound
        )
    }
}
```

The system uses this to recommend available agents via `ExecutorConfigs::get_recommended_executor_profile()`. Priority order: `LoginDetected` > `InstallationFound` > `NotFound`

## Example: Detailed Claude Code Implementation

For a reference implementation with:
- Custom protocol handling (stream-json over stdio)
- Approval service integration
- Slash command support
- Complex log normalization
- Permission mode handling

See [`executors/claude.rs`](crates/executors/src/executors/claude.rs) and the `claude/` subdirectory for SDK submodules.

## Testing

1. Add unit tests for command building
2. Test log normalization with sample outputs
3. Verify profile loading and merging
4. Test availability detection

## Key Dependencies

```toml
# Cargo.toml
async-trait = "0.1"
command-group = { version = "5.0", features = ["with-tokio"] }
enum_dispatch = "0.3"
ts-rs = "..."        # TypeScript generation
schemars = "..."     # JSON Schema generation
serde = { features = ["derive"] }
tokio = { features = ["process", "io-util"] }
workspace-utils = "..."  # Contains msg_store, approvals, path utilities
```

### Common Imports

```rust
use std::{path::Path, process::Stdio, sync::Arc};
use async_trait::async_trait;
use command_group::AsyncCommandGroup;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::{io::AsyncWriteExt, process::Command};
use ts_rs::TS;
use workspace_utils::msg_store::MsgStore;

use crate::{
    command::{CmdOverrides, CommandBuildError, CommandBuilder, apply_overrides},
    env::ExecutionEnv,
    executors::{AppendPrompt, AvailabilityInfo, ExecutorError, SpawnedChild, StandardCodingAgentExecutor},
    logs::{stderr_processor::normalize_stderr_logs, utils::EntryIndexProvider},
};
```

## Common Patterns

### 1. Command Building with Overrides

```rust
fn build_command_builder(&self) -> Result<CommandBuilder, CommandBuildError> {
    let builder = CommandBuilder::new("base command")
        .params(["default", "flags"]);
    
    apply_overrides(builder, &self.cmd)
}
```

### 2. Environment Application

```rust
env.clone()
    .with_profile(&self.cmd)
    .apply_to_command(&mut command);
```

### 3. Session Resume Pattern

```rust
async fn spawn_follow_up(&self, ..., session_id: &str, ...) {
    // Build with resume flag
    let parts = self.build_command_builder()?
        .build_follow_up(&["--resume", session_id])?;
    
    // Continue session...
}
```

## Checklist for New Agent Integration

- [ ] Create new module in `executors/` with struct definition
- [ ] Add derive macros: `#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, TS, JsonSchema)]`
- [ ] Implement `StandardCodingAgentExecutor` trait (all required methods)
- [ ] Add to `CodingAgent` enum in `executors/mod.rs`
- [ ] Add module declaration in `executors/mod.rs`
- [ ] Add import statement in `executors/mod.rs` imports section
- [ ] Add to `CodingAgent::get_mcp_config()` match arms
- [ ] Add to `CodingAgent::capabilities()` match arms
- [ ] Add profile defaults to `default_profiles.json`
- [ ] Implement log normalization (stdout + stderr processing)
- [ ] Add availability detection in `get_availability_info()`
- [ ] Write unit tests for command building and log normalization
- [ ] Test end-to-end execution
- [ ] If feature-gated: Add feature to `Cargo.toml` and use `#[cfg(feature = "...")]` attributes
