# CLAUDE.md: knittingCrab Project Guide

> This guide helps Claude Code (and human contributors) understand knittingCrab's architecture, conventions, and development workflow. **Reference this before making changes.**

## 🔧 CRITICAL: Environment Setup

**Always use nix-shell before development:**
```bash
nix-shell --option binary-caches "https://nix-cache.stevedores.org/"
```

All development work requires this isolated environment. See `memory/ENVIRONMENT_SETUP.md` for details.

---

## Project Overview

**knittingCrab** is a resource-aware local task scheduler for AI agent workloads, optimized for macOS M4 Max. It transforms chaotic agent swarms into deterministic "factory lines" by providing:

- Priority-based task queuing
- Resource-aware allocation (CPU, RAM, Metal slots)
- Lease-based concurrency control
- Graceful process management with signal handling
- Remote session execution via aivcs.local

**Why "knittingCrab"?** The scheduler orchestrates many concurrent threads of work (like knitting) with crustacean-style lateral thinking—sideways approaches to deadlock and resource contention.

---

## Core Philosophy

### 1. **TDD-First Development**
Every feature **must** have tests written before implementation:
- Unit tests verify isolated logic (e.g., lease state machine)
- Component tests verify trait contracts (e.g., ProcessExecutor)
- Integration tests verify real-world behavior (e.g., actual subprocess spawning)
- Tests are deterministic and never flaky (no sleep-based timing)

### 2. **Zero Unsafe Code Policy** (Except Where Necessary)
- No unsafe code in task logic, queuing, or trait implementations
- Process group setup (`setpgid`) requires unsafe; document with `// SAFETY: ...` comments
- Every unsafe block requires justification and is reviewed carefully

### 3. **No Warnings Policy**
- All code compiles with `RUSTFLAGS="-D warnings"`
- All code passes `cargo clippy -- -D warnings`
- No suppression of warnings except with explicit justification

### 4. **Trait-Based Abstraction**
- Concrete implementations are swappable (e.g., FakeWorker for RealProcessExecutor)
- Every external dependency is hidden behind a trait boundary
- This enables testing without side effects (no network calls, no disk I/O in unit tests)

### 5. **Resource Determinism**
- Same inputs → same outputs (no randomness except where explicitly needed)
- Idempotent operations where possible (e.g., lease attach-or-create semantics)
- Session names generated deterministically: `aivcs__{repo}__{work_id}__{role}`

---

## Workspace Structure

