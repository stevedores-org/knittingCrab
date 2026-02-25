use async_trait::async_trait;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::CoreError;
use crate::event::{LogLine, TaskEvent};
use crate::execution_location::ExecutionLocation;
use crate::ids::{TaskId, WorkerId};
use crate::lease::Lease;
use crate::priority::Priority;
use crate::resource::ResourceAllocation;
use crate::retry::RetryPolicy;

/// A task descriptor passed to a worker from the queue.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskDescriptor {
    pub task_id: TaskId,
    pub command: Vec<String>,
    pub working_dir: PathBuf,
    pub env: HashMap<String, String>,
    pub resources: ResourceAllocation,
    pub policy: RetryPolicy,
    pub attempt: u32,
    /// Whether this task is critical and should run even under high load.
    /// Non-critical tasks may be rejected when the system enters Critical degradation mode.
    /// This field is maintained for Phase 1 backward compatibility.
    /// Prefer using the `priority` field for new code.
    pub is_critical: bool,
    /// Task priority level for scheduling and fair allocation.
    /// Determines queue placement and round-robin scheduling order.
    /// Defaults to Normal priority if not specified.
    pub priority: Priority,
    /// Task IDs that must complete before this task can run.
    #[serde(default)]
    pub dependencies: Vec<TaskId>,

    /// Where this task should execute (Local or Remote).
    /// Defaults to Local for backward compatibility.
    #[serde(default)]
    pub location: ExecutionLocation,
}

/// A queue that stores and distributes tasks to workers.
#[async_trait]
pub trait Queue: Send + Sync + 'static {
    /// Dequeue a task for a worker.
    async fn dequeue(&self, worker_id: WorkerId) -> Result<Option<TaskDescriptor>, CoreError>;

    /// Requeue a task for later retry.
    async fn requeue(&self, task_id: TaskId, attempt: u32) -> Result<(), CoreError>;

    /// Permanently discard a task.
    async fn discard(&self, task_id: TaskId) -> Result<(), CoreError>;
}

/// Storage and management of leases.
#[async_trait]
pub trait LeaseStore: Send + Sync + 'static {
    /// Insert a new lease. Returns error if one already exists for this task_id.
    async fn insert(&self, lease: Lease) -> Result<(), CoreError>;

    /// Get a lease by task_id.
    async fn get(&self, task_id: TaskId) -> Result<Option<Lease>, CoreError>;

    /// Update an existing lease.
    async fn update(&self, lease: Lease) -> Result<(), CoreError>;

    /// Remove a lease.
    async fn remove(&self, task_id: TaskId) -> Result<(), CoreError>;

    /// Get all active leases.
    async fn active_leases(&self) -> Result<Vec<Lease>, CoreError>;
}

/// Monitors and allocates system resources.
#[async_trait]
pub trait ResourceMonitor: Send + Sync + 'static {
    /// Check if resources can be allocated.
    async fn can_allocate(&self, allocation: &ResourceAllocation) -> Result<bool, CoreError>;

    /// Allocate resources (stub for Epic 1).
    async fn allocate(&self, allocation: &ResourceAllocation) -> Result<(), CoreError>;

    /// Release previously allocated resources.
    async fn release(&self, allocation: &ResourceAllocation) -> Result<(), CoreError>;
}

/// Sink for events and logs emitted by the runtime.
#[async_trait]
pub trait EventSink: Send + Sync + 'static {
    /// Emit a task event.
    async fn emit_event(&self, event: TaskEvent) -> Result<(), CoreError>;

    /// Emit a log line.
    async fn emit_log(&self, log: LogLine) -> Result<(), CoreError>;
}

/// Opaque handle to a live remote session.
#[derive(Debug, Clone)]
pub struct SessionHandle {
    pub session_name: String,
}

/// Result of running a command in a remote session.
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Config for a remote execution target (used by RemoteSessionManager).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RemoteSessionConfig {
    pub host: String,
    pub user: String,
    pub repo_name: String,
    pub work_id: String,
    pub role: RemoteRole,
}

/// Role of a remote session (mirrors aivcs-session::Role).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub enum RemoteRole {
    #[default]
    Runner, // disposable, killed after task
    Agent, // long-lived, survives task
    Human, // manual intervention
}

/// Manages lifecycle of remote tmux sessions (synchronous command execution).
#[async_trait]
pub trait RemoteSessionManager: Send + Sync + 'static {
    async fn create_or_attach(
        &self,
        config: &RemoteSessionConfig,
    ) -> Result<SessionHandle, CoreError>;
    async fn run_command(
        &self,
        session: &SessionHandle,
        cmd: &str,
    ) -> Result<ExecutionResult, CoreError>;
    async fn kill_session(&self, session: &SessionHandle) -> Result<(), CoreError>;
}

