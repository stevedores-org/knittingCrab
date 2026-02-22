# KnittingCrab: Distributed Task Scheduling System

A Rust-based distributed task scheduling system featuring priority queues, lease management, SSH health monitoring, and worker distribution across multiple nodes.

## Project Overview

KnittingCrab is built as a monorepo with 6 coordinated crates, implementing:
- **Coordinator**: Centralized task distribution and node management
- **Workers**: Distributed execution engines with lease-based task guarantees
- **Scheduler**: Task scheduling with weighted priority support
- **Transport**: Framed messaging protocol for inter-node communication
- **Core**: Shared domain models (tasks, priorities, leases, IDs)
- **Node**: Main entry point for worker nodes

**Total LOC**: ~69,000 lines of Rust | **Test Count**: 26+ tests per major module

---

## Quick Start

### Build
```bash
# Build entire workspace
cargo build --release

# Build specific crate
cargo build -p knitting-crab-coordinator
```

### Test
```bash
# Run all tests with strict warnings
RUSTFLAGS="-D warnings" cargo test --all

# Test specific crate (coordinator example)
cargo test -p knitting-crab-coordinator

# Test specific module
cargo test -p knitting-crab-coordinator ssh_health
cargo test -p knitting-crab-coordinator node_registry

# Run with output
cargo test --all -- --nocapture

# Run single test
cargo test -p knitting-crab-coordinator health_starts_healthy_after_register -- --exact
```

### Lint & Format
```bash
# Clippy with strict warnings
RUSTFLAGS="-D warnings" cargo clippy -p knitting-crab-coordinator -- -D warnings

# Format check
cargo fmt --all -- --check

# Auto-format
cargo fmt --all
```

---

## Project Structure

```
crates/
├── core/              # Shared types: Priority, TaskDescriptor, Lease, WorkerId, TaskId
├── coordinator/       # Central hub for task distribution & node health
├── worker/           # Distributed execution engines
├── scheduler/        # Task scheduling with priority queues
├── transport/        # Binary framing protocol (TCP/FramedTransport)
└── node/             # Worker node runtime entry
```

### Key Directories
- `crates/coordinator/src/`: Server, node registry, SSH health monitoring, state
- `crates/core/src/`: Domain models, priority system, lease management
- `crates/scheduler/src/`: Priority queue and scheduling logic
- `crates/worker/src/`: Lease manager, task execution, heartbeat protocol

---

## Test Suite Reference

### Coverage by Crate

#### `knitting-crab-coordinator` (26 tests)
Tests for distributed task coordination, node health, and cache management.

**SSH Health Monitoring** (8 tests: `ssh_health::tests::*`)
- `healthy_probe_records_no_failures` - Successful TCP probes reset failure count
- `single_failure_marks_degraded` - 1 failure → Degraded state
- `three_failures_marks_unhealthy` - 3 consecutive failures → Unhealthy state
- `success_after_failures_resets_to_healthy` - Success resets counter to 0
- `probe_all_visits_every_registered_node` - All nodes probed each interval
- `probe_all_skips_node_removed_mid_scan` - Gracefully handles mid-scan removals
- `unhealthy_node_not_returned_in_unhealthy_if_reset` - Reset removes from unhealthy list
- `concurrent_probe_all_does_not_double_count` - Race-safe concurrent probes

**Node Registry Health** (4 tests: `node_registry::tests::health_*`)
- `health_starts_healthy_after_register` - New nodes are Healthy (0 failures)
- `degraded_after_one_probe_failure` - 1+ failures trigger Degraded
- `unhealthy_after_three_probe_failures` - 3+ failures trigger Unhealthy
- `reset_to_healthy_after_probe_success` - Successful probe resets to Healthy

**Node Registry Core** (4 tests)
- `register_node` - Register adds node to registry
- `heartbeat_updates_last_seen` - Heartbeats refresh last_seen timestamp
- `stale_nodes_detection` - Nodes timeout after threshold
- `remove_node` - Removal cleans up all tracking data