```
knittingCrab/
├── CLAUDE.md                          # ← You are here
├── README.md                          # Project overview & quick start
├── IMPLEMENTATION.md                  # Architecture deep-dive (Epic 2 detail)
├── CHANGELOG.md                       # Release notes & breaking changes
├── CI.md                             # CI/CD configuration & workflows
├── Cargo.toml                        # Workspace definition
├── Cargo.lock                        # Pinned dependencies
│
├── .github/
│   └── workflows/
│       └── ci.yml                    # GitHub Actions: build, test, clippy, security audit, Apple Silicon
│
├── .pre-commit-config.yaml           # Pre-commit hooks: rustfmt, clippy
├── .githooks/                        # Local git hooks
│
├── crates/
│   ├── core/                         # Shared types, traits, error types (Epic 1 boundary)
│   │   ├── src/
│   │   │   ├── lib.rs               # Public exports
│   │   │   ├── error.rs             # CoreError types
│   │   │   ├── event.rs             # TaskEvent, LogLine
│   │   │   ├── lease.rs             # Lease state machine
│   │   │   ├── traits.rs            # Queue, LeaseStore, GoalLockStore, EventSink
│   │   │   ├── agent.rs             # AgentBudget, TestGate (Epic 5)
│   │   │   └── ...                  # Circuit breaker, backpressure, timeout
│   │   └── tests/                   # 141 unit tests
│   │
│   ├── worker/                       # Worker runtime (Epic 2 complete)
│   │   ├── src/
│   │   │   ├── lib.rs               # Public exports
│   │   │   ├── process.rs           # ProcessHandle, signal handling
│   │   │   ├── worker_runtime.rs    # Main task orchestration
│   │   │   ├── fake_worker.rs       # Test double (no OS processes)
│   │   │   ├── cancel_token.rs      # Graceful cancellation
│   │   │   └── retry.rs             # Exponential backoff
│   │   └── tests/                   # 30 component & integration tests
│   │
│   ├── scheduler/                    # Task queue stub (for testing)
│   │   ├── src/
│   │   │   └── lib.rs               # StubScheduler
│   │   └── tests/
│   │
│   ├── agent/                        # Agent workloads (Epic 5 new)
│   │   ├── src/
│   │   │   ├── lib.rs               # AgentPlan (4 tests)
│   │   │   ├── goal_lock.rs         # InMemoryGoalLockStore (5 tests)
│   │   │   ├── budget.rs            # BudgetTracker (5 tests)
│   │   │   └── test_gate.rs         # TestGateRunner (5 tests)
│   │   └── 19 total tests
│   │
│   ├── transport/                    # Message framing protocol (Epic 5 new)
│   │   ├── src/
│   │   │   ├── lib.rs               # FramedTransport
│   │   │   └── messages.rs          # Request/Response types
│   │   └── 10 tests
│   │
│   ├── coordinator/                  # Server-side state (Epic 5 new)
│   │   ├── src/
│   │   │   ├── lib.rs               # Public API
│   │   │   ├── state.rs             # CoordinatorState, NodeRegistry
│   │   │   └── server.rs            # CoordinatorServer, handlers
│   │   └── tests/
│   │
│   ├── node/                         # Client-side networking (Epic 5 new)
│   │   ├── src/
│   │   │   ├── lib.rs               # Public API
│   │   │   ├── worker_node.rs       # WorkerNode builder
│   │   │   └── network_*.rs         # Network trait impls
│   │   └── tests/
│   │
│   └── aivcs-session/                # Remote session manager (Phase 2 new)
│       ├── src/
│       │   ├── lib.rs               # Public API
│       │   ├── session.rs           # SessionConfig, deterministic naming
│       │   ├── remote.rs            # RemoteSessionManager, SSH/tmux
│       │   ├── main.rs              # CLI (attach, list, kill)
│       │   └── error.rs             # SessionError
│       └── 14 tests
│
├── src/                              # (Old location, being migrated to crates/)
│   └── ...                           # Legacy code; prefer crates/ structure
│
└── tests/                            # Integration tests
    └── ...
```

---

## Key Architecture Patterns

### Lease System (Prevents Duplicate Execution)
```
WorkItem → [Queued] → (dequeue) → [Scheduled] → (acquire lease) → [Running]
                          ↑                                              ↓
                          ←─── (retry) ←─ (fail) ←─ (release lease) ←─
```

**Key invariant**: A task can only execute if:
1. It's dequeued
2. Its dependencies are satisfied
3. It acquires a unique lease (no other worker holds it)

**Implementation**: `LeaseManager` holds a `DashMap<task_id, Lease>` with heartbeat renewal.

### Process Execution Flow
```rust
RealProcessExecutor::execute(cmd) {
    1. Create process group: setpgid(0, 0)       // Allows killpg()
    2. Spawn child process
    3. Drive with select! {
        - Process completion (wait_on_child)
        - Cancellation signal (recv_cancel_signal)
    }
    4. On signal: send SIGTERM, wait grace period, SIGKILL if needed
    5. Capture exit code, stdout, stderr
}
```

### Trait-Based Testing (No Side Effects)
```rust
// Production code
impl WorkerRuntime {
    fn new(queue: Arc<dyn Queue>, executor: Arc<dyn ProcessExecutor>) -> Self { ... }
}

// Test code
let fake_queue = Arc::new(FakeQueue::with_tasks(vec![...]));
let fake_executor = Arc::new(FakeProcessExecutor::success(0));
let runtime = WorkerRuntime::new(fake_queue, fake_executor);
runtime.run().await;  // No OS processes, no network calls
```

### Deterministic Session Naming
```
Input:  repo_name="my-repo", work_id="task-123", role=Agent
Output: "aivcs__my-repo__task-123__agent"

Rules:
- lowercase everything
- replace invalid chars with '_'
- forbid ".." and "/" (path traversal prevention)
- escape shell metacharacters before SSH
```

