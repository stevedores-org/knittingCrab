# knittingCrab — Gemini CLI Instructions

## Project Context

**knittingCrab** is a distributed task scheduler written in Rust, optimized for AI workloads on Apple Silicon. It uses a lease-based system to ensure task exclusivity and supports remote execution via SSH/tmux.

## Core Mandates

1. **Safety First**: Use process groups (`setpgid`) and graceful signal handling (SIGTERM -> SIGKILL) for all managed tasks.
2. **Determinism**: Ensure task execution order respects the DAG and priority levels.
3. **Traceability**: All process events (spawn, exit, log) must be emitted to the configured `EventSink`.
4. **Remote context**: When executing remotely, ensure `repo_name` and `work_id` are correctly sanitized to prevent shell injection and path traversal.

## Development Patterns

- **Trait boundaries**: Always program against traits (e.g., `ProcessExecutor`, `RemoteSessionManagerTrait`) to allow for easy mocking.
- **TDD-First**: Add unit or integration tests for every new feature or bug fix.
- **Clippy hygiene**: Enforce zero warnings in CI (`-D warnings`).

## Required Files (7-File Rule)

Maintain consistency across the following files:
1. `README.md`: High-level overview and status.
2. `CLAUDE.md`: Build/test commands.
3. `AGENTS.md`: Capability registry.
4. `GEMINI.md`: This file.
5. `.cursorrules`: IDE rules.
6. `.github/copilot-instructions.md`: Copilot context.
7. `.github/system-instruction.md`: Intelligence standards.

## Useful Commands

```bash
# Run all tests
cargo test --all

# Check clippy
cargo clippy --all-targets --all-features -- -D warnings

# Build all crates
cargo build --all-targets
```
