# CI/CD Setup for knittingCrab

This project uses **GitHub Actions** for continuous integration, **local-ci** for pre-push validation, and **git hooks** for pre-commit checks.

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

## Local-CI Pre-push Validation

**Recommended**: Use local-ci to validate all changes before pushing to GitHub.

### Quick Start

```bash
# 1. Install local-ci
brew install stevedores-org/local-ci/local-ci

# 2. Run validation
local-ci run

# 3. Push when checks pass
git push  # Pre-push hook validates again automatically
```

For detailed setup instructions, see [LOCAL_CI_SETUP.md](LOCAL_CI_SETUP.md).

### What local-ci Validates

- ✅ Code formatting (`cargo fmt --all -- --check`)
- ✅ Clippy lints (`cargo clippy --all-targets --all-features -- -D warnings`)
- ✅ Build (`cargo build --all-targets`)
- ✅ Tests (`cargo test --all-targets && cargo test --doc`)
- ✅ Security audit (`cargo audit`)

All checks match the GitHub Actions pipeline exactly.

## Local Pre-commit Hook

Lightweight pre-commit validation (runs before committing):

```bash
# Configure git to use local hooks
git config core.hooksPath .githooks

# The pre-commit hook will run on each commit and check:
# - cargo fmt --all -- --check  (formatting)
# - cargo clippy -- -D warnings  (lints)
```

The hook prevents commits that don't pass formatting or clippy checks, catching issues early.

## Pre-push Hook (with local-ci)

Comprehensive pre-push validation (runs before pushing to GitHub):

```bash
# Already configured in .githooks/pre-push
# Automatically validates all CI stages before allowing push

# To bypass (not recommended):
git push --no-verify
```

The pre-push hook runs `local-ci run` if available, or falls back to basic cargo checks.

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