---

## Development Workflow

### Before You Code: Check the Plan
1. **Is there a related GitHub issue?** Read it fully.
2. **Is there an open PR?** Check its status and avoid duplicating work.
3. **Have you reviewed the relevant docs?** (README.md, IMPLEMENTATION.md, CLAUDE.md)
4. **Do you understand the test strategy?** (See "Testing Requirements" below)

### Creating a Feature
1. **Create a test first** (TDD):
   ```rust
   #[test]
   fn my_feature_works() {
       // Arrange
       // Act
       // Assert
   }
   ```

2. **Run the test** to verify it fails:
   ```bash
   cargo test my_feature --lib
   ```

3. **Implement the feature** (minimal code to pass the test)

4. **Add more tests** for edge cases:
   - Happy path
   - Error conditions
   - Boundary conditions
   - Integration (if trait-based)

5. **Verify no warnings**:
   ```bash
   RUSTFLAGS="-D warnings" cargo test --all
   cargo clippy --all -- -D warnings
   cargo fmt --all
   ```

6. **Run pre-commit hooks**:
   ```bash
   pre-commit run --all-files
   ```

7. **Create a PR** with:
   - Clear title describing the feature
   - Reference to related issue (#37, #38, etc.)
   - Summary of what changed and why
   - Test counts (e.g., "25 tests passing")

### Making a Bug Fix
1. **Write a test that reproduces the bug** (fails before fix)
2. **Fix the bug** (minimal change)
3. **Verify the test now passes**
4. **Check no other tests broke**:
   ```bash
   cargo test --all
   ```

### Refactoring
1. **Ensure all tests pass** before refactoring:
   ```bash
   cargo test --all
   ```

2. **Refactor incrementally** (small, focused changes)

3. **Run tests after each change**:
   ```bash
   cargo test --all
   ```

4. **If a test breaks**, either:
   - Fix your refactor to preserve behavior, OR
   - Update the test if the behavior change is intentional
   - Document why in the commit message

---

## Testing Requirements

### What Must Be Tested?

| Category | Examples | Min Tests |
|----------|----------|-----------|
| **State machines** | Lease lifecycle, task state transitions | 5+ |
| **Retry logic** | Backoff, max attempts, code filtering | 3+ |
| **Traits** | Queue, ProcessExecutor, LeaseStore | 3+ per trait |
| **Edge cases** | Empty input, max size, timeout, cancellation | 5+ |
| **Error conditions** | Path traversal, shell injection, invalid role | 5+ |

### Test Naming Convention

```rust
// ✅ Good
#[test]
fn lease_prevents_duplicate_execution() { ... }

#[test]
fn session_name_forbids_path_traversal() { ... }

// ❌ Bad
#[test]
fn test_1() { ... }

#[test]
fn it_works() { ... }
```

### Test Organization

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;

    // Group related tests with comments
    // ===== Lease Lifecycle Tests =====

    #[test]
    fn acquire_prevents_duplicate() { ... }

    #[test]
    fn release_frees_slot() { ... }

    // ===== Retry Logic Tests =====

    #[test]
    fn backoff_increases_exponentially() { ... }
}
```

### Running Tests

```bash
# All tests
cargo test --all

# Specific crate
cargo test -p knitting-crab-worker

# Specific test
cargo test my_feature --lib

# With verbose output
cargo test --all -- --nocapture

# With backtrace on panic
RUST_BACKTRACE=1 cargo test --all

# Strict: no warnings allowed
RUSTFLAGS="-D warnings" cargo test --all
```

---

## Code Quality Standards

### Clippy
```bash
# Check for warnings
cargo clippy --all -- -D warnings

# Fix automatically (when possible)
cargo clippy --fix --all
```

### Formatting
```bash
# Check format
cargo fmt --all -- --check

# Auto-format
cargo fmt --all
```

### Pre-Commit Hooks
```bash
# Install (one-time)
pip install pre-commit
pre-commit install

