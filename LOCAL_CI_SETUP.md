# Local CI Setup Guide

This guide explains how to set up and use **local-ci** with knittingCrab for pre-push validation.

## What is local-ci?

[local-ci](https://github.com/stevedores-org/local-ci) is a lightweight CI/CD tool that runs the same checks locally as your GitHub Actions pipeline, ensuring code passes validation before pushing.

**Benefits:**
- 🚀 **Fast feedback** - Catch CI failures before pushing to GitHub
- 🔗 **Identical checks** - Validates against the same stages as GitHub Actions
- 📊 **Clear reporting** - Detailed output showing which checks passed/failed
- 🛑 **Pre-push blocking** - Optional git hook prevents pushes that fail CI

## Installation

### Option 1: Using Homebrew (macOS/Linux)

```bash
brew install stevedores-org/local-ci/local-ci
```

### Option 2: Using Cargo

```bash
cargo install local-ci
```

### Option 3: From Source

```bash
git clone https://github.com/stevedores-org/local-ci.git
cd local-ci
cargo install --path .
```

### Verify Installation

```bash
local-ci --version
local-ci --help
```

## Configuration

The project includes a `.local-ci.yml` file that defines all CI stages:

```yaml
stages:
  - name: fmt          # Code formatting
  - name: clippy       # Lint checks
  - name: build        # Build all targets
  - name: test         # Run tests
  - name: security     # Security audit (optional)
```

This mirrors the GitHub Actions workflow in `.github/workflows/ci.yml`.

## Usage

### Run All Checks

```bash
# Run all CI stages in order
local-ci run

# Equivalent to: fmt → clippy → build → test → security
```

### Run Specific Stage

```bash
# Run only formatting check
local-ci run --stage fmt

# Run formatting and clippy only
local-ci run --stage fmt --stage clippy
```

### Dry Run (Show what would run, don't execute)

```bash
local-ci run --dry-run
```

### Verbose Output

```bash
# Show detailed command output
local-ci run --verbose
```

### Skip Optional Stages

```bash
# Skip security audit (which is marked optional)
local-ci run --skip security
```

## Git Hook Integration

### Enable Pre-push Hook

The `.githooks/pre-push` hook automatically validates changes before allowing pushes:

```bash
# Configure git to use our hooks
git config core.hooksPath .githooks

# Make hook executable (if needed)
chmod +x .githooks/pre-push
```

### How It Works

1. **Before push**: Git calls `.githooks/pre-push`
2. **Validation**: Hook runs `local-ci run`
3. **If passes**: Push proceeds normally
4. **If fails**: Push is blocked with error message

### Disable Pre-push Hook Temporarily

```bash
# Push without running pre-push hook
git push --no-verify

# ⚠️ Not recommended - use only if you know what you're doing
```

## Typical Workflow

### During Development

```bash
# Make changes
vim crates/core/src/lib.rs

# Test locally (without running full CI)
cargo test --lib

# When ready to commit, check formatting
cargo fmt --all

# Before pushing, validate with local-ci
local-ci run

# If all checks pass:
git push  # Pre-push hook will validate again
```

### If local-ci Fails

```bash
# View which stage failed
local-ci run

# Fix the issue, e.g., for formatting:
cargo fmt --all

# Re-run to verify
local-ci run --stage fmt

# Then run full checks
local-ci run

# Push when ready
git push
```

## CI Stages Explained

### 1. Code Formatting (`fmt`)
```bash
cargo fmt --all -- --check
```
- Validates code follows Rust formatting standards
- **Fix**: `cargo fmt --all`

### 2. Clippy Lints (`clippy`)
```bash
cargo clippy --all-targets --all-features -- -D warnings
```
- Checks for common Rust anti-patterns
- **All warnings treated as errors** for quality
- **Fix**: Address clippy suggestions and re-run

### 3. Build (`build`)
```bash
cargo build --all-targets --verbose
```
- Compiles all targets
- Ensures no compilation errors

### 4. Tests (`test`)
```bash
cargo test --all-targets --verbose && cargo test --doc
```
- Runs unit, integration, and doc tests
- Must all pass

### 5. Security Audit (`security`) - Optional
```bash
cargo audit --deny warnings
```
- Scans dependencies for known vulnerabilities
- Marked optional (non-blocking)
- **Action**: Update vulnerable dependencies

## Troubleshooting

### local-ci command not found

```bash
# Install local-ci
brew install stevedores-org/local-ci/local-ci

# Or via cargo
cargo install local-ci
```

### Pre-push hook not running

```bash
# Ensure hooks directory is configured
git config core.hooksPath .githooks

# Make hook executable
chmod +x .githooks/pre-push

# Verify setup
git config core.hooksPath  # Should show: .githooks
```

### Specific stage fails locally but passes on GitHub

```bash
# Check Rust toolchain
rustc --version
cargo --version

# Update Rust
rustup update

# Clean and rebuild
cargo clean
local-ci run
```

### Want to skip validation temporarily

```bash
# Run without git hook
git push --no-verify

# Or disable hook temporarily
chmod -x .githooks/pre-push
# ... make push ...
chmod +x .githooks/pre-push

# ⚠️ Not recommended
```

## Integration with CI/CD

The local-ci configuration maps directly to GitHub Actions:

| local-ci Stage | GitHub Action | Status Badge |
|---|---|---|
| `fmt` | Code Formatting | 📝 |
| `clippy` | Clippy Lints | 🔍 |
| `build` | Build | 🏗️ |
| `test` | Tests | ✅ |
| `security` | Security Audit | 🔐 |

Passing local-ci guarantees GitHub Actions will pass.

## Benefits of Local-CI Adoption

✅ **Development Speed**: Catch issues before pushing
✅ **CI Confidence**: Run identical checks locally
✅ **Less Re-work**: Avoid GitHub Actions re-runs
✅ **Clear Feedback**: Know exactly what failed
✅ **Team Alignment**: Everyone uses same validation

## Next Steps

1. **Install local-ci** using one of the methods above
2. **Configure git hooks**: `git config core.hooksPath .githooks`
3. **Test setup**: `local-ci run`
4. **Try making a change** and verify pre-push hook works
5. **Add to team documentation** and development guide

## Questions?

See the [local-ci documentation](https://github.com/stevedores-org/local-ci) or run:

```bash
local-ci --help
local-ci run --help
```
