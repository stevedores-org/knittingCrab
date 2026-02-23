# Architecture Update Summary (Issue #37 Integration)

**Date**: Feb 2026
**Scope**: Comprehensive architecture documentation + remote session management strategy
**Status**: Documentation complete, implementation pending Phase 2

---

## What Was Delivered

### 1. Updated README.md
- Reflects current status: 67% complete (16/24 user stories)
- Lists completed epics (2–5) with test counts
- Describes all key components across distributed system
- Adds remote execution strategy section
- Links to detailed architecture documentation

### 2. New ARCHITECTURE.md (30+ pages)
**Comprehensive system architecture covering**:
- System overview diagram (Client → Coordinator → Workers)
- Trait-based abstraction explanation (Queue, LeaseStore, EventSink)
- Task lifecycle state machine
- Resilience features (Circuit breaker, Timeouts, Backpressure)
- Scheduling foundation (TimeSlice, Priority Queue, Inversion Detection)
- Distributed execution (TCP transport, Coordinator server, Worker nodes)
- **Remote session management** (aivcs-session contract + integration)
- Scheduler core (Epic 1) detailed specification with hard invariants
- Observability (Epic 6) design
- Testing & reliability (Epic 7) framework
- Design decisions with rationales

**Key addition**: Detailed aivcs.local session management strategy linking Issue #37 into the architecture.

### 3. New AIVCS_SESSION_SPEC.md (20+ pages)
**Complete implementation specification for aivcs-session CLI tool**:
- User-facing interface (attach, kill, list, prune)
- Input validation & sanitization (forbid `..`, `/`, etc.)
- Deterministic session naming: `aivcs__${REPO}__${WORK}__${ROLE}`
- Pseudocode + full Rust implementation with tokio/SSH
- SSH module for executing commands on aivcs.local
- Validation module for input safety
- Testing strategy (unit + integration)
- Deployment instructions
- Future enhancements (connection pooling, metrics, config file)

### 4. New SCHEDULER_INTEGRATION.md (15+ pages)
**Tactical guide for Phase 2 implementation**:
- Architecture overview with remote execution flow
- TaskDescriptor extensions (ExecutionLocation enum)
- Extended dequeue logic (setup remote session before execution)
- Task cleanup (kill tmux session on completion)
- Node registry extensions (track remote worker location)
- Error scenarios & handling (session setup fail, task hang, network partition)
- Configuration (TOML settings for remote execution)
- Testing strategy (unit + integration tests)
- Deployment sequence (Week-by-week for Phase 2)
- Implementation checklist

---

## How It All Fits Together

```
Issue #37 (Session Management Spec)
  ↓
AIVCS_SESSION_SPEC.md
  (CLI tool design)
  ↓
SCHEDULER_INTEGRATION.md
  (How scheduler uses it)
  ↓
ARCHITECTURE.md
  (Integrated into overall system design)
  ↓
README.md
  (High-level overview + links)
```

### Key Insight: Pluggable Remote Execution

The trait-based design (already in place from Epic 5) makes remote execution **transparent to the core logic**:

```rust
// Same code works with local or remote tasks
async fn execute_task(queue: Arc<dyn Queue>, task: TaskDescriptor) {
    // Dequeue doesn't care if execution happens locally or on aivcs
    // The TaskDescriptor.location tells the coordinator where to send it
    match queue.dequeue(worker_id).await {
        Ok(Some(task)) => {
            // Worker (local or remote) executes normally
            // ProcessHandle or RemoteSession, same result: exit code + logs
        }
        Ok(None) => { /* queue empty */ }
    }
}
```

---

## Remote Execution Flow

### Before (Epic 5: All Local)
```
enqueue(task)
  ↓ dequeue(worker_id)
  ↓ NetworkQueue calls Coordinator
  ↓ Coordinator calls StubScheduler
  ↓ returns TaskDescriptor
  ↓ WorkerNode executes locally
```

### After (Phase 2: With aivcs.local)
```
enqueue(task with location=RemoteAivcs{repo, branch})
  ↓ dequeue(worker_id)
  ↓ NetworkQueue calls Coordinator
  ↓ Coordinator calls aivcs-session attach --repo X --work Y --role runner
  ↓ tmux session created on aivcs.local
  ↓ returns TaskDescriptor (same contract, no changes)
  ↓ WorkerNode (on aivcs) executes normally
  ↓ on completion: aivcs-session kill
```

**Key**: Coordinator handles remote setup; worker execution unchanged.

---

## Hard Invariants (The Resurrection)

Per Issue #36 + ARCHITECTURE.md, every scheduler PR must verify:

1. **DAG Correctness**: No task runs before deps complete ✓ (testable)
2. **Ordering**: Priority + fairness always respected ✓ (testable)
3. **Resource Safety**: Never exceed configured slots ✓ (testable)
4. **Progress**: Starvation prevention with SLO ✓ (testable)
5. **Explainability**: Every decision attributed ("why blocked") ✓ (testable)
6. **Effectiveness**: Soak + metrics prove it works ✓ (measurable)

These are built into the acceptance criteria for Epic 1 + Epic 7.

---

## Phase 2 Roadmap (from docs)

**Week 1**: Epic 1 (DAG, priority, resources) — all local
- Cycle detection unblocks everything
- Priority ordering + fairness
- Resource allocation + metal slots

**Week 2**: aivcs-session CLI tool
- Validation, SSH, tmux integration
- Unit tests pass locally
- Integration tests on actual aivcs.local