# Run manually
pre-commit run --all-files
```

### Security Audit
```bash
cargo audit
```

---

## Design Decisions & Rationale

### Why DashMap for Leases?
- **Lock-free concurrent hashmap** (better than `tokio::sync::Mutex`)
- Allows multiple readers without blocking async tasks
- O(1) lookup and insertion
- Reference: `crates/core/src/lease.rs`

### Why Process Groups (setpgid)?
- Creates isolated process groups so `killpg()` reaches all descendants
- Required on macOS for reliable signal delivery
- Prevents child processes from escaping termination
- Reference: `crates/worker/src/process.rs`

### Why FakeWorker Test Double?
- Enables integration testing without spawning OS processes
- No flaky timing issues (no sleep, deterministic)
- Configurable task behaviors (Succeed, Fail, Hang, Crash)
- Reference: `crates/worker/src/fake_worker.rs`

### Why Trait-Based Architecture?
- Swappable implementations (Real vs Fake)
- Testable without side effects (no network, disk I/O in unit tests)
- Future-proof for distributed execution (HTTP backends, database backends)
- Reference: `crates/core/src/lib.rs` (trait exports)

### Why Deterministic Session Names?
- Same inputs always produce same tmux session
- Enables idempotent attach-or-create semantics
- Prevents session name collisions
- Security: forbids path traversal and shell injection
- Reference: `crates/aivcs-session/src/session.rs`

### Why GoalLockStore Trait?
- Prevents duplicate agent execution on same goal
- DashMap-backed in-memory implementation (fast, lock-free)
- Enables future implementations (Redis, database, cluster-aware)
- Trait allows testing with FakeGoalLockStore
- Reference: `crates/core/src/traits.rs` (definition), `crates/agent/src/goal_lock.rs` (implementation)

---

## Common Tasks

### Add a New Test
```rust
#[test]
fn my_test_name() {
    // Arrange
    let input = ...;

    // Act
    let result = function_under_test(input);

    // Assert
    assert_eq!(result, expected_value);
}
```

### Add a New Trait
1. Define in `crates/core/src/traits.rs` (not lib.rs)
2. Export from `crates/core/src/lib.rs`
3. Add a FakeImpl for testing (e.g., FakeGoalLockStore)
4. Add at least 3 unit tests for trait contract
5. Create a component test using real + fake implementations
6. Example: `GoalLockStore` trait (crates/core/src/traits.rs) with `InMemoryGoalLockStore` impl (crates/agent/src/goal_lock.rs)

### Add a New Crate
```bash
cd crates
cargo new my-feature
```

Then update `Cargo.toml` in root:
```toml
[workspace]
members = [
    "crates/core",
    "crates/worker",
    "crates/scheduler",
    "crates/agent",           # Epic 5
    "crates/transport",       # Epic 5
    "crates/coordinator",     # Epic 5
    "crates/node",            # Epic 5
    "crates/aivcs-session",
    "crates/my-feature",      # Add new crate here
]
```

### Fix a Clippy Warning
```bash
# Identify the warning
cargo clippy --all -- -D warnings

# Let clippy try to fix it
cargo clippy --fix --all

# Manually verify and commit
git diff
cargo test --all
```

### Update Documentation
- **README.md**: Quick start, project overview, status
- **IMPLEMENTATION.md**: Architecture deep-dive, design decisions
- **CLAUDE.md**: This file (development guide)
- **Inline docs**: Doc comments for all public APIs
  ```rust
  /// Acquires an exclusive lease for a task.
  ///
  /// # Returns
  /// - `Ok(Lease)` if the lease was acquired
  /// - `Err(LeaseError::Conflict)` if already held
  pub fn acquire(&self, task_id: u64) -> Result<Lease, LeaseError> { ... }
  ```

---

## Known Issues & Limitations

### Current Limitations
1. **InMemoryLeaseStore**: Not persistent (lost on restart)
2. **StubScheduler**: Simple VecDeque (no priority, no persistence)
3. **Log sequence tracking**: Resets per task (not global)
4. **Process groups**: Unix-only (guarded with `#[cfg(unix)]`)
5. **Resource monitoring**: Stubbed for Epic 1 boundary
6. **Apple Silicon CI**: Occasional transient timing flakes in `test_half_open_transition`