/// Configuration for a remote execution session.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub host: String,
    pub user: String,
    pub repo_name: String,
}

/// Manager for remote session execution via SSH + tmux (sentinel-based async polling).
#[async_trait]
pub trait RemoteSessionManagerTrait: Send + Sync + 'static {
    /// Ensure a tmux session exists for the given configuration.
    async fn ensure_session(&self, config: &SessionConfig) -> Result<String, CoreError>;

    /// Run a command in a specific session window.
    /// Returns when the command is submitted (not when it completes).
    /// Use poll_exit_code() to check for completion.
    async fn run_command_in_session(
        &self,
        session_name: &str,
        window_name: &str,
        command: &str,
        sentinel_path: &str,
    ) -> Result<(), CoreError>;

    /// Poll for exit code via sentinel file.
    /// Returns Some(code) if the sentinel file exists, None if not yet available.
    async fn poll_exit_code(&self, sentinel_path: &str) -> Result<Option<i32>, CoreError>;

    /// Remove the sentinel file (cleanup after task completes).
    async fn cleanup_sentinel(&self, sentinel_path: &str) -> Result<(), CoreError>;

    /// Kill a tmux session.
    async fn kill_session(&self, session_name: &str) -> Result<(), CoreError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_descriptor_creation() {
        let task_id = TaskId::new();
        let desc = TaskDescriptor {
            task_id,
            command: vec!["echo".to_string(), "hello".to_string()],
            working_dir: PathBuf::from("/tmp"),
            env: HashMap::new(),
            resources: ResourceAllocation::default(),
            policy: RetryPolicy::default(),
            attempt: 0,
            is_critical: false,
            priority: Priority::Normal,
            dependencies: vec![],
            location: ExecutionLocation::Local,
        };

        assert_eq!(desc.task_id, task_id);
        assert_eq!(desc.command[0], "echo");
        assert!(!desc.is_critical);
        assert_eq!(desc.priority, Priority::Normal);
    }

    #[test]
    fn test_task_descriptor_with_critical_priority() {
        let task_id = TaskId::new();
        let desc = TaskDescriptor {
            task_id,
            command: vec!["critical-task".to_string()],
            working_dir: PathBuf::from("/tmp"),
            env: HashMap::new(),
            resources: ResourceAllocation::default(),
            policy: RetryPolicy::default(),
            attempt: 0,
            is_critical: true,
            priority: Priority::Critical,
            dependencies: vec![],
            location: ExecutionLocation::Local,
        };

        assert!(desc.is_critical);
        assert_eq!(desc.priority, Priority::Critical);
        assert!(desc.priority.is_critical());
    }

    #[test]
    fn task_descriptor_location_defaults_to_local() {
        let task_id = TaskId::new();
        let desc = TaskDescriptor {
            task_id,
            command: vec!["echo".to_string()],
            working_dir: PathBuf::from("/tmp"),
            env: HashMap::new(),
            resources: ResourceAllocation::default(),
            policy: RetryPolicy::default(),
            attempt: 0,
            is_critical: false,
            priority: Priority::Normal,
            dependencies: vec![],
            location: ExecutionLocation::default(),
        };

        assert_eq!(desc.location, ExecutionLocation::Local);
    }

    #[test]
    fn task_descriptor_with_remote_location_serializes() {
        let task_id = TaskId::new();
        let target = crate::execution_location::RemoteSessionTarget {
            host: "aivcs.local".to_string(),
            user: "aivcs".to_string(),
            repo_name: "my-repo".to_string(),
        };
        let desc = TaskDescriptor {
            task_id,
            command: vec!["echo".to_string()],
            working_dir: PathBuf::from("/tmp"),
            env: HashMap::new(),
            resources: ResourceAllocation::default(),
            policy: RetryPolicy::default(),
            attempt: 0,
            is_critical: false,
            priority: Priority::Normal,
            dependencies: vec![],
            location: ExecutionLocation::RemoteSession(target),
        };

        let json = serde_json::to_string(&desc).unwrap();
        let deserialized: TaskDescriptor = serde_json::from_str(&json).unwrap();
        assert_eq!(desc.location, deserialized.location);
    }

    #[test]
    fn remote_session_manager_trait_is_object_safe() {
        // This test ensures RemoteSessionManagerTrait can be used as a trait object.
        // If this compiles, the trait is object-safe.
        let _: Option<Box<dyn RemoteSessionManagerTrait>> = None;
    }

    #[test]
    fn session_config_creation() {
        let config = SessionConfig {
            host: "aivcs.local".to_string(),
            user: "aivcs".to_string(),
            repo_name: "my-repo".to_string(),
        };

        assert_eq!(config.host, "aivcs.local");
        assert_eq!(config.user, "aivcs");
        assert_eq!(config.repo_name, "my-repo");
    }
}
