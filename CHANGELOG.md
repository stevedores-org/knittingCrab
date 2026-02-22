# Changelog

All notable changes to knittingCrab will be documented in this file.

## [Unreleased]

### Epic 2: Worker Runtime (In Review)
- [x] Core types and trait definitions
- [x] Lease state machine with automatic expiry
- [x] Lease manager with lifecycle operations
- [x] Cancel token for graceful task cancellation
- [x] Process handle with SIGTERM→SIGKILL lifecycle
- [x] Retry handler with exponential backoff
- [x] Worker runtime orchestration loop
- [x] Fake worker for component testing
- [x] Comprehensive test suite (36+ tests)
- [x] Pre-commit hooks (fmt, clippy)
- [x] Implementation documentation
- [x] Zero unsafe code (except process groups)

### Added

#### Core (`crates/core/`)
- `ids.rs`: TaskId, WorkerId, LeaseId (UUID newtypes)
- `lease.rs`: Lease state machine (Active → Completed/Failed/Expired/Cancelled)
- `retry.rs`: RetryPolicy with exponential backoff (100ms → 2.0x → 30s)
- `event.rs`: TaskEvent and LogLine types for event streaming
- `heartbeat.rs`: HeartbeatRecord for lease renewal
- `resource.rs`: ResourceAllocation stub for Epic 1 boundary
- `traits.rs`: Queue, LeaseStore, ResourceMonitor, EventSink traits

#### Worker (`crates/worker/`)
- `cancel_token.rs`: CancelToken/CancelGuard using tokio::sync::watch
- `lease_manager.rs`: LeaseManager with lifecycle operations
  - InMemoryLeaseStore backed by DashMap
  - acquire, renew, complete, fail, cancel, collect_expired
- `process.rs`: ProcessHandle for OS process management
  - spawn with process group isolation
  - Graceful SIGTERM → grace period → SIGKILL
  - Async log readers for stdout/stderr
- `retry_handler.rs`: Pure stateless retry decision logic
- `worker_runtime.rs`: WorkerRuntime orchestration
  - worker_loop: dequeue → execute → retry cycle
  - reaper_loop: collect expired leases periodically
  - Concurrent heartbeat renewal
  - Task cancellation support
- `fake_worker.rs`: Test double implementing all traits
  - No OS process spawning
  - Configurable task behaviors (Succeed, Fail, Hang, Crash)

#### Scheduler (`crates/scheduler/`)
- `stub.rs`: StubScheduler implementing Queue trait
  - Simple VecDeque-based queue for testing

#### Configuration
- `.pre-commit-config.yaml`: Pre-commit hooks
  - cargo fmt for code formatting
  - cargo clippy with warnings-as-errors
  - Standard hooks (whitespace, YAML, TOML, merge conflicts, private keys)

#### Documentation
- `IMPLEMENTATION.md`: Detailed architecture and design guide
- `README.md`: Quick start and overview
- `CHANGELOG.md`: This file

### Test Coverage

**Unit Tests** (13 tests)
- `lease_tests.rs`: no_duplicate_leases, lease_expires_requeues, heartbeat_extends_lease
- `retry_tests.rs`: exponential backoff, max attempts, retryable code filtering
- `cancel_tests.rs`: token basics, running task cancel, idempotency
- `fake_worker_tests.rs`: queue operations, event emission

**Component Tests** (3 tests)
- `log_tests.rs`: ordered and lossless log streaming
- `process_tests.rs`: real subprocess spawning (echo)
- `runtime_tests.rs`: basic runtime operations

**Total**: 36+ tests passing with zero warnings

### macOS M4 Max Specific

- Process groups via `setpgid(0,0)` for reliable signal delivery
- SIGTERM → grace period → SIGKILL lifecycle
- Async subprocess management via `spawn_blocking`
- Signal handling with `nix::sys::signal`

### Changed

- Initial commit: baseline project structure

### Fixed

- Clippy warnings: unused imports, manual loops, unnecessary returns
- Code style: proper formatting, linting standards

### Known Limitations

- Lease storage is in-memory only (non-persistent)
- Task queue is simple FIFO (no priorities)
- Resource monitoring is stubbed for Epic 1
- Unix-only (macOS, Linux)

### Dependencies

**Runtime**
- tokio 1.49.0
- async-trait 0.1
- dashmap 5.5.3
- chrono 0.4.43
- uuid 1.21.0
- nix 0.27.1
- serde 1.0.228
- thiserror 1

**Development**
- tokio-test 0.4.5
- tracing-subscriber 0.3

## Format

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

### Sections

- **Added**: New features
- **Changed**: Changes in existing functionality
- **Deprecated**: Soon-to-be removed features
- **Removed**: Removed features
- **Fixed**: Bug fixes
- **Security**: Security vulnerability fixes
- **Known Limitations**: Known issues and workarounds
