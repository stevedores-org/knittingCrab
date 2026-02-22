# Scheduler Integration with aivcs-session

**Purpose**: Detailed guide for integrating remote session management into the CoordinatorServer and WorkerNode for multi-machine task execution.

**Scope**: Epic 1 + Phase 2 implementation (not yet started)

---

## Architecture Overview

```
┌─────────────────────────────────────┐
│     CoordinatorServer               │
│  (manage queue, leases, nodes)      │
└──────────────┬──────────────────────┘
               │
        enqueue_task(TaskDescriptor)
               │
        ┌──────┴──────┬──────────────┐
        │             │              │
        ▼             ▼              ▼
   [Local]       [Local]        [Remote]
   Worker1       Worker2      (aivcs.local)
                                   │
                        aivcs-session attach
                        (creates tmux session)
                                   │
                        WorkerNode connects
                        (NetworkQueue/LeaseStore)
                                   │
                        execute task normally
                                   │
                        aivcs-session kill
                        (cleanup on completion)
```

---

## Task Descriptor Extensions (for Remote Execution)

Currently `TaskDescriptor` includes:
```rust
pub struct TaskDescriptor {
    pub task_id: TaskId,
    pub command: Vec<String>,
    pub working_dir: PathBuf,
    pub env: HashMap<String, String>,
    pub resources: ResourceAllocation,
    pub policy: RetryPolicy,
    pub attempt: u32,
    pub is_critical: bool,
    pub priority: Priority,
    pub dependencies: Vec<TaskId>,  // from Epic 1
}
```

**Extend with** (Phase 2):
```rust
pub struct TaskDescriptor {
    // ... existing fields ...

    /// Execution location constraint
    pub location: ExecutionLocation,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExecutionLocation {
    /// Run on any available worker (default)
    Local,

    /// Run specifically on aivcs.local (Apple Silicon studio)
    RemoteAivcs {
        /// Repository to clone/use
        repo_name: String,

        /// Optional: specific branch/commit
        git_ref: Option<String>,
    },

    /// Future: other remote environments
    RemoteK8s { namespace: String },
}

impl Default for ExecutionLocation {
    fn default() -> Self {
        ExecutionLocation::Local
    }
}
```

---

## Dequeue Logic (CoordinatorServer)

Current dequeue (Phase 1):
```rust
async fn handle_dequeue(&self, worker_id: WorkerId) -> CoordinatorResponse {
    match self.queue.dequeue(worker_id).await {
        Ok(Some(task)) => CoordinatorResponse::Dequeued(Some(task)),
        Ok(None) => CoordinatorResponse::Dequeued(None),
        Err(e) => CoordinatorResponse::Error(e.to_string()),
    }
}
```

