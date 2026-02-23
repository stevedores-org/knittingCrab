# knittingCrab 🦀

A resource-aware distributed task scheduler for AI agent workloads on Apple Silicon with multi-machine support via SSH/tmux.

## Status: 67% Complete (16/24 User Stories)

**Completed Epics**:
- ✅ **Epic 2**: Worker Runtime — Full task lifecycle, leases, retries, graceful shutdown (18 tests)
- ✅ **Epic 3**: Local Mac Execution — Process runner, isolation, test sharding (4 tests)
- ✅ **Epic 4**: Cache + Artifacts — Stable keys, artifact browsing, build caching (3 tests)
- ✅ **Epic 5**: Distribution & Scaling — Network traits, distributed coordinator, persistent recovery, resilience (circuit breaker, timeouts, backpressure, priority scheduling, priority inversion detection) (20 tests)

**In Progress**:
- 🔄 **Epic 1**: Scheduler Core — DAG validation, priority + fairness, resource allocation (0/4 stories)
- 🔄 **Epic 6**: Observability + UX — Status CLI, event timelines, tunable config (0/3 stories)
- 🔄 **Epic 7**: Reliability & Soak — Soak tests, metrics validation (0/2 stories)

**Test Coverage**: 63+ tests passing with `RUSTFLAGS="-D warnings"`, zero unsafe code outside required OS interfaces

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
├── core/              # Shared types, traits, scheduling policies (Epic 1-7)
│   ├── lease.rs       # Lease state machine
│   ├── retry.rs       # Exponential backoff policy
│   ├── priority.rs    # Task priorities (Critical/High/Normal/Low)
│   ├── circuit_breaker.rs     # Resilience pattern
│   ├── task_timeout.rs        # Soft + hard timeouts
│   ├── queue_backpressure.rs  # Degradation modes (Normal/Moderate/High/Critical)
│   ├── priority_queue.rs      # Priority-aware task queue
│   ├── time_slice_scheduler.rs # Weighted round-robin (50/30/15/5)
│   ├── priority_inversion.rs  # Detection for diagnostics
│   ├── event_log.rs           # Memory + SQLite event sinks
│   ├── persistent_lease.rs    # SQLite-backed recovery
│   └── traits.rs              # Core abstractions (Queue, LeaseStore, EventSink, etc.)
├── worker/            # Worker runtime (Epic 2 complete + resilience)
│   ├── worker_runtime.rs      # Main task orchestration
│   ├── lease_manager.rs       # Lifecycle management
│   ├── process.rs             # OS process execution (macOS optimized)
│   ├── cancel_token.rs        # Graceful cancellation
│   └── fake_worker.rs         # Test double
├── scheduler/         # StubScheduler for testing
├── transport/         # Wire protocol (Epic 5)
│   ├── framing.rs     # Length-prefixed JSON + framing
│   ├── messages.rs    # CoordinatorRequest/Response types
│   └── error.rs       # Transport error handling
├── coordinator/       # Server-side scheduler state (Epic 5)
│   ├── server.rs      # TCP listener + request dispatch
│   ├── state.rs       # CoordinatorState (Arc-wrapped traits)
│   ├── node_registry.rs       # Worker node tracking + health
│   ├── cache_index.rs         # Distributed cache coordination
│   └── error.rs
└── node/              # Client-side worker integration (Epic 5)
    ├── connection.rs          # SSH/tmux session mgmt + TCP
    ├── network_queue.rs       # Queue trait over network
    ├── network_lease_store.rs # LeaseStore trait over network
    ├── network_event_sink.rs  # EventSink trait over network
    ├── network_cache.rs       # Cache discovery
    └── worker_node.rs         # Builder pattern for runtime construction
```

### Key Components

**Task Execution**:
- **WorkerRuntime**: Main async orchestration loop (dequeue → acquire lease → execute → emit events)
- **LeaseManager**: Prevents duplicate execution, handles expiry/renewal via heartbeat
- **ProcessHandle**: macOS-optimized subprocess spawning with process groups + graceful shutdown

**Resilience**:
- **CircuitBreaker**: 3-state pattern (Closed → Open → Half-Open) for fault tolerance
- **TimeoutPolicy**: Soft timeouts (graceful) + hard timeouts (forced kill) with load-aware multipliers
- **RetryHandler**: Exponential backoff (100ms → 2.0x → 30s max) with configurable codes

**Scheduling (Epic 5, Phase 1)**:
- **PriorityQueueManager**: 4-queue system respects degradation modes
- **TimeSliceScheduler**: Deterministic round-robin (50% Critical, 30% High, 15% Normal, 5% Low)
- **QueueBackpressureManager**: Adaptive degradation (Normal → Moderate → High → Critical)
- **PriorityInversionDetector**: Tracks lock contention for diagnostics

**Distribution**:
- **CoordinatorServer**: Multi-client TCP server managing global state, task distribution, lease recovery
- **NodeRegistry**: Worker tracking with stale detection (60s timeout) + automatic recovery
- **FramedTransport**: Length-prefixed JSON wire protocol (16 MiB max, robust EOF handling)
- **NetworkQueue/NetworkLeaseStore/NetworkEventSink**: Trait implementations over TCP

**Remote Execution**:
- **aivcs-session CLI** (planned): SSH/tmux session manager for `aivcs.local` (Apple Silicon studio)
  - Sanitizes repo names, generates deterministic session IDs
  - Manages tmux session lifecycle (attach-or-create)
  - Scheduler shells out to `aivcs-session attach --repo X --work Y --role Z`

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

## Remote Execution Strategy (Issue #37)

The scheduler can execute tasks on remote Apple Silicon machines (`aivcs.local`) using SSH + tmux:

### Session Management Contract
```bash
aivcs-session attach \
  --repo knittingCrab \
  --work task-12345 \
  --role runner
```

**Guarantees**:
- Deterministic session naming: `aivcs__knittingcrab__task-12345__runner`
- Idempotent: Same inputs → attach to existing session
- Safe: Sanitizes inputs, forbids `..` and `/` in repo names
- Correct directory: Auto-starts in `~/engineering/code/clone-base/$REPO_NAME`

**Roles**:
- `agent`: Long-lived (keep for debugging)
- `runner`: Disposable (auto-kill on completion)
- `human`: Manual interactive sessions

See [ARCHITECTURE.md](ARCHITECTURE.md) for implementation details.

## Roadmap

**Immediate** (Phase 2):
- Epic 1: Scheduler Core (DAG validation, priority + fairness, resource allocation)
  - Critical path: DAG cycle detection unblocks all other work
- Epic 6: Observability + UX (status CLI, event timelines, causal reasoning)
- Epic 7: Soak testing + metrics validation

**Next** (Phase 3):
- aivcs-session CLI tool (SSH/tmux session manager)
- Scheduler integration with remote workers
- Multi-machine orchestration

## License

See [LICENSE](LICENSE) file.
