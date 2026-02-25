use async_trait::async_trait;
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::Mutex;

use knitting_crab_core::retry::ExitOutcome;
use knitting_crab_core::traits::{
    ExecutionResult, RemoteSessionConfig, RemoteSessionManager, SessionHandle,
};
use knitting_crab_core::{CoreError, EventSink};

use crate::cancel_token::CancelGuard;
use crate::error::WorkerError;
use crate::process::SpawnParams;
use crate::worker_runtime::ProcessExecutor;

/// Executes processes on a remote session via RemoteSessionManager.
pub struct RemoteProcessExecutor<RSM: RemoteSessionManager> {
    #[allow(dead_code)]
    manager: Arc<RSM>,
}

impl<RSM: RemoteSessionManager> RemoteProcessExecutor<RSM> {
    pub fn new(manager: Arc<RSM>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl<RSM: RemoteSessionManager + 'static> ProcessExecutor for RemoteProcessExecutor<RSM> {
    async fn execute(
        &self,
        _params: SpawnParams,
        _sink: Arc<dyn EventSink>,
        _cancel_guard: CancelGuard,
    ) -> Result<ExitOutcome, WorkerError> {
        // For now, we require that remote config is available
        // In a full implementation, this would come from TaskDescriptor.location
        // This is a placeholder that demonstrates the trait implementation.

        // TODO: Extract RemoteSessionConfig from params or context
        // For this phase, we'll return an error indicating remote execution needs context
        Err(WorkerError::Internal(
            "remote execution requires RemoteSessionConfig from TaskDescriptor.location"
                .to_string(),
        ))
    }
}

/// Fake RemoteSessionManager for testing.
pub struct FakeRemoteSessionManager {
    responses: Arc<Mutex<VecDeque<Result<ExecutionResult, CoreError>>>>,
    sessions: Arc<Mutex<Vec<String>>>,
}

impl FakeRemoteSessionManager {
    pub fn new() -> Self {
        Self {
            responses: Arc::new(Mutex::new(VecDeque::new())),
            sessions: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Inject a response for the next command execution.
    pub async fn inject_response(&self, result: Result<ExecutionResult, CoreError>) {
        self.responses.lock().await.push_back(result);
    }

    /// Get list of created sessions.
    pub async fn created_sessions(&self) -> Vec<String> {
        self.sessions.lock().await.clone()
    }

    /// Inject a success response with the given exit code and output.
    pub async fn inject_success(&self, exit_code: i32, stdout: String, stderr: String) {
        self.inject_response(Ok(ExecutionResult {
            exit_code,
            stdout,
            stderr,
        }))
        .await;
    }

    /// Inject a failure response.
    pub async fn inject_failure(&self, error: CoreError) {
        self.inject_response(Err(error)).await;
    }
}

impl Default for FakeRemoteSessionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl RemoteSessionManager for FakeRemoteSessionManager {
    async fn create_or_attach(
        &self,
        config: &RemoteSessionConfig,
    ) -> Result<SessionHandle, CoreError> {
        let session_name = format!(
            "aivcs__{}__{}__{}",
            config.repo_name,
            config.work_id,
            format!("{:?}", config.role).to_lowercase()
        );
        self.sessions.lock().await.push(session_name.clone());
        Ok(SessionHandle { session_name })
    }

    async fn run_command(
        &self,
        _session: &SessionHandle,
        _cmd: &str,
    ) -> Result<ExecutionResult, CoreError> {
        self.responses.lock().await.pop_front().unwrap_or_else(|| {
            Ok(ExecutionResult {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            })
        })
    }

    async fn kill_session(&self, session: &SessionHandle) -> Result<(), CoreError> {
        let mut sessions = self.sessions.lock().await;
        sessions.retain(|s| s != &session.session_name);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use knitting_crab_core::ids::TaskId;
    use std::path::PathBuf;

    struct TestSink;

    #[async_trait]
    impl EventSink for TestSink {
        async fn emit_event(&self, _event: knitting_crab_core::TaskEvent) -> Result<(), CoreError> {
            Ok(())
        }

        async fn emit_log(&self, _log: knitting_crab_core::LogLine) -> Result<(), CoreError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_fake_session_manager_creation() {
        let manager = FakeRemoteSessionManager::new();
        let config = RemoteSessionConfig {
            host: "test.local".to_string(),
            user: "testuser".to_string(),
            repo_name: "testrepo".to_string(),
            work_id: "job123".to_string(),
            role: knitting_crab_core::RemoteRole::Runner,
        };

        let session = manager.create_or_attach(&config).await.unwrap();
        assert!(session.session_name.contains("testrepo"));
        assert!(session.session_name.contains("job123"));
    }

    #[tokio::test]
    async fn test_fake_session_manager_tracks_sessions() {
        let manager = FakeRemoteSessionManager::new();
        let config = RemoteSessionConfig {
            host: "test.local".to_string(),
            user: "testuser".to_string(),
            repo_name: "testrepo".to_string(),
            work_id: "job123".to_string(),
            role: knitting_crab_core::RemoteRole::Agent,
        };

        let session1 = manager.create_or_attach(&config).await.unwrap();
        let sessions = manager.created_sessions().await;
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0], session1.session_name);
    }

    #[tokio::test]
    async fn test_fake_session_manager_kill_session() {
        let manager = FakeRemoteSessionManager::new();
        let config = RemoteSessionConfig {
            host: "test.local".to_string(),
            user: "testuser".to_string(),
            repo_name: "testrepo".to_string(),
            work_id: "job123".to_string(),
            role: knitting_crab_core::RemoteRole::Runner,
        };

        let session = manager.create_or_attach(&config).await.unwrap();
        let sessions = manager.created_sessions().await;
        assert_eq!(sessions.len(), 1);

        manager.kill_session(&session).await.unwrap();
        let sessions = manager.created_sessions().await;
        assert_eq!(sessions.len(), 0);
    }

    #[tokio::test]
    async fn test_fake_session_manager_injected_responses() {
        let manager = FakeRemoteSessionManager::new();
        let session = SessionHandle {
            session_name: "test_session".to_string(),
        };

        manager
            .inject_success(0, "output".to_string(), "".to_string())
            .await;

        let result = manager.run_command(&session, "echo test").await.unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "output");
    }

    #[tokio::test]
    async fn test_fake_session_manager_default_response() {
        let manager = FakeRemoteSessionManager::new();
        let session = SessionHandle {
            session_name: "test_session".to_string(),
        };

        let result = manager.run_command(&session, "echo test").await.unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "");
    }