**Extended dequeue** (Phase 2):
```rust
async fn handle_dequeue(&self, worker_id: WorkerId) -> CoordinatorResponse {
    match self.queue.dequeue(worker_id).await {
        Ok(Some(task)) => {
            // Check if task requires remote execution
            match &task.location {
                ExecutionLocation::Local => {
                    // Normal case: worker handles directly
                    CoordinatorResponse::Dequeued(Some(task))
                }
                ExecutionLocation::RemoteAivcs { repo_name, git_ref } => {
                    // Create/attach tmux session on aivcs.local
                    if let Err(e) = self.setup_remote_session(
                        &task.task_id,
                        repo_name,
                        git_ref.as_deref(),
                    ).await {
                        // Session creation failed: requeue and return error
                        let _ = self.queue.requeue(task.task_id, task.attempt).await;
                        return CoordinatorResponse::Error(
                            format!("remote session setup failed: {}", e)
                        );
                    }

                    // Return task with session info
                    CoordinatorResponse::Dequeued(Some(task))
                }
                ExecutionLocation::RemoteK8s { .. } => {
                    CoordinatorResponse::Error("K8s not yet implemented".to_string())
                }
            }
        }
        Ok(None) => CoordinatorResponse::Dequeued(None),
        Err(e) => CoordinatorResponse::Error(e.to_string()),
    }
}

async fn setup_remote_session(
    &self,
    task_id: &TaskId,
    repo_name: &str,
    git_ref: Option<&str>,
) -> Result<String, String> {
    // 1. Call aivcs-session CLI
    let session_name = format!("aivcs__{}__{}__{}",
        repo_name.to_lowercase(),
        task_id,
        "runner"
    );

    // Invoke aivcs-session directly without going through a shell to avoid command injection.
    // Pass each argument as a separate .arg() so that repo_name and task_id are never parsed by a shell.
    let output = tokio::process::Command::new("aivcs-session")
        .arg("attach")
        .arg("--repo")
        .arg(repo_name)
        .arg("--work")
        .arg(task_id.to_string())
        .arg("--role")
        .arg("runner")
        .timeout(Duration::from_secs(30))
        .output()
        .await
        .map_err(|e| format!("aivcs-session exec failed: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(stderr.to_string());
    }

    // 2. Optional: if git_ref specified, checkout branch/commit in session
    if let Some(git_ref) = git_ref {
        // Use separate arguments to avoid shell interpretation of git_ref
        let _output = tokio::process::Command::new("aivcs-session")
            .arg("exec")
            .arg(&session_name)
            .arg("bash")
            .arg("-c")
            .arg(format!(
                "cd ~/engineering/code/clone-base/{} && git checkout {}",
                shell_escape(repo_name),
                shell_escape(&git_ref)
            ))
            .timeout(Duration::from_secs(30))
            .output()
            .await
            .map_err(|e| format!("git checkout in aivcs-session failed: {}", e))?;
    }

    Ok(session_name)
}
```

---

## Task Cleanup (On Completion or Timeout)

