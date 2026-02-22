# knittingCrab 🦀

A resource-aware local task scheduler for AI agent workloads, optimized for macOS M4 Max.

## Status

**Epic 2: Worker Runtime** ✅ Complete
- Full task lifecycle management
- Concurrent lease coordination
- Exponential backoff retry logic
- Process spawning with graceful shutdown
- 36+ tests passing, zero warnings

## Quick Start

### Prerequisites
- Rust 1.93+
- macOS (M1/M2/M3/M4 or Intel)
- Pre-commit hooks (optional)

### Installation

```bash
git clone https://github.com/stevedores-org/knittingCrab.git
cd knittingCrab
cargo build --release
```

### Running Tests

```bash
# All tests
cargo test --all

# Specific crate
cargo test -p knitting-crab-worker --lib

# With strict warnings
RUSTFLAGS="-D warnings" cargo test --all

# Pre-commit checks
pre-commit run --all-files
```

## Architecture

### Workspace Structure

```
crates/
├── core/          # Shared types & trait stubs (Epic 1 boundary)
├── worker/        # Worker runtime (Epic 2 complete)
└── scheduler/     # Task queue stub (for testing)
```

### Key Components

- **CancelToken/CancelGuard**: Graceful task cancellation
- **LeaseManager**: Prevents duplicate task execution
- **RetryHandler**: Exponential backoff retry logic
- **ProcessHandle**: OS process spawning & lifecycle management
- **FakeWorker**: Test double without OS processes
- **WorkerRuntime**: Main task orchestration loop

## Testing

- **36+ tests** passing with zero warnings
- **Unit tests**: Leases, retries, cancellation
- **Component tests**: Log streaming, process spawning
- **Integration tests**: Real subprocess execution
- **Pre-commit hooks**: Auto-format and lint

## Code Quality

- Zero unsafe code (except required for process groups)
- All warnings denied: `RUSTFLAGS="-D warnings"`
- Clippy strict mode: `cargo clippy -- -D warnings`

## Documentation

- **[IMPLEMENTATION.md](IMPLEMENTATION.md)**: Detailed architecture & design decisions
- Inline documentation for all public APIs
- Test examples in `crates/worker/tests/`

## Roadmap

- Epic 3: Workspace Isolation
- Epic 4: Artifact Browsing
- Future: Distributed scheduling, cloud orchestration

## License

See [LICENSE](LICENSE) file.
