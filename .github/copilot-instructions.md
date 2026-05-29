# GitHub Copilot Instructions — knittingCrab

- **Project Type**: Rust Workspace (multiple crates).
- **Core Stack**: Rust, Tokio, Nix, SSH, tmux.
- **Conventions**:
  - Prefer async/await via Tokio.
  - Follow TDD: write tests for logic in `tests/` or `#[cfg(test)]` modules.
  - Use `tracing` for logging.
  - Adhere to the "Lease-based" concurrency model for task execution.
- **Style**:
  - Idiomatic Rust (2021 edition).
  - No unsafe blocks without `// SAFETY:` comments.
  - Clean clippy output is mandatory.
