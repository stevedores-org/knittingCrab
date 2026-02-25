//! SSH+tmux process executor for remote session execution.

use crate::process::SpawnParams;
use async_trait::async_trait;
use knitting_crab_core::ids::TaskId;
use knitting_crab_core::retry::ExitOutcome;
use knitting_crab_core::traits::{EventSink, RemoteSessionManagerTrait, SessionConfig};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::{sleep, timeout};
use tracing::{debug, error};

use crate::cancel_token::CancelGuard;
use crate::error::WorkerError;
use crate::worker_runtime::ProcessExecutor;

const POLL_INTERVAL_MS: u64 = 500;
const POLL_TIMEOUT_SECS: u64 = 3600; // 1 hour default timeout

/// SSH+tmux session executor for remote process execution.
pub struct SshTmuxSessionExecutor {
    session_manager: Arc<dyn RemoteSessionManagerTrait>,
}

impl SshTmuxSessionExecutor {
    pub fn new(session_manager: Arc<dyn RemoteSessionManagerTrait>) -> Self {
        Self { session_manager }
    }

    /// Generate a deterministic session name for a task.
    fn session_name_for_task(task_id: TaskId) -> String {
        format!("kc_{}", task_id.to_string().replace("-", "_"))
    }

    /// Generate sentinel file path for exit code polling.
    fn sentinel_path_for_task(task_id: TaskId) -> String {
        let task_hex = task_id.to_string().replace("-", "");
        format!("/tmp/kc_exit_{}", &task_hex[..12])
    }
}

#[async_trait]
impl ProcessExecutor for SshTmuxSessionExecutor {
    async fn execute(
        &self,
        params: SpawnParams,
        _sink: Arc<dyn EventSink>,
        mut cancel_guard: CancelGuard,
    ) -> Result<ExitOutcome, WorkerError> {
        let task_id = params.task_id;
        let session_name = Self::session_name_for_task(task_id);
        let sentinel_path = Self::sentinel_path_for_task(task_id);
        let window_name = "main";

        // Ensure session exists
        let config = SessionConfig {
            host: "aivcs.local".to_string(),
            user: "aivcs".to_string(),
            repo_name: "unknown".to_string(), // TODO: pass from params
        };

        self.session_manager
            .ensure_session(&config)
            .await
            .map_err(|e| WorkerError::Internal(format!("ensure_session failed: {}", e)))?;

        debug!("Created session: {}", session_name);

        // Build command with sentinel file wrapper
        let command_str = params.command.join(" ");
        let wrapped_command = format!("sh -c '({}); echo $? > {}'", command_str, sentinel_path);

        // Run command in session
        self.session_manager
            .run_command_in_session(&session_name, window_name, &wrapped_command, &sentinel_path)
            .await
            .map_err(|e| WorkerError::Internal(format!("run_command_in_session failed: {}", e)))?;

        debug!("Ran command in session: {}", session_name);

        // Poll for exit code
        let outcome = tokio::select! {
            result = self.poll_for_exit(&sentinel_path) => result,
            _ = cancel_guard.cancelled() => {
                // On cancellation, kill the session
                let _ = self.session_manager.kill_session(&session_name).await;
                Ok(ExitOutcome::FailedWithCode(137)) // SIGKILL
            }
        };

        // Cleanup sentinel file
        let _ = self.session_manager.cleanup_sentinel(&sentinel_path).await;

        outcome
    }
}

