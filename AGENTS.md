# Repository Guidelines

## Project Structure & Module Organization
- `crates/`: Rust workspace crates — `server` (API + bins), `db` (SQLx models/migrations), `executors`, `services`, `utils`, `deployment`, `local-deployment`, `remote`.
- `frontend/`: React + TypeScript app (Vite, Tailwind). Source in `frontend/src`.
- `frontend/src/components/dialogs`: Dialog components for the frontend.
- `remote-frontend/`: Remote deployment frontend.
- `shared/`: Generated TypeScript types (`shared/types.ts`). Do not edit directly.
- `assets/`, `dev_assets_seed/`, `dev_assets/`: Packaged and local dev assets.
- `npx-cli/`: Files published to the npm CLI package.
- `scripts/`: Dev helpers (ports, DB preparation).
- `docs/`: Documentation files.

## Managing Shared Types Between Rust and TypeScript

ts-rs allows you to derive TypeScript types from Rust structs/enums. By annotating your Rust types with #[derive(TS)] and related macros, ts-rs will generate .ts declaration files for those types.
When making changes to the types, you can regenerate them using `pnpm run generate-types`
Do not manually edit shared/types.ts, instead edit crates/server/src/bin/generate_types.rs

## Build, Test, and Development Commands
- Install: `pnpm i`
- Run dev (frontend + backend with ports auto-assigned): `pnpm run dev`
- Backend (watch): `pnpm run backend:dev:watch`
- Frontend (dev): `pnpm run frontend:dev`
- Type checks: `pnpm run check` (frontend) and `pnpm run backend:check` (Rust cargo check)
- Rust tests: `cargo test --workspace`
- Generate TS types from Rust: `pnpm run generate-types` (or `generate-types:check` in CI)
- Prepare SQLx (offline): `pnpm run prepare-db`
- Prepare SQLx (remote package, postgres): `pnpm run remote:prepare-db`
- Local NPX build: `pnpm run build:npx` then `pnpm pack` in `npx-cli/`

## Automated QA
- When testing changes by running the application, you should prefer `pnpm run dev:qa` over `pnpm run dev`, which starts the application in a dedicated mode that is optimised for QA testing

## Coding Style & Naming Conventions
- Rust: `rustfmt` enforced (`rustfmt.toml`); group imports by crate; snake_case modules, PascalCase types.
- TypeScript/React: ESLint + Prettier (2 spaces, single quotes, 80 cols). PascalCase components, camelCase vars/functions, kebab-case file names where practical.
- Keep functions small, add `Debug`/`Serialize`/`Deserialize` where useful.

## Testing Guidelines
- Rust: prefer unit tests alongside code (`#[cfg(test)]`), run `cargo test --workspace`. Add tests for new logic and edge cases.
- Frontend: ensure `pnpm run check` and `pnpm run lint` pass. If adding runtime logic, include lightweight tests (e.g., Vitest) in the same directory.

## Security & Config Tips
- Use `.env` for local overrides; never commit secrets. Key envs: `FRONTEND_PORT`, `BACKEND_PORT`, `HOST`
- Dev ports and assets are managed by `scripts/setup-dev-environment.js`.

---

## Kilo Code CLI Agent

### Description

Kilo Code CLI (`@kilocode/cli`) is a terminal-based AI coding assistant that supports multiple operational modes and model switching. It integrates with the vibe-kanban project using the Agent Communication Protocol (ACP) via the `--experimental-acp` flag.

**Key Features:**
- Multiple LLM model support (switch between providers freely)
- Agent Skills (extendable capabilities via `~/.kilocode/skills/`)
- Custom Commands (`~/.kilocode/commands/`)
- Checkpoint management for state recovery
- Task history and search
- Parallel mode for concurrent work
- Auto-approval settings configuration

**Available Modes:**
- **Architect** - Planning and architecture design
- **Code** - General coding tasks
- **Ask** - Question-answering mode
- **Debug** - Troubleshooting and debugging
- **Orchestrator** - Complex multi-step tasks
- **Custom modes** - User-defined modes

### Installation Requirements

```bash
npm install -g @kilocode/cli
```

**Requirements:**
- Node.js 18+ and npm
- Home indicator file created at `~/.kilocode/installation_id` after first run

**Availability Check:**
The executor checks for installation by looking for `~/.kilocode/installation_id`. If found, it reports `AvailabilityInfo::LoginDetected` with the last authentication timestamp.

### Configuration Options

The Kilo Code executor supports the following configuration options:

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `mode` | `Option<String>` | `None` | Operating mode (e.g., "code", "architect", "debug", "orchestrator") |
| `model` | `Option<String>` | `None` | LLM model to use |
| `yolo` | `Option<bool>` | `false` | Enable auto-approval (no confirmation prompts) |
| `append_prompt` | `AppendPrompt` | empty | Additional prompt to append to all requests |
| `cmd` | `CmdOverrides` | empty | Command overrides for executable path |

**Environment Variables:**
- MCP configuration is read from `~/.kilocode/config.json`

### Default Profiles

The following profiles are configured in `crates/executors/default_profiles.json`:

```json
{
  "KILO_CODE": {
    "DEFAULT": { "yolo": true },
    "CODE": { "mode": "code", "yolo": true },
    "ARCHITECT": { "mode": "architect", "yolo": true },
    "DEBUG": { "mode": "debug", "yolo": true },
    "ORCHESTRATOR": { "mode": "orchestrator", "yolo": true },
    "APPROVALS": { "mode": "code", "yolo": false }
  }
}
```

### Basic Commands

| Command | Description |
|---------|-------------|
| `kilocode` | Start interactive chat session |
| `kilocode --mode architect` | Start with specific mode |
| `kilocode --workspace /path` | Start with specific workspace |
| `kilocode --continue` | Resume last conversation |

### Special Notes

1. **ACP Protocol:** Kilo Code uses the experimental ACP protocol (`--experimental-acp` flag) for integration. This enables structured tool call handling, session management, and log normalization.

2. **MCP Configuration:** MCP servers are configured via `~/.kilocode/config.json` under the `mcpServers` key.

3. **Session Forking:** Kilo Code supports session forking, allowing parallel agent sessions for concurrent work.

4. **Command Builder:** The executor uses `npx -y @kilocode/cli@latest` by default. Override using `cmd.binary_path` in configuration.

5. **YOLO Mode:** When `yolo: true`, all tool calls are auto-approved without user confirmation. Set to `false` for interactive approval workflows.

6. **Agent Skills:** Custom skills can be added to `~/.kilocode/skills/` directory for extended capabilities.