**Week 3**: Scheduler integration
- TaskDescriptor.location field
- Dequeue → aivcs-session attach
- Cleanup on completion

**Week 4**: Validation & soak
- Mix of local + remote tasks
- Metrics validation

---

## What Remains (8 User Stories)

From Issue #35:

| Epic | Story | Status |
|------|-------|--------|
| **1** | DAG scheduling + cycle detection | 🔴 Not started |
| **1** | Priority + fairness | 🔴 Not started |
| **1** | Resource allocation | 🔴 Not started |
| **1** | Starvation prevention | 🔴 Not started |
| **6** | Queue + status visibility | 🔴 Not started |
| **6** | Event timelines + causality | 🔴 Not started |
| **6** | Tunable policy config | 🔴 Not started |
| **7** | Soak tests + metrics | 🔴 Not started |

All designs in place; ready for implementation.

---

## How to Use These Documents

### For Contributors (Implementing Features)

1. **Start with ARCHITECTURE.md**: Understand the full system
2. **Find your story**: Look up in issue #35 or #36
3. **Read the spec**: Each epic has acceptance criteria + test checklist in ARCHITECTURE.md
4. **Reference SCHEDULER_INTEGRATION.md**: If touching remote execution
5. **Follow TDD**: Write tests first (unit → property → integration)

### For Operators/Users

1. **README.md**: Status and quick start
2. **ARCHITECTURE.md** (high-level sections): Understand components
3. **SCHEDULER_INTEGRATION.md** (Configuration section): Tune for your setup

### For Code Reviewers

1. **Hard invariants** (ARCHITECTURE.md): Verify PR doesn't violate
2. **Test checklist** (for each story): Ensure all tests present
3. **Design decisions**: Refer to rationales section

---

## Files Created/Updated

| File | Purpose | Lines |
|------|---------|-------|
| README.md | Updated: status, components, roadmap | 50–60 |
| ARCHITECTURE.md | NEW: comprehensive system design | ~900 |
| AIVCS_SESSION_SPEC.md | NEW: CLI tool implementation spec | ~600 |
| SCHEDULER_INTEGRATION.md | NEW: phase 2 integration guide | ~500 |
| ARCHITECTURE_UPDATE_SUMMARY.md | This file | ~300 |

**Total**: ~2,400 lines of architecture documentation

---

## Integration Points with Existing Code

These docs do **not** require code changes yet, but clearly identify:

### In crates/core/src/traits.rs
- Add `ExecutionLocation` enum to `TaskDescriptor` (Phase 2)
- Serializable, requires serde derive

### In crates/coordinator/src/server.rs
- Add `setup_remote_session()` method (Phase 2)
- Call aivcs-session CLI before returning task to worker
- Add cleanup on task completion

### In crates/node/src/worker_node.rs
- Extend `RegisterNode` request with location field (Phase 2)
- Report back where worker runs

### New binary (separate crate or crates/aivcs-session/)
- aivcs-session CLI tool (Phase 2)
- SSH/tmux orchestration

---

## Validation Against Requirements

**From Issue #37** (session management spec):
- ✅ Deterministic session naming
- ✅ Idempotent (same inputs → same session)
- ✅ Safe (forbids path traversal, shell injection)
- ✅ Correct directory (auto-starts in repo root)
- ✅ Operational commands (attach, kill, list, prune)
- ✅ Session lifecycle policies (agent/runner/human)

**From Issue #36** (TDD + architecture):
- ✅ Hard invariants defined (6 core principles)
- ✅ Epic 1–7 acceptance criteria written
- ✅ Test checklists for each story
- ✅ Copilot instructions integrated into ARCHITECTURE.md

**From Issue #35** (completion tracking):
- ✅ Current status reflected (67%)
- ✅ Remaining work enumerated (8 stories)
- ✅ Next steps identified

---

## Success Criteria for Phase 2

Implementation is "done" when:

1. ✅ Epic 1 complete (4 stories) with all hard invariants tested
2. ✅ Epic 6 complete (3 stories) with status CLI working
3. ✅ aivcs-session CLI tool deployable
4. ✅ Scheduler successfully dispatches tasks to aivcs.local
5. ✅ Soak test runs 30–60s in CI with invariants passing
6. ✅ Metrics exported with stable schema
7. ✅ Documentation updated to reflect new features

All prerequisites now in place.

---

## Next Steps

1. **Use these docs to unblock Epic 1 implementation**
   - Each story has detailed acceptance criteria
   - Test checklists are provided
   - Start with DAG cycle detection (unblocks others)

2. **Implement aivcs-session CLI tool**
   - AIVCS_SESSION_SPEC.md is implementation-ready
   - Can be done in parallel with Epic 1
   - Requires aivcs.local access for integration testing

3. **Integrate scheduler with remote execution**
   - SCHEDULER_INTEGRATION.md provides tactical guide
   - Can be done after basic Epic 1 is working
   - Phase 2 Week 3–4 activity

4. **Keep these docs updated as implementation proceeds**
   - Design decisions should be recorded
   - New patterns should be documented
   - Test results should feed back into docs

---

## References

- **Issue #35**: [Overall Completion Tracking](https://github.com/stevedores-org/knittingCrab/issues/35)
- **Issue #36**: [TDD Resurrection + Architecture Baseline](https://github.com/stevedores-org/knittingCrab/issues/36)
- **Issue #37**: [Session Management Spec](https://github.com/stevedores-org/knittingCrab/issues/37)
- **PR #30**: Epic 5 (Option B) Distribution & Scaling
- **PR #33**: Merge develop → main (all Epic 5 features)