impl SshTmuxSessionExecutor {
    /// Poll for exit code via sentinel file.
    async fn poll_for_exit(&self, sentinel_path: &str) -> Result<ExitOutcome, WorkerError> {
        let poll_interval = Duration::from_millis(POLL_INTERVAL_MS);
        let timeout_duration = Duration::from_secs(POLL_TIMEOUT_SECS);

        let result = timeout(timeout_duration, async {
            loop {
                match self.session_manager.poll_exit_code(sentinel_path).await {
                    Ok(Some(code)) => {
                        return if code == 0 {
                            ExitOutcome::Success
                        } else {
                            ExitOutcome::FailedWithCode(code)
                        };
                    }
                    Ok(None) => {
                        // Not ready yet, sleep and retry
                        sleep(poll_interval).await;
                    }
                    Err(e) => {
                        error!("Failed to poll exit code: {}", e);
                        return ExitOutcome::FailedWithCode(1);
                    }
                }
            }
        })
        .await;

        match result {
            Ok(outcome) => Ok(outcome),
            Err(_) => {
                // Timeout
                Ok(ExitOutcome::FailedWithCode(124)) // timeout exit code
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_session_manager::{ExitCodeBehavior, FakeRemoteSessionManager};

    #[tokio::test]
    async fn success_path_returns_exit_outcome_success() {
        let fake_manager = Arc::new(
            FakeRemoteSessionManager::new()
                .with_behavior("kc_".to_string(), ExitCodeBehavior::Success),
        );
        let executor = SshTmuxSessionExecutor::new(fake_manager);

        let params = SpawnParams {
            task_id: TaskId::new(),
            command: vec!["echo".to_string(), "hello".to_string()],
            working_dir: std::path::PathBuf::from("/tmp"),
            env: Default::default(),
            location: knitting_crab_core::ExecutionLocation::default(),
        };

        let sink = Arc::new(crate::fake_worker::FakeWorker::new()) as Arc<dyn EventSink>;
        let (_token, guard) = crate::CancelToken::new();

        let outcome = executor.execute(params, sink, guard).await.unwrap();
        assert_eq!(outcome, ExitOutcome::Success);
    }

    #[tokio::test]
    async fn nonzero_exit_code_returns_failed_with_code() {
        let fake_manager = Arc::new(
            FakeRemoteSessionManager::new()
                .with_behavior("kc_".to_string(), ExitCodeBehavior::FailCode(42)),
        );
        let executor = SshTmuxSessionExecutor::new(fake_manager);

        let params = SpawnParams {
            task_id: TaskId::new(),
            command: vec!["exit".to_string(), "42".to_string()],
            working_dir: std::path::PathBuf::from("/tmp"),
            env: Default::default(),
            location: knitting_crab_core::ExecutionLocation::default(),
        };

        let sink = Arc::new(crate::fake_worker::FakeWorker::new()) as Arc<dyn EventSink>;
        let (_token, guard) = crate::CancelToken::new();

        let outcome = executor.execute(params, sink, guard).await.unwrap();
        match outcome {
            ExitOutcome::FailedWithCode(code) => assert_eq!(code, 42),
            _ => panic!("Expected FailedWithCode(42)"),
        }
    }

    #[tokio::test]
    async fn cancellation_kills_session() {
        let fake_manager = Arc::new(
            FakeRemoteSessionManager::new()
                .with_behavior("kc_".to_string(), ExitCodeBehavior::NeverComplete),
        );
        let executor = SshTmuxSessionExecutor::new(fake_manager.clone());

        let params = SpawnParams {
            task_id: TaskId::new(),
            command: vec!["sleep".to_string(), "100".to_string()],
            working_dir: std::path::PathBuf::from("/tmp"),
            env: Default::default(),
            location: knitting_crab_core::ExecutionLocation::default(),
        };

        let sink = Arc::new(crate::fake_worker::FakeWorker::new()) as Arc<dyn EventSink>;
        let (token, guard) = crate::CancelToken::new();

        // Cancel immediately
        token.cancel();

        let outcome = executor.execute(params, sink, guard).await.unwrap();
        assert_eq!(outcome, ExitOutcome::FailedWithCode(137));
    }

    #[tokio::test]
    async fn sentinel_removed_on_success() {
        let fake_manager = Arc::new(
            FakeRemoteSessionManager::new()
                .with_behavior("kc_".to_string(), ExitCodeBehavior::Success),
        );
        let executor = SshTmuxSessionExecutor::new(fake_manager.clone());

        let params = SpawnParams {
            task_id: TaskId::new(),
            command: vec!["echo".to_string()],
            working_dir: std::path::PathBuf::from("/tmp"),
            env: Default::default(),
            location: knitting_crab_core::ExecutionLocation::default(),
        };

        let sink = Arc::new(crate::fake_worker::FakeWorker::new()) as Arc<dyn EventSink>;
        let (_token, guard) = crate::CancelToken::new();

        let _ = executor.execute(params, sink, guard).await;

        // Verify cleanup_sentinel was called
        let calls = fake_manager.drain_calls();
        let cleanup_calls: Vec<_> = calls
            .iter()
            .filter(|c| c.method == "cleanup_sentinel")
            .collect();
        assert!(!cleanup_calls.is_empty());
    }

    #[tokio::test]
    async fn sentinel_removed_on_failure() {
        let fake_manager = Arc::new(
            FakeRemoteSessionManager::new()
                .with_behavior("kc_".to_string(), ExitCodeBehavior::FailCode(1)),
        );
        let executor = SshTmuxSessionExecutor::new(fake_manager.clone());

        let params = SpawnParams {
            task_id: TaskId::new(),
            command: vec!["false".to_string()],
            working_dir: std::path::PathBuf::from("/tmp"),
            env: Default::default(),
            location: knitting_crab_core::ExecutionLocation::default(),
        };

        let sink = Arc::new(crate::fake_worker::FakeWorker::new()) as Arc<dyn EventSink>;
        let (_token, guard) = crate::CancelToken::new();

        let _ = executor.execute(params, sink, guard).await;

        // Verify cleanup_sentinel was called
        let calls = fake_manager.drain_calls();
        let cleanup_calls: Vec<_> = calls
            .iter()
            .filter(|c| c.method == "cleanup_sentinel")
            .collect();
        assert!(!cleanup_calls.is_empty());
    }

    #[tokio::test]
    async fn poll_timeout_returns_timed_out() {
        let fake_manager = Arc::new(
            FakeRemoteSessionManager::new()
                .with_behavior("kc_".to_string(), ExitCodeBehavior::NeverComplete),
        );
        let executor = SshTmuxSessionExecutor::new(fake_manager);

        let task_id = TaskId::new();
        let sentinel_path = SshTmuxSessionExecutor::sentinel_path_for_task(task_id);

        // Use a shorter timeout for testing
        let result = timeout(
            Duration::from_millis(100),
            executor.poll_for_exit(&sentinel_path),
        )
        .await;

        assert!(result.is_err()); // Should timeout
    }
}