Current: No special cleanup for local tasks (process killed, that's it)

**With remote**: Need to kill tmux session

```rust
async fn handle_task_completion(
    &self,
    task: &TaskDescriptor,
    outcome: TaskOutcome,
) {
    // 1. Release lease
    let _ = self.lease_store.remove(task.task_id).await;

    // 2. If remote execution, kill tmux session
    if let ExecutionLocation::RemoteAivcs { repo_name, .. } = &task.location {
        let session_name = format!("aivcs__{}__{}__{}",
            repo_name.to_lowercase(),
            task.task_id,
            "runner"
        );

        let cmd = format!("aivcs-session kill --session {}", session_name);
        let _ = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .output()
            .await;  // ignore errors; session may already be gone
    }

    // 3. Emit completion event
    let _ = self.event_sink.emit_event(
        TaskEvent::Completed {
            task_id: task.task_id,
            worker_id: /* ... */,
            exit_code: outcome.exit_code,
        }
    ).await;

    // 4. If failed and retryable, requeue
    if should_retry(&outcome, &task.policy) {
        let _ = self.queue.requeue(task.task_id, task.attempt + 1).await;
    }
}
```

---

## Node Registry Extensions

Current `NodeInfo`:
```rust
pub struct NodeInfo {
    pub worker_id: WorkerId,
    pub hostname: String,
    pub capacity: ResourceAllocation,
    pub registered_at: DateTime<Utc>,
}
```

**Extend with** (Phase 2):
```rust
pub struct NodeInfo {
    pub worker_id: WorkerId,
    pub hostname: String,
    pub capacity: ResourceAllocation,
    pub registered_at: DateTime<Utc>,

    /// Where this worker runs
    pub location: WorkerLocation,
}

#[derive(Debug, Clone)]
pub enum WorkerLocation {
    /// Local machine (implicit)
    Local,

    /// Remote via tmux/SSH
    RemoteSession {
        ssh_host: String,           // e.g., "aivcs.local"
        tmux_session: String,       // e.g., "aivcs__knittingcrab__task-1__runner"
        last_session_check: DateTime<Utc>,
    },
}
```

**Health check for remote workers**:
```rust
async fn check_remote_worker_health(
    &self,
    worker_id: &WorkerId,
    location: &WorkerLocation,
) -> Result<(), String> {
    if let WorkerLocation::RemoteSession { ssh_host, tmux_session, .. } = location {
        // Check if session still exists
        let cmd = format!(
            "autossh -t {}@{} '/opt/homebrew/bin/tmux has-session -t {}'",
            "aivcs", ssh_host, tmux_session
        );

        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(&cmd)
            .timeout(Duration::from_secs(5))
            .output()
            .await
            .map_err(|e| format!("health check timeout: {}", e))?;

        if !output.status.success() {
            return Err("tmux session terminated".to_string());
        }
    }
    Ok(())
}
```

---

## Worker Node Extension (aivcs)

Current `WorkerNode`:
```rust
pub struct WorkerNode {
    worker_id: WorkerId,
    coordinator_addr: SocketAddr,
    capacity: ResourceAllocation,
    hostname: String,
    heartbeat_interval: Duration,
}

impl WorkerNode {
    pub async fn connect(self) -> Result<ConnectedNode, NodeError> {
        // ... TCP connection ...
    }
}
```

**On aivcs.local**, when WorkerNode starts:

1. **Session detection** (optional):
   ```rust
   async fn detect_session_environment() -> WorkerLocation {
       if let Ok(session) = std::env::var("TMUX_SESSION") {
           WorkerLocation::RemoteSession {
               ssh_host: "aivcs.local".to_string(),
               tmux_session: session,
               last_session_check: Utc::now(),
           }
       } else {
           WorkerLocation::Local
       }
   }
   ```

2. **Register with coordinator**:
   ```rust
   async fn register(&self, location: WorkerLocation) -> Result<(), NodeError> {
       let register_req = CoordinatorRequest::RegisterNode {
           worker_id: self.worker_id,
           hostname: self.hostname.clone(),
           capacity: self.capacity,
           location,  // NEW: tell coordinator where we run
       };
       // ... send registration ...
   }
   ```

3. **Normal execution** (unchanged):
   ```rust
   async fn run_loop(&self) -> Result<(), NodeError> {
       loop {
           match self.dequeue_and_execute().await {
               Ok(()) => { /* task completed */ }
               Err(e) => { /* handle error */ }
           }
       }
   }
   ```

---

## Error Scenarios

### Scenario 1: aivcs-session attach fails

```
Coordinator tries to dispatch to aivcs
  → aivcs-session attach fails (repo not found, SSH down, etc.)
  → Coordinator returns Error response
  → WorkerNode retries with exponential backoff
  → Task eventually dequeued by different worker or timeout
```

**Implementation**:
```rust
if let Err(e) = self.setup_remote_session(...).await {
    // Requeue task immediately (don't wait for timeout)
    self.queue.requeue(task_id, attempt).await?;

    // Emit error event for operator visibility
    self.event_sink.emit_event(
        TaskEvent::Error {
            task_id,
            reason: format!("remote session setup failed: {}", e),
        }
    ).await.ok();
}
```

### Scenario 2: Task hangs on aivcs

```
Task running in tmux session doesn't respond
  → Heartbeat fails (no response from WorkerNode)
  → NodeRegistry marks node stale after 60s
  → NodeReaper wakes up
  → Requeues task.active_leases()
  → Task is available for different worker
```

**No special handling needed**: Existing heartbeat + stale detection works.

### Scenario 3: aivcs.local network partition

```
CoordinatorServer can't reach aivcs via SSH
  → aivcs-session timeout (e.g., 30s)
  → Error returned to coordinator
  → Task requeued
  → (Eventually) WorkerNode on aivcs can't reach coordinator
  → Reconnect loop: exponential backoff
  → When network heals, reconnect succeeds
```

**Circuit breaker** (from Epic 5):
```rust
let circuit_breaker = CircuitBreaker::with_config(
    CircuitBreakerConfig {
        failure_threshold: 5,
        timeout: Duration::from_secs(60),
        ..Default::default()
    }
);

// Track aivcs-session failures
match self.setup_remote_session(...).await {
    Ok(session) => circuit_breaker.record_success(),
    Err(e) => {
        circuit_breaker.record_failure();
        if circuit_breaker.is_open() {
            // Reject all remote tasks until cooldown
            return CoordinatorResponse::Error("circuit breaker open".to_string());
        }
    }
}
```

---

## Configuration (Phase 2 / Epic 6 US3)

Add to config file:
```toml
[remote_execution]
# aivcs.local settings
aivcs_enabled = true
aivcs_host = "aivcs.local"
aivcs_user = "aivcs"
aivcs_repo_base = "~/engineering/code/clone-base"

# Timeouts
aivcs_session_setup_timeout_s = 30
aivcs_health_check_timeout_s = 5

# Circuit breaker for aivcs failures
aivcs_failure_threshold = 5
aivcs_recovery_timeout_s = 60

# Session cleanup
auto_kill_runner_sessions = true
session_prune_older_than_hours = 24
```

---

## Testing Strategy

### Unit Tests (no aivcs required)

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn test_execution_location_serialization() {
        let loc = ExecutionLocation::RemoteAivcs {
            repo_name: "knittingCrab".to_string(),
            git_ref: Some("main".to_string()),
        };

        let json = serde_json::to_string(&loc).unwrap();
        let deserialized: ExecutionLocation = serde_json::from_str(&json).unwrap();
        assert_eq!(loc, deserialized);
    }

    #[test]
    fn test_remote_task_requeue_on_session_setup_fail() {
        // Mock CoordinatorServer::setup_remote_session to return error
        // Verify task is requeued
    }
}
```

### Integration Tests (requires aivcs.local)

```rust
#[tokio::test]
#[ignore]  // requires aivcs.local setup
async fn test_full_remote_task_lifecycle() {
    // 1. Create task with RemoteAivcs location
    let task = TaskDescriptor {
        location: ExecutionLocation::RemoteAivcs {
            repo_name: "knittingCrab".to_string(),
            git_ref: Some("main".to_string()),
        },
        ..default_task()
    };

    // 2. Enqueue on coordinator
    coordinator.enqueue_task(task.clone()).unwrap();

    // 3. Dequeue from aivcs worker
    let dequeued = coordinator.dequeue_task(aivcs_worker_id).await.unwrap();
    assert_eq!(dequeued.task_id, task.task_id);

    // 4. Execute task
    // (WorkerNode runs normally)

    // 5. Cleanup: verify session killed
    // ...
}
```

Run with:
```bash
cargo test --test remote_integration -- --ignored --nocapture
```

---

## Deployment Sequence

**Phase 2 (Epics 1 + remote execution)**:

1. **Week 1**: Implement Epic 1 (DAG, priority, resources) in-process
   - No remote execution changes yet
   - All workers local

2. **Week 2**: aivcs-session tool
   - CLI validation, SSH, tmux integration
   - Unit tests pass locally
   - Integration tests (requires aivcs.local access)

3. **Week 3**: Integrate into coordinator
   - TaskDescriptor.location field
   - Dequeue logic updated
   - Cleanup on completion
   - Error handling (requeue on setup fail)

4. **Week 4**: Validation & soak
   - Run soak tests with mix of local + remote tasks
   - Verify no deadlocks, memory stable
   - Metrics: remote task success rate, setup latency

---

## Appendix: Quick Integration Checklist

When implementing remote execution integration:

- [ ] Add `ExecutionLocation` enum to `TaskDescriptor`
- [ ] Update `CoordinatorRequest/Response` to handle remote setup
- [ ] Extend `NodeInfo` with `WorkerLocation`
- [ ] Implement `setup_remote_session()` in CoordinatorServer
- [ ] Implement `cleanup_remote_session()` in cleanup handler
- [ ] Add aivcs-session CLI tool (separate repo or subdir)
- [ ] Update TaskDescriptor serialization (Serialize, Deserialize)
- [ ] Add integration tests (with `#[ignore]`)
- [ ] Update configuration schema (TOML)
- [ ] Circuit breaker for aivcs failures
- [ ] Health check for remote workers
- [ ] Documentation update (README, ARCHITECTURE)