### Known Flakes
- **Apple Silicon Compatibility check**: Sometimes fails with `test_half_open_transition` (circuit breaker timing)
  - **Workaround**: Push an empty commit to rerun CI
  - **Root cause**: Timing-sensitive test on CI macOS (not in PR code)
  - **Status**: Being investigated; does not block merges

---

## Git & GitHub Workflow

### Creating a PR
1. **Branch naming**: `epic<N>/<description>` or `feature/<description>` or `bugfix/description`
2. **Base branch**: `develop` (for new features and epics), `main` (for hotfixes only)
3. **Commit messages**: Clear and descriptive
   ```
   feat: Add deterministic session naming for aivcs-session

   Implements SessionConfig with validation and sanitization.
   - forbids path traversal (.., /)
   - replaces invalid chars with underscore
   - enforces lowercase naming
   - deterministic: same inputs → same output

   Adds 22 unit tests covering all edge cases.
   ```

4. **PR description**:
   ```markdown
   ## Summary
   Brief description of what changed.

   ## Related Issue
   Resolves #37

   ## Test Results
   - 25 tests passing
   - Zero clippy warnings
   - All pre-commit hooks pass

   ## Architecture Alignment
   This implementation satisfies the requirements from #37/#38/#40.
   ```

### Merging a PR
1. Ensure **all CI checks pass** (build, test, clippy, security audit, Apple Silicon)
2. Get **at least 1 approval** from a maintainer
3. **Squash or rebase** as appropriate
4. **Delete the branch** after merge

---

## Performance Considerations

| Component | Complexity | Notes |
|-----------|-----------|-------|
| Lease lookup | O(1) | DashMap hash lookup |
| Task dequeue | O(1) | VecDeque pop front (for now) |
| Task acquire | O(1) | DashMap insert |
| Process spawn | O(1) | Command::new + spawn |
| Heartbeat | O(n) | Where n = active leases (every 5s) |
| Reaper | O(n) | Where n = expired leases (every 10s) |

### Optimization Opportunities
- Replace VecDeque with priority queue (for Epic 1)
- Persistent lease store (database-backed, for production)
- Event sink batching (reduce allocations)

---

## Debugging Tips

### Enable Backtrace
```bash
RUST_BACKTRACE=1 cargo test --all
RUST_BACKTRACE=full cargo test --all
```

### Print Debug Info
```rust
// Use dbg! for quick debugging
let x = dbg!(expensive_function());

// Use eprintln! for logging
eprintln!("Task state: {:?}", task.state);
```

### Run a Single Test with Output
```bash
cargo test my_test -- --nocapture --exact
```

### Check Test Code Structure
```bash
# List all test names
cargo test --all -- --list
```

---

## Resources & References

- **[Tokio Runtime Documentation](https://tokio.rs/)**: Async/await patterns
- **[Signal Handling on Unix](https://www.man7.org/linux/man-pages/man2/signal.2.html)**: setpgid, SIGTERM, SIGKILL
- **[macOS Process Management](https://developer.apple.com/documentation/os/process_management)**: M4 Max specifics
- **[Rust Testing Guide](https://doc.rust-lang.org/book/ch11-00-testing.html)**: Test organization & conventions
- **[Interior Mutability Patterns](https://doc.rust-lang.org/reference/interior-mutability.html)**: Arc, RwLock, DashMap

---

## Contributing

We welcome contributions! Please:

1. **Read this guide first** ← You are here
2. **Check open issues** for good first tasks
3. **Write tests first** (TDD)
4. **Ensure no clippy warnings**
5. **Run pre-commit hooks**
6. **Create a PR** with clear description
7. **Respond to feedback** promptly

---

## Questions?

- **Architecture questions**: See IMPLEMENTATION.md
- **Specific crate questions**: Check inline documentation in `crates/*/src/lib.rs`
- **Git/GitHub questions**: Ask in the PR or issue
- **General questions**: Open a GitHub issue

---

**Last updated**: 2026-02-22 | **Reviewer**: Claude Code TDD-first verification