    #[tokio::test]
    async fn test_remote_process_executor_placeholder() {
        let manager = Arc::new(FakeRemoteSessionManager::new());
        let executor = RemoteProcessExecutor::new(manager);

        let params = SpawnParams {
            task_id: TaskId::new(),
            command: vec!["echo".to_string(), "test".to_string()],
            working_dir: PathBuf::from("/tmp"),
            env: Default::default(),
        };

        let sink = Arc::new(TestSink);
        let (_token, guard) = crate::cancel_token::CancelToken::new();

        let result = executor.execute(params, sink, guard).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_runner_role_session_name() {
        let manager = FakeRemoteSessionManager::new();
        let config = RemoteSessionConfig {
            host: "test.local".to_string(),
            user: "testuser".to_string(),
            repo_name: "myrepo".to_string(),
            work_id: "work123".to_string(),
            role: knitting_crab_core::RemoteRole::Runner,
        };

        let session = manager.create_or_attach(&config).await.unwrap();
        assert!(session.session_name.contains("runner"));
    }

    #[tokio::test]
    async fn test_agent_role_session_name() {
        let manager = FakeRemoteSessionManager::new();
        let config = RemoteSessionConfig {
            host: "test.local".to_string(),
            user: "testuser".to_string(),
            repo_name: "myrepo".to_string(),
            work_id: "work123".to_string(),
            role: knitting_crab_core::RemoteRole::Agent,
        };

        let session = manager.create_or_attach(&config).await.unwrap();
        assert!(session.session_name.contains("agent"));
    }

    #[tokio::test]
    async fn test_multiple_sessions_tracking() {
        let manager = FakeRemoteSessionManager::new();

        for i in 0..3 {
            let config = RemoteSessionConfig {
                host: "test.local".to_string(),
                user: "testuser".to_string(),
                repo_name: format!("repo{}", i),
                work_id: "job".to_string(),
                role: knitting_crab_core::RemoteRole::Runner,
            };
            let _ = manager.create_or_attach(&config).await.unwrap();
        }

        let sessions = manager.created_sessions().await;
        assert_eq!(sessions.len(), 3);
    }
}
