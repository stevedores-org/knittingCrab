# Epic 2: Worker Runtime Implementation Guide

## Overview

This document describes the implementation of Epic 2 (Worker Runtime) for knittingCrab. The worker runtime is responsible for executing tasks, managing their lifecycle, handling retries, and gracefully shutting down processes.

## Architecture

### Core Components

```
WorkerRuntime
├── Queue (trait) → StubScheduler (implementation)
├── LeaseStore (trait) → InMemoryLeaseStore (implementation)
│   └── LeaseManager (wraps LeaseStore)
├── ResourceMonitor (trait) → stub for Epic 1
├── EventSink (trait) → stub for Epic 1
└── ProcessExecutor (trait)
    ├── RealProcessExecutor (production)
    └── FakeWorker (testing)
```

### Key Design Decisions

1. **DashMap for Leases**: Uses lock-free concurrent hashmap instead of `tokio::sync::Mutex` to avoid blocking across `.await` points
2. **Process Groups**: Uses `setpgid(0,0)` in `pre_exec` to create isolated process groups for reliable signal delivery
3. **Graceful Shutdown**: SIGTERM → grace period → SIGKILL ensures clean shutdown with fallback
4. **Pure RetryHandler**: Stateless decision logic makes testing easy and prevents subtle state bugs
5. **Trait-Based Testing**: FakeWorker implements all traits without spawning OS processes

## Lease Lifecycle

```
┌─────────────────────────────────────────────────────┐
│                                                       │
│  [Queued]                                            │
│    │                                                  │
│    ├──→ [Active] (lease acquired)                   │
│    │      │                                          │
│    │      ├─(success)──→ [Completed] (done)         │
│    │      │                                          │
│    │      ├─(retryable)─→ [Failed] ──→ requeue     │
│    │      │                                          │
│    │      └─(expired)──→ [Expired] ──→ requeue     │
│    │                                                  │
│    └─(cancel)──→ [Cancelled] (discarded)           │
│                                                       │
└─────────────────────────────────────────────────────┘
```

## Retry Logic

Exponential backoff with configurable parameters:
- **Initial backoff**: 100ms
- **Multiplier**: 2.0x per attempt
- **Max backoff**: 30s
- **Max attempts**: 3 (default)
- **Retryable codes**: Configurable (empty = all codes retryable)

Example progression: 100ms → 200ms → 400ms → abandon

## Task Execution Flow

```
1. Dequeue task from Queue
2. Acquire lease (exclusive, prevents duplicate execution)
3. Emit TaskEvent::Acquired
4. Spawn heartbeat task (renews lease every 5s)
5. Spawn process via ProcessExecutor
6. Drive process with select! {
     - Process completion
     - Cancellation signal
   }
7. Receive exit outcome
8. Apply retry decision:
   - Success → mark complete, stop
   - Retryable failure → mark failed, delay, requeue
   - Non-retryable → mark abandoned, stop
9. Stop heartbeat task
10. Emit completion event
```

## Testing Strategy

### Unit Tests (13 tests)
- **Leases** (3): duplicate prevention, expiry, heartbeat renewal
- **Retries** (6): backoff progression, max attempts, code filtering
- **Cancellation** (4): token basics, waiting, idempotency

### Component Tests (3 tests)
- Log streaming: ordered, lossless sequence tracking
- Process spawning: real subprocess (echo "hello world")
- Runtime basics: task dequeuing, runtime creation

### Test Double: FakeWorker
- Implements all traits (Queue, EventSink, LeaseStore, etc.)
- Configurable task behaviors (Succeed, Fail, Hang, Crash)
- No OS process spawning
- Perfect for integration testing

## macOS M4 Max Specific Implementation

### Process Groups
```rust
unsafe {
    cmd.pre_exec(|| {
        nix::unistd::setpgid(Pid::from_raw(0), Pid::from_raw(0))
            .map_err(std::io::Error::from)
    });
}
```
- Creates child in its own process group
- Allows `killpg()` to reach all descendants

### Signal Handling
```rust
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;

// Kill entire process group
kill(Pid::from_raw(-(pid as i32)), Signal::SIGTERM)
```
- SIGTERM asynchronously delivered on macOS
- Always use `timeout()` before SIGKILL
- Negative PID sends signal to process group

### Async Process Management
```rust
tokio::task::spawn_blocking(|| {
    let mut c = child.lock().unwrap();
    c.wait()
})
```
- Blocks on std::process::Child::wait() without blocking tokio runtime
- `std::process::Child` lacks async methods; spawn_blocking needed

## Performance Considerations

- **Lease lookup**: O(1) via DashMap
- **Concurrent leases**: No lock contention (lock-free)
- **Task spawning**: Per-task two async log-reader tasks
- **Heartbeat**: Configurable interval (default 5s)
- **Reaper**: Configurable interval (default 10s)

## Known Limitations

1. **InMemoryLeaseStore**: Not persistent; lost on restart
2. **StubScheduler**: Simple VecDeque; no priority/persistence
3. **Log sequence tracking**: Resets per task (not global)
4. **Process groups**: Unix-only (guarded with `#[cfg(unix)]`)
5. **Resource monitoring**: Stubbed for Epic 1 boundary

## Future Work (Epic 3+)

- Persistent lease store (database-backed)
- Priority queue scheduler
- Resource allocation enforcement
- Global event sink (logging, metrics)
- Worker pool orchestration
- Graceful shutdown of runtime
- Task timeout enforcement
- Circuit breaker for cascading failures

## Testing Checklist

Before merging, verify:
- [ ] All 36 tests passing: `cargo test --all`
- [ ] No warnings: `RUSTFLAGS="-D warnings" cargo test --all`
- [ ] Clippy clean: `cargo clippy --all -- -D warnings`
- [ ] Format clean: `cargo fmt --all -- --check`
- [ ] Pre-commit hooks pass: `pre-commit run --all-files`
- [ ] Real subprocess test passes: `cargo test spawn_real_subprocess -- --nocapture`

## Code Walkthrough

### Key Files

1. **`crates/core/src/lease.rs`**: Lease state machine
2. **`crates/worker/src/lease_manager.rs`**: Lease lifecycle management
3. **`crates/worker/src/process.rs`**: OS process spawning and lifecycle
4. **`crates/worker/src/worker_runtime.rs`**: Main orchestration loop
5. **`crates/worker/src/fake_worker.rs`**: Test double
6. **`crates/worker/src/cancel_token.rs`**: Cancellation mechanism

### Notable Patterns

- **Trait objects**: `Arc<dyn EventSink>` for abstraction
- **Generic constraints**: `LS: LeaseStore + Clone` for task spawning
- **Async closure captures**: Move blocks with Arc for shared state
- **Select! patterns**: Concurrent process + cancellation
- **Enumerate with cast**: `(seq, line) in reader.lines().enumerate(); seq as u64`
