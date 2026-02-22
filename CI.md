# CI/CD Setup for knittingCrab

This project uses GitHub Actions for continuous integration and local git hooks for pre-commit validation.

## GitHub Actions Workflow

The `.github/workflows/ci.yml` file defines the following automated checks:

### Triggered On
- **Push**: to `main`, `develop`, and feature branches (`epic*/**`, `copilot/**`, `claude-code/**`)
- **Pull Requests**: targeting `main` or `develop`

### Jobs

1. **Code Formatting** (`fmt`)
   - Runs `cargo fmt --all -- --check`
   - Ensures consistent code style

2. **Clippy Lints** (`clippy`)
   - Runs `cargo clippy --all-targets --all-features -- -D warnings`
   - Enforces all clippy warnings as errors
   - Prevents common Rust anti-patterns

3. **Build** (`build`)
   - Runs `cargo build --all-targets`
   - Compiles the entire project

4. **Unit & Integration Tests** (`test`)
   - Runs `cargo test --all-targets --verbose`
   - Runs `cargo test --doc` for documentation tests
   - Validates all functionality

5. **Security Audit** (`security`)
   - Runs `cargo audit`
   - Scans dependencies for known vulnerabilities

6. **Apple Silicon Compatibility** (`apple-silicon`)
   - Runs on `macos-latest` runner
   - Validates local Mac execution environment
   - Builds and tests native Apple Silicon binaries

## Local Pre-commit Hook

To enable local validation before pushing:

```bash
# Configure git to use local hooks
git config core.hooksPath .githooks

# The pre-commit hook will run on each commit and check:
# - cargo fmt --all -- --check  (formatting)
# - cargo clippy -- -D warnings  (lints)
```

The hook prevents commits that don't pass formatting or clippy checks, catching issues early.

## Branch Protection Rules

Recommended settings for `main` and `develop` branches:

1. **Require status checks to pass before merging**:
   - fmt
   - clippy
   - build
   - test
   - security
   - apple-silicon

2. **Require pull request reviews before merging**:
   - At least 1 approval

3. **Dismiss stale pull request approvals when new commits are pushed**

4. **Require branches to be up to date before merging**

## Running Checks Locally

```bash
# Format code
cargo fmt --all

# Check formatting
cargo fmt --all -- --check

# Run clippy
cargo clippy --all-targets --all-features -- -D warnings

# Build project
cargo build --all-targets

# Run all tests
cargo test --all-targets

# Run security audit
cargo audit
```

## Troubleshooting

### Pre-commit hook not running?
```bash
git config core.hooksPath .githooks
chmod +x .githooks/pre-commit
```

### Clippy or fmt failures locally?
```bash
# Auto-fix formatting
cargo fmt --all

# Review clippy suggestions and fix
cargo clippy --all-targets --all-features
```

### CI fails on GitHub but passes locally?
- Check Rust version: `rustc --version`
- Ensure toolchain is up to date: `rustup update`
- Clear cargo cache: `cargo clean && cargo build`
