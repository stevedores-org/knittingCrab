# knittingCrab Architecture & Strategy

This document describes the complete architecture of knittingCrab, including the scheduler core, distributed execution, and remote session management strategy.

**Status**: 67% complete (16/24 user stories). Epics 2–5 complete; Epics 1, 6, 7 in design/planning phase.

---

## Table of Contents

1. [System Overview](#system-overview)
2. [Core Architecture](#core-architecture)
3. [Distributed Execution (Epics 2–5)](#distributed-execution-epics-2-5)
4. [Remote Session Management (Issue #37)](#remote-session-management-issue-37)
5. [Scheduler Core (Epic 1)](#scheduler-core-epic-1)
6. [Observability (Epic 6)](#observability-epic-6)
7. [Testing & Reliability (Epic 7)](#testing--reliability-epic-7)
8. [Design Decisions](#design-decisions)

---

## System Overview

```
┌────────────────────────────────────────────────────────────────┐
│                      Client/Orchestrator                       │
│  (Local machine or CI/CD pipeline)                             │
└─────────────┬──────────────────────────────────────────────────┘
              │
              ├─ enqueue(TaskDescriptor) → CoordinatorServer
              │  (TCP to coordinator.local:8080)
              │
              ├─ monitor(task_id) → event stream
              │
              └─ explain(task_id) → "blocked by: deps/resource/backpressure"

┌─────────────┴──────────────────────────────────────────────────┐
│                    CoordinatorServer (TCP)                      │
│  (Global state: queue, leases, cache index, node registry)    │
└─────────────┬──────────────────────────────────────────────────┘
              │
      ┌───────┴───────┬───────────┬─────────────┐
      │               │           │             │
      ▼               ▼           ▼             ▼
┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐
│ Worker 1 │  │ Worker 2 │  │ Worker N │  │ aivcs    │
│ (local)  │  │ (local)  │  │ (local)  │  │(remote)  │
│          │  │          │  │          │  │ SSH+tmux │
└────┬─────┘  └────┬─────┘  └────┬─────┘  └──────────┘
     │             │             │
     │ WorkerRuntime (NetworkQueue, NetworkLeaseStore, ...)
     │
     ├─ ProcessHandle → OS subprocess
     ├─ LeaseManager → heartbeat/renewal
     ├─ RetryHandler → exponential backoff
     └─ EventSink → event stream
```

---

## Core Architecture

### 1. Trait-Based Abstraction (Epics 2–5 Complete)

The system is built around five core traits, all implementing network versions in Epic 5:

```rust
#[async_trait]
pub trait Queue: Send + Sync + 'static {
    async fn dequeue(&self, worker_id: WorkerId) -> Result<Option<TaskDescriptor>, CoreError>;
    async fn requeue(&self, task_id: TaskId, attempt: u32) -> Result<(), CoreError>;
    async fn discard(&self, task_id: TaskId) -> Result<(), CoreError>;
}

#[async_trait]
pub trait LeaseStore: Send + Sync + 'static {
    async fn insert(&self, lease: Lease) -> Result<(), CoreError>;
    async fn get(&self, task_id: TaskId) -> Result<Option<Lease>, CoreError>;
    async fn update(&self, lease: Lease) -> Result<(), CoreError>;
    async fn remove(&self, task_id: TaskId) -> Result<(), CoreError>;
    async fn active_leases(&self) -> Result<Vec<Lease>, CoreError>;
}

#[async_trait]
pub trait EventSink: Send + Sync + 'static {
    async fn emit_event(&self, event: TaskEvent) -> Result<(), CoreError>;
    async fn emit_log(&self, log: LogLine) -> Result<(), CoreError>;
}
```

**Why traits**:
- **Testability**: FakeWorker doubles without OS/network
- **Distribution**: NetworkQueue/NetworkLeaseStore implement same contract over TCP
- **Extensibility**: Can swap in Redis, cloud queues, databases later

### 2. Task Lifecycle (Epic 2: Worker Runtime)

```
enqueue(task)
    ↓
[Queued] ← requeue (retry)
    ↓
dequeue(worker_id) → NetworkQueue
    ↓
acquire_lease(task_id) → NetworkLeaseStore
    ↓
[Leased] ← heartbeat every 5s (renew)
    ↓
spawn process (ProcessHandle)
    ↓
[Running] ← SIGTERM (graceful, 5s grace period)
    ↓
process exits (OK or error)
    ↓
apply RetryHandler policy
    ├→ Success → [Completed]
    ├→ Retryable (exit code 1, 2) → [Failed] + delay + requeue
    └→ Non-retryable → [Abandoned]
    ↓
emit_event(TaskEvent::Completed/Failed/Abandoned)
```

### 3. Resilience (Epic 5: Phase 1)

Added in Epic 5:

**CircuitBreaker** (3-state):
- **Closed** (normal): Requests pass through
- **Open** (failing): Requests rejected immediately
- **Half-Open** (recovery): Limited requests allowed to test recovery
- Configurable: `failure_threshold=5`, `timeout=60s`, `half_open_max_calls=3`, `success_threshold=67%`

**Timeouts** (soft + hard):
- **Soft timeout**: Sends cancellation signal (SIGTERM), gives grace period
- **Hard timeout**: Forced kill (SIGKILL) if soft times out
- Configurable: Default hard=5min, optional soft=grace period
- Load-aware: Multiplier scales with backpressure (1.0x normal → 5.0x critical)

**Backpressure & Degradation** (4 modes):
- **Normal** (0–50% queue): All tasks accepted
- **Moderate** (50–80%): Info for scheduler, timeout 1.5x
- **High** (80–95%): Single-threaded, timeout 2.5x
- **Critical** (95%+): Non-critical tasks rejected, timeout 5.0x

**Priority Inversion Detection**:
- Tracks when high-priority task is blocked by low-priority lock holder
- Emits diagnostic events for visibility

### 4. Scheduling (Epic 5: Phase 1 — Foundation)

Implemented but not yet integrated into worker scheduling:

**TimeSliceScheduler** (weighted round-robin):
- 50% Critical, 30% High, 15% Normal, 5% Low
- Deterministic: Same state → same next priority
- Prevents starvation: Low priority always eventually runs

**PriorityQueueManager** (4 queues):
- Separate VecDeque per priority level
- Respects degradation mode (Normal/Moderate/High/Critical)
- In Critical mode, only Critical queue available

**PriorityInversionDetector**:
- Tracks task-to-lock ownership
- Identifies when high-priority waiting on low-priority

---

## Distributed Execution (Epics 2–5)

### TCP Transport Layer (Epic 5 Gate 1)

**FramedTransport** (length-prefixed JSON):
```
┌─────────────────────────────┐
│  u32 LE message length      │ 4 bytes
├─────────────────────────────┤
│  JSON payload (UTF-8)       │ variable, max 16 MiB
└─────────────────────────────┘
```

**CoordinatorRequest** (14 variants):
- `RegisterNode { worker_id, hostname, capacity }`
- `Dequeue { worker_id }` → `CoordinatorResponse::Dequeued(Option<TaskDescriptor>)`
- `InsertLease { lease }` / `GetLease { task_id }` / `UpdateLease` / `RemoveLease`
- `QueryCache { cache_key }` → `Vec<CacheLocation>`
- `AnnounceCache { cache_key, path }`
- Event/log emission (fire-and-forget)

**Error handling**:
- Clean EOF: `Ok(None)`
- Truncated frame: `TransportError::Protocol("truncated frame")`
- Oversized message: `TransportError::MessageTooLarge`
- JSON errors: `TransportError::Serialize`

### Coordinator Server (Epic 5 Gate 2)

**State Management**:
```rust
pub struct CoordinatorState {
    pub queue: Arc<StubScheduler>,              // task queue
    pub lease_store: Arc<InMemoryLeaseStore>,   // leases
    pub node_registry: Arc<NodeRegistry>,       // worker health
    pub cache_index: Arc<CacheIndex>,           // distributed cache
}
```

All fields wrapped in Arc for concurrent access; no global mutable state.

**Per-Connection Handler**:
1. Accept TCP stream
2. `register_node()` → store in NodeRegistry
3. Loop: `recv_message()` → `handle_request()` → `send_message()`
4. On EOF/error: deregister node, evict cache entries

**Node Reaper Loop** (separate task):
- Every 10s: `stale_nodes(timeout=60s)`
- For each stale node:
  - Requeue its active leases with `attempt += 1`
  - Remove from registry
  - Evict cache entries

### Worker Node Client (Epic 5 Gate 3)

**NodeConnection** (shared TCP link):
```rust
#[derive(Clone)]
pub struct NodeConnection {
    inner: Arc<Mutex<Option<FramedTransport>>>,  // Arc<Mutex> for shared access
    coordinator_addr: SocketAddr,
    worker_id: WorkerId,
    // ...
}
```

Single TCP connection shared across all four trait implementations.

**Background Tasks**:
- `start_heartbeat_loop()`: Sends `NodeHeartbeat` every N seconds
- `start_reconnect_loop()`: Exponential backoff (1s → 30s max) on disconnect

**Trait Implementations**:
- **NetworkQueue**: Calls `Dequeue`, parses response
- **NetworkLeaseStore**: Calls `InsertLease/GetLease/UpdateLease/RemoveLease`, maps errors
- **NetworkEventSink**: Fire-and-forget (event loss acceptable Phase 1)
- **NetworkCacheClient**: Standalone, calls `QueryCache/AnnounceCache`

**Error Mapping** (from CoordinatorResponse::Error):
- Contains "already" → `CoreError::AlreadyLeased`
- Contains "not found" → `CoreError::LeaseNotFound`
- Otherwise → `CoreError::Internal(msg)`

---

## Remote Session Management (Issue #37)

### Problem Statement

Running tasks on remote Apple Silicon machine (`aivcs.local`) requires:
1. **Isolation**: Each task gets its own tmux session (no interference)
2. **Determinism**: Same inputs → always attach to same session
3. **Safety**: Prevent shell injection, enforce correct directory
4. **Idempotence**: Repeated calls with same inputs don't create duplicates

### Solution: aivcs-session CLI Tool

#### Design Spec

```bash
aivcs-session attach \
  --repo knittingCrab \
  --work task-12345 \
  --role runner
```

#### Inputs

| Parameter | Type | Example | Rules |
|-----------|------|---------|-------|
| `repo` | string | `knittingCrab` | Lowercase, `[a-z0-9._-]` only, forbid `..` and `/` |
| `work` | string | `task-12345` | Work/task ID, sanitized |
| `role` | enum | `agent`/`runner`/`human` | Determines session lifecycle policy |

#### Session Naming

**Deterministic format**:
```
aivcs__${REPO}__${WORK}__${ROLE}
```

Example: `aivcs__knittingcrab__task-12345__runner`

**Sanitization**:
- Lowercase all input
- Replace non-`[a-z0-9._-]` with `_`
- Forbid `..` and `/`
- Validate length < 250 chars

#### Implementation (Rust/Bash)

**Pseudo-Rust**:
```rust
async fn attach_session(repo: &str, work: &str, role: &str) -> Result<()> {
    // 1. Sanitize inputs
    let repo = sanitize_name(repo)?;      // forbid ..
    let work = sanitize_name(work)?;
    let role = validate_role(role)?;

    // 2. Generate session name
    let session_name = format!("aivcs__{}__{}__{}", repo, work, role);

    // 3. Check repo exists on remote
    let repo_dir = format!("~/engineering/code/clone-base/{}", repo);
    autossh("aivcs@aivcs.local", &format!("[ -d {} ] || exit 1", repo_dir))?;

    // 4. Attach-or-create tmux session
    let cmd = format!(
        r#"/opt/homebrew/bin/tmux new-session -A -s "{}" -c "{}""#,
        session_name, repo_dir
    );
    exec_autossh("aivcs@aivcs.local", &cmd)?;

    Ok(())
}

fn sanitize_name(s: &str) -> Result<String> {
    let lower = s.to_lowercase();
    if lower.contains("..") || lower.contains('/') {
        return Err("forbidden characters");
    }
    let sanitized: String = lower
        .chars()
        .map(|c| if c.is_alphanumeric() || "._-".contains(c) { c } else { '_' })
        .collect();
    Ok(sanitized)
}
```

#### Session Lifecycle Policies

| Role | Lifetime | Cleanup | Use Case |
|------|----------|---------|----------|
| `agent` | Long-lived | Manual or operator prune | Agent debugging, exploration |
| `runner` | Task duration | Auto-kill on completion | CI/CD pipeline tasks |
| `human` | Interactive | Manual `kill-session` | Developer sandbox |

#### Operational Commands

```bash
# List all aivcs sessions
autossh -t aivcs@aivcs.local "/opt/homebrew/bin/tmux ls"

# Kill a specific session
autossh -t aivcs@aivcs.local "/opt/homebrew/bin/tmux kill-session -t aivcs__knittingcrab__task-12345__runner"

# Prune sessions older than N hours
autossh aivcs@aivcs.local "tmux list-sessions | grep aivcs__ | while read line; do
  # Extract creation time, check age
  # kill if too old
done"
```

### Scheduler Integration

The coordinator scheduler will:

1. **When dispatching task to remote worker**:
   ```
   SELECT task FROM queue WHERE resource_constraint.location = "aivcs.local"
   FOR worker_id in [aivcs_workers]:
       session_name = aivcs-session attach --repo knittingCrab --work task-id --role runner
       send_dequeue_request(worker_id)
   ```

2. **Cleanup on task completion**:
   ```
   if task.status == Completed or Abandoned:
       aivcs-session kill --session session_name
   ```

3. **Monitoring**:
   ```
   heartbeat_handler:
       if session_exists(session_name):
           mark_worker_alive()
       else:
           mark_worker_dead()
           emit_event(WorkerOffline)
           requeue_active_leases()
   ```

---

## Scheduler Core (Epic 1)

### Hard Invariants (The Resurrection)

Every Epic 1 PR must verify these:

1. **DAG Correctness**: No task runs before deps complete
   - Unit test: `cannot_run_before_deps_complete`
   - Property test: Random DAG → never schedule early
2. **Ordering**: Priority + fairness always respected
   - Unit test: `priority_orders_correctly`
   - Simulation: Sustained high load → low priority still advances
3. **Resource Safety**: Never exceed configured slots
   - Unit test: `does_not_overcommit_cpu`, `ram_budget_enforced`, `exclusive_metal_slot_enforced`
   - Property test: Random tasks/resources → never exceed capacity
4. **Progress**: Starvation prevention with bounded SLO
   - Simulation: High-priority flood → aged tasks still lease within N ticks
   - Event: `SchedulingDecision::AgedBoost` emitted
5. **Explainability**: Every decision attributed
   - Unit test: `explain_returns_correct_reason_for_each_blocked_type`
   - Types: `BlockedByDeps(ids)`, `BlockedByResource(reason)`, `BlockedByBackpressure(mode)`
6. **Effectiveness**: Soak + metrics prove it works
   - 30–60s deterministic soak in CI
   - Metrics: runnable latency, starvation mitigations, resource utilization

### US1: DAG Scheduling with Cycle Detection (High)

**What**: Task graph with dependencies, cycle detection

**Design**:
```rust
pub struct TaskDescriptor {
    // ... existing fields ...
    pub dependencies: Vec<TaskId>,  // NEW: parent task IDs
}

pub struct SchedulerDecision {
    runnable_tasks: Vec<TaskId>,   // tasks ready to run (deps satisfied)
    blocked_reasons: HashMap<TaskId, BlockReason>,
}

pub enum BlockReason {
    DependenciesNotComplete(Vec<TaskId>),
    InsufficientResources(String),
    Backpressure(DegradationMode),
    CycleDetected,
}
```

**Algorithm**:
- Topological sort + cycle detection on enqueue
- Only issue leases for tasks with `dependencies.all_complete()`
- Track completed tasks in CoordinatorState

**Tests**:
```
cannot_run_before_deps_complete                 (unit)
cycle_detection_rejects_plan                    (unit)
diamond_deps_ordering_stable                    (regression)
random_dag_never_schedules_early                (property)
```

### US2: Priority Ordering + Fairness (Medium)

**What**: Priority affects ordering; fairness prevents starvation

**Design**:
- Integrate TimeSliceScheduler (already exists) into dequeue
- Add "aging" for queued tasks (increase priority if waited too long)
- Configurable SLO: "runnable task leases within N scheduler ticks"

**Algorithm**:
```
when_dequeuing(worker_id):
    runnable = [t for t in queued if t.deps_complete()]

    // Apply time-slice scheduler
    next_priority = time_slice.next_priority()

    // Find highest priority runnable task
    candidates = [t for t in runnable if t.priority == next_priority]

    if candidates:
        aged = [t for t in candidates if age > STARVATION_SLO]
        if aged:
            return aged[0]  // boost aged task
        return candidates[0]

    // No candidates at this priority, try next lower priority
    // (continue until found or all queues empty)
```

**Tests**:
```
priority_orders_correctly                       (unit)
fairness_prevents_starvation_simulation         (simulation)
starvation_mitigation_emits_event               (unit)
```

### US3: Resource Allocation (Medium)

**What**: CPU/RAM/Metal slots enforced

**Design**:
```rust
pub struct ResourceAllocation {
    pub cpu_millicores: u32,  // e.g., 4000 = 4.0 CPU
    pub memory_mb: u32,       // e.g., 2048 = 2 GB
    pub metal_slots: u32,     // 0 or 1 (exclusive)
}

pub struct ResourceCapacity {
    pub cpu_millicores: u32,    // M4 Max has 12 cores = 12000
    pub memory_mb: u32,         // 36 GB default
    pub metal_slots: u32,       // 1 (exclusive GPU resource)
}
```

**Algorithm**:
```
when_dequeuing(worker_id):
    task = next_runnable_task()
    available = capacity - currently_allocated

    if task.resources.cpu_millicores > available.cpu_millicores:
        return BlockReason::InsufficientResources("CPU")

    if task.metal_slots > 0 and metal_slots_available == 0:
        return BlockReason::InsufficientResources("ExclusiveMetalSlot")

    allocate(task.resources)
    return task
```

**Tests**:
```
does_not_overcommit_cpu                         (unit)
ram_budget_enforced                             (unit)
exclusive_metal_slot_enforced                   (unit)
random_task_load_never_exceeds_capacity         (property)
```

### US4: Starvation Prevention (Medium)

**What**: SLO-based guarantee that any runnable task eventually leases

**Design**:
- Define starvation SLO: "Any runnable task must lease within N scheduler ticks (configurable, default 100)"
- Track: `task.queued_at`, `current_tick`
- Emit: `SchedulingDecision::AgedBoost` when age > SLO

**Tests**:
```
no_starvation_under_sustained_load              (simulation)
aged_tasks_prioritized                          (unit)
starvation_detection_emits_event                (unit)
```

---

## Observability (Epic 6)

### US1: Queue + Running State Visibility (Medium)

**What**: `status` CLI shows current queue, running, resources

**Design**:
```bash
$ knitting-crab status

Queue Summary:
  Total: 42 tasks
  By priority: Critical 3 | High 8 | Normal 18 | Low 13
  Top 5 queued:
    task-001 (Critical, 2m ago)
    task-002 (High, 1m ago)
    ...

Resource Usage:
  CPU: 8000/12000 millicores (67%)
  RAM: 22/36 GB (61%)
  Metal slots: 1/1 (100%)

Workers:
  aivcs_worker_1: Running task-042, last heartbeat 2s ago
  local_worker_2: Idle, 3 tasks completed
  local_worker_3: Dead (no heartbeat for 65s, will auto-requeue)
```

**Implementation**:
- Query CoordinatorState: queue depths, allocated resources, worker status
- Format as CLI table (golden snapshot tests)
- Optional JSON export for metrics pipelines

### US2: Event Timelines + Causality (Medium)

**What**: Per-task timeline and explain

**Design**:
```bash
$ knitting-crab timeline task-001
2024-02-22 10:15:23.001 TaskEvent::Queued
2024-02-22 10:15:24.523 TaskEvent::BlockedByDeps([task-999, task-1000])
2024-02-22 10:15:46.001 TaskEvent::Acquired by worker_2
2024-02-22 10:15:46.100 TaskEvent::Started
2024-02-22 10:16:10.500 TaskEvent::Completed (exit code 0)

$ knitting-crab explain task-042
Currently: QUEUED
Reason: BlockedByResource (CPU: need 4000 mC, have 0 available)
Blocking task: task-040 (3000 mC, 30s remaining)
Next opportunity: ~30s from now
```

**Implementation**:
- Event sink stores (timestamp, event) tuples per task
- `timeline`: Filter events by task_id, sort by timestamp
- `explain`: Inspect latest state transition reason

### US3: Tunable Policy Config (Low)

**What**: Config file for scheduling policies

**Design** (TOML):
```toml
[scheduler]
fairness_mode = "time_slice"           # or "aging"
aging_rate_ticks = 100
time_slice_critical = 50               # % allocation
time_slice_high = 30
time_slice_normal = 15
time_slice_low = 5

[resources]
cpu_millicores = 12000                 # M4 Max
memory_mb = 36864
metal_slots = 1

[backpressure]
max_queue_depth = 10000
moderate_threshold = 0.5
high_threshold = 0.8
critical_threshold = 0.95

[remote_execution]
aivcs_local_enabled = true
aivcs_repo_base = "~/engineering/code/clone-base"
```

**Validation**: Startup checks, defaults applied, errors clear

---

## Testing & Reliability (Epic 7)

### US1: Soak Tests + Invariants (Medium)

**What**: Hours-long randomized workload harness

**Design**:
```rust
#[tokio::test(flavor = "multi_thread")]
async fn soak_test_short_30s() {
    let harness = SoakHarness::new()
        .duration(Duration::from_secs(30))
        .random_dag_count(100)
        .failure_injection(FailureMode::Crash, 0.05)  // 5% crash rate
        .build();

    let report = harness.run().await;

    // Invariant checks
    assert!(report.queue_drained() || report.stable());
    assert!(!report.deadlock_detected);
    assert!(report.memory_growth_bounded(100 * 1024 * 1024));  // < 100 MB

    println!("{}", report);  // summary artifact
}
```

**Failure injection modes**:
- Crash: Worker disappears
- Hang: Worker stops heartbeat
- Timeout: Task takes too long
- Network partition: Coordinator unreachable

**Assertions**:
- Queue drains to empty or reaches stable state
- No deadlocks (using cycle detection in wait graph)
- Memory < start + 100 MB over duration
- All tasks eventually terminal (done/failed/abandoned)

### US2: Metrics Validation (Medium)

**What**: Metrics exported, stable schema, prove effectiveness

**Design**:
```json
{
  "timestamp": "2024-02-22T10:15:23Z",
  "metrics": {
    "queue": {
      "total": 42,
      "critical": 3,
      "high": 8,
      "normal": 18,
      "low": 13
    },
    "workers": {
      "healthy": 2,
      "stale": 0,
      "recovered": 3
    },
    "resources": {
      "cpu_utilization_pct": 67,
      "memory_utilization_pct": 61,
      "metal_slot_utilization_pct": 100
    },
    "scheduling": {
      "runnable_latency_ms": 145,      // avg time from runnable to leased
      "starvation_mitigations": 2,     // aged boosts applied
      "circuit_breaker_trips": 0
    },
    "task_outcomes": {
      "completed": 1203,
      "failed": 15,
      "abandoned": 2,
      "retried": 45
    }
  }
}
```

**Export**:
- Prometheus `/metrics` endpoint (Prometheus text format)
- JSON export for archival
- Golden tests for schema stability

---

## Design Decisions

### 1. Trait-Based Over Monolithic

**Decision**: Use `Queue`, `LeaseStore`, `EventSink` traits instead of single `Scheduler` type.

**Rationale**:
- Enables local testing (FakeWorker) vs distributed (NetworkQueue) without code duplication
- Allows swapping implementations: in-memory → Redis → database
- Follows dependency injection pattern, testable without mocks

### 2. Single TCP Connection Per Worker

**Decision**: `NodeConnection` shared via `Arc<Mutex<>>` rather than connection pool.

**Rationale**:
- Phase 1 acceptable (worker tasks are sequential)
- Simplifies state management (one source of truth)
- Phase 2 will add request multiplexing via request IDs if needed

### 3. Fire-and-Forget Event Sink

**Decision**: `NetworkEventSink` swallows transport errors (event loss acceptable).

**Rationale**:
- Task execution must not block on event delivery
- Events are observability, not correctness critical (state is in leases/queue)
- Phase 2 will add acknowledgment + retry if needed

### 4. Deterministic Session Names

**Decision**: `aivcs__${repo}__${work}__${role}` with sanitization.

**Rationale**:
- Idempotent: Same inputs → same session always
- Safe: Forbids `..`, `/` and shell injection
- Observable: Human-readable session names for debugging

### 5. Distributed Coordinator vs Decentralized

**Decision**: Single CoordinatorServer (not decentralized consensus).

**Rationale**:
- Simpler model (single source of truth)
- M4 Max is typically single machine (scaling later with replicas)
- Network protocols (Raft) add complexity; worth only at scale

### 6. Priority Inversion Detection (Not Mitigation)

**Decision**: Detect and report, don't auto-fix (Phase 1).

**Rationale**:
- Diagnostics first; automated mitigation (priority inheritance) adds complexity
- Allows operators to understand scheduler behavior
- Phase 2 can add mitigation if needed

---

## Implementation Roadmap

### Phase 1 (In Progress)

- ✅ Epics 2–5: Worker runtime, distribution, resilience
- 🔄 Epic 1: DAG + priority + resources (design phase)
- 🔄 Epic 6: Observability (design phase)
- 🔄 Epic 7: Soak + metrics (design phase)

### Phase 2 (Next)

- Implement Epic 1 (4 stories)
  - Critical path: DAG cycle detection
- Implement Epic 6 (3 stories)
  - Unblocks debugging
- aivcs-session CLI tool
  - Enables remote execution

### Phase 3 (Future)

- Implement Epic 7 (2 stories)
- Metrics integration (Prometheus, Datadog, etc.)
- Scheduler replicas (Raft)
- Web UI for timeline visualization

---

## References

- [Issue #35](https://github.com/stevedores-org/knittingCrab/issues/35): Overall progress tracking
- [Issue #36](https://github.com/stevedores-org/knittingCrab/issues/36): TDD + architecture baseline
- [Issue #37](https://github.com/stevedores-org/knittingCrab/issues/37): Session management spec (remote execution)
- [README.md](README.md): Quick start & status
- [IMPLEMENTATION.md](IMPLEMENTATION.md): Epic 2 detailed guide (outdated, to be updated)