**Cache Index** (3 tests)
- `announce_and_query` - Cache announce/query roundtrip
- `query_nonexistent_returns_empty` - Missing keys return empty
- `evict_node_removes_entries` - Node eviction cascades to cache

**Server & State** (7 tests)
- `register_node_returns_ok` - RegisterNode request succeeds
- `dequeue_empty_returns_none` - Empty queue returns None
- `insert_lease_succeeds` - Lease creation and duplicate detection
- `query_cache_returns_locations` - Cache query routing
- `state_creation` - State initialization
- `enqueue_and_dequeue_task` - Task lifecycle

#### `knitting-crab-core` (10 tests)
- Priority system, task descriptors, lease management
- **Doc tests**: 6 examples (ignored—for documentation)

#### `knitting-crab-scheduler` (3 tests)
- Scheduler trait implementation

#### `knitting-crab-worker` (18 tests)
- Lease manager, heartbeat protocol, worker runtime

#### `knitting-crab-transport` (1 test)
- Framed transport protocol

#### `knitting-crab-node` (0 tests)
- Integration layer (uses manual testing or acceptance tests)

**Total: 40+ unit tests**

---

## Testing Strategy

### Unit Tests (Inline `#[cfg(test)]`)
All modules have inline test submodules. Example from `ssh_health.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    struct FakeProbe { /* test impl */ }

    #[test]
    fn test_name() { /* arrange, act, assert */ }
}
```

### Test Patterns Used
1. **Dependency Injection**: `NodeProbe` trait allows `FakeProbe` for isolation
2. **Arc<DashMap>** for thread-safe, concurrent test state
3. **Tokio runtime** spawning for async test execution
4. **Assertion by state inspection**: Direct registry/queue checks post-action

### Running Tests

**By crate:**
```bash
cargo test -p knitting-crab-coordinator
cargo test -p knitting-crab-worker
cargo test -p knitting-crab-core
```

**By module:**
```bash
cargo test -p knitting-crab-coordinator ssh_health --lib
cargo test -p knitting-crab-coordinator node_registry --lib
```

**By test name pattern:**
```bash
cargo test health_              # Matches all health_* tests
cargo test probe_all            # Matches all probe_all* tests
```

**Strict mode (fail on warnings):**
```bash
RUSTFLAGS="-D warnings" cargo test --all
```

---

## Key Architectures

### SSH Health Monitoring (`coordinator/src/ssh_health.rs`)
Active TCP probe monitor runs every 30s per node.
- **NodeHealth enum**: Healthy → Degraded (1 failure) → Unhealthy (3 failures)
- **Probe success resets** counter to 0 (fast recovery)
- **Unhealthy nodes** auto-evicted, leases requeued via `node_reaper_loop`
- **Trait-based**: `NodeProbe` for production TCP vs. test fakes

### Priority System (`core/src/priority.rs`)
Four-tier priority: Critical (3), High (2), Normal (1), Low (0)
- Supports **priority inversion detection** and degradation modes
- **Weighted round-robin** scheduling (50/30/15/5 distribution)

### Lease Management (`worker/src/lease_manager.rs`)
Task execution guarantees via distributed leases:
- **Lease acquire** on task start
- **Periodic heartbeat** extends lease TTL
- **On timeout**: task requeued with incremented attempt counter
- **Coordinator tracking**: all active leases visible for eviction logic

### Node Registry (`coordinator/src/node_registry.rs`)
Tracks registered worker nodes with dual timeout mechanisms:
1. **Passive**: `heartbeat()` updates `last_seen`; stale timeout after 60s
2. **Active**: `SshHealthMonitor` TCP probes every 30s; 3 failures = unhealthy

---

## Workflow Commands

### Development Loop
```bash
# 1. Make changes to a module (e.g., ssh_health.rs)
# 2. Run module tests to validate
cargo test -p knitting-crab-coordinator ssh_health -- --nocapture

# 3. Run full crate tests
cargo test -p knitting-crab-coordinator --lib

# 4. Lint & format
cargo fmt --all
RUSTFLAGS="-D warnings" cargo clippy -p knitting-crab-coordinator -- -D warnings

# 5. Full workspace test
RUSTFLAGS="-D warnings" cargo test --all
```

### Adding a New Test
```rust
// In the module's #[cfg(test)] section:
#[test]
fn new_test_name() {
    // Arrange
    let registry = NodeRegistry::new();

    // Act
    registry.record_probe_failure(&id);

    // Assert
    assert_eq!(registry.health_status(&id), NodeHealth::Degraded);
}
```

### Creating a New Module with Tests
1. Create `src/new_module.rs`
2. Add module declaration to `lib.rs`: `pub mod new_module`
3. Add module export if public API needed
4. Write inline `#[cfg(test)] mod tests { ... }`
5. Run: `cargo test -p <crate> new_module --lib`

---

## Git Workflow

### Branch Naming
- **Feature**: `feature/short-description`
- **Epic**: `epic/epic-number-short-description`
- **Fix**: `fix/issue-number-short-description`
- **Docs**: `docs/topic`

### Pull Request Checklist
- [ ] All tests pass: `cargo test --all`
- [ ] No clippy warnings: `RUSTFLAGS="-D warnings" cargo clippy -p <crate> -- -D warnings`
- [ ] Code formatted: `cargo fmt --all`
- [ ] PR description includes: what, why, testing strategy
- [ ] Commits include: `Co-Authored-By: Claude Haiku 4.5 <noreply@anthropic.com>`

### Base Branch
**Always create PRs against `develop` branch** (not main)
```bash
git checkout develop
git pull origin develop
git checkout -b feature/your-feature
# ... make changes ...
git push -u origin feature/your-feature
# Create PR: base branch = develop
```

---

## Common Tasks

### Debug a Failing Test
```bash
# Run test with backtrace
RUST_BACKTRACE=1 cargo test -p knitting-crab-coordinator health_starts_healthy -- --nocapture

# Run test with debug output
RUST_LOG=debug cargo test -p knitting-crab-coordinator -- --nocapture
```

### Profile Memory/Performance
```bash
# Build release profile
cargo build --release -p knitting-crab-coordinator

# Run with perf (macOS: use Instruments)
cargo build --release
time ./target/release/coordinator
```

### Update Dependencies
```bash
# Check for outdated deps
cargo outdated

# Update workspace deps (use workspace Cargo.toml)
# Edit Cargo.toml [workspace.dependencies] section
cargo update
```

### Clean Build
```bash
# Remove build artifacts
cargo clean

# Rebuild
cargo build --all
```

---

## Troubleshooting

### `error[E0432]: unresolved import`
- Add missing crate to `Cargo.toml` dependencies
- Ensure workspace dependencies are declared in workspace root

### Test fails with `panic: ...`
- Check test assertion failure message
- Run test with `--nocapture` to see println! output
- Use `RUST_BACKTRACE=1` for full stack trace

### Clippy warnings preventing build
- Run: `RUSTFLAGS="-D warnings" cargo clippy -p <crate> -- -D warnings`
- Fix indicated warnings or refactor code
- Never use `#![allow(warnings)]` unless explicitly justified

### Workspace test count mismatch
- Some tests may be ignored (e.g., doc-tests)
- Run: `cargo test --all -- --include-ignored` to run all
- Check `#[ignore]` attributes in test code

---

## Resources

- **Rust Documentation**: https://doc.rust-lang.org/
- **Tokio Async Runtime**: https://tokio.rs/
- **DashMap Concurrent HashMap**: https://docs.rs/dashmap/
- **Thiserror Error Handling**: https://docs.rs/thiserror/
- **Clippy Lints**: https://doc.rust-lang.org/clippy/

---

## License & Attribution

KnittingCrab is developed as an educational project demonstrating distributed systems concepts in Rust.

All tests use standard Rust testing conventions and are verified with:
- `cargo test` (unit tests)
- `cargo clippy` (linting)
- `cargo fmt` (formatting)
