//! Fake remote session manager for testing.

use async_trait::async_trait;
use knitting_crab_core::error::CoreError;
use knitting_crab_core::traits::{RemoteSessionManagerTrait, SessionConfig};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Call record for test verification.
#[derive(Debug, Clone)]
pub struct CallRecord {
    pub method: String,
    pub session_name: Option<String>,
    pub window_name: Option<String>,
    pub sentinel_path: Option<String>,
}

/// Configuration for exit code behavior.
#[derive(Debug, Clone, Default)]
pub enum ExitCodeBehavior {
    /// Command succeeds immediately.
    #[default]
    Success,
    /// Command fails with specific code.
    FailCode(i32),
    /// Command never completes (for timeout testing).
    NeverComplete,
    /// Fail on ensure_session.
    FailEnsureSession(String),
}

/// Fake remote session manager for testing.
pub struct FakeRemoteSessionManager {
    calls: Arc<Mutex<Vec<CallRecord>>>,
    behaviors: Arc<Mutex<HashMap<String, ExitCodeBehavior>>>,
    default_behavior: ExitCodeBehavior,
}

impl FakeRemoteSessionManager {
    pub fn new() -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            behaviors: Arc::new(Mutex::new(HashMap::new())),
            default_behavior: ExitCodeBehavior::Success,
        }
    }

    pub fn with_behavior(self, session_name: String, behavior: ExitCodeBehavior) -> Self {
        self.behaviors
            .lock()
            .unwrap()
            .insert(session_name, behavior);
        self
    }

    pub fn drain_calls(&self) -> Vec<CallRecord> {
        self.calls.lock().unwrap().drain(..).collect()
    }
}

impl Default for FakeRemoteSessionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl RemoteSessionManagerTrait for FakeRemoteSessionManager {
    async fn ensure_session(&self, config: &SessionConfig) -> Result<String, CoreError> {
        let session_name = format!("fake_session_{}_{}", config.user, config.repo_name);
        self.calls.lock().unwrap().push(CallRecord {
            method: "ensure_session".to_string(),
            session_name: Some(session_name.clone()),
            window_name: None,
            sentinel_path: None,
        });

        // Check for failure behavior
        let behaviors = self.behaviors.lock().unwrap();
        if let Some(ExitCodeBehavior::FailEnsureSession(err)) = behaviors.get(&session_name) {
            return Err(CoreError::SessionFailed(err.clone()));
        }

        Ok(session_name)
    }

    async fn run_command_in_session(
        &self,
        session_name: &str,
        window_name: &str,
        _command: &str,
        sentinel_path: &str,
    ) -> Result<(), CoreError> {
        self.calls.lock().unwrap().push(CallRecord {
            method: "run_command_in_session".to_string(),
            session_name: Some(session_name.to_string()),
            window_name: Some(window_name.to_string()),
            sentinel_path: Some(sentinel_path.to_string()),
        });
        Ok(())
    }

    async fn poll_exit_code(&self, sentinel_path: &str) -> Result<Option<i32>, CoreError> {
        self.calls.lock().unwrap().push(CallRecord {
            method: "poll_exit_code".to_string(),
            session_name: None,
            window_name: None,
            sentinel_path: Some(sentinel_path.to_string()),
        });

        // Extract session name from sentinel path to determine behavior
        let behaviors = self.behaviors.lock().unwrap();
        let behavior = behaviors.values().next().unwrap_or(&self.default_behavior);

        match behavior {
            ExitCodeBehavior::Success => Ok(Some(0)),
            ExitCodeBehavior::FailCode(code) => Ok(Some(*code)),
            ExitCodeBehavior::NeverComplete => Ok(None),
            ExitCodeBehavior::FailEnsureSession(_) => Ok(Some(1)),
        }
    }

    async fn cleanup_sentinel(&self, sentinel_path: &str) -> Result<(), CoreError> {
        self.calls.lock().unwrap().push(CallRecord {
            method: "cleanup_sentinel".to_string(),
            session_name: None,
            window_name: None,
            sentinel_path: Some(sentinel_path.to_string()),
        });
        Ok(())
    }

    async fn kill_session(&self, session_name: &str) -> Result<(), CoreError> {
        self.calls.lock().unwrap().push(CallRecord {
            method: "kill_session".to_string(),
            session_name: Some(session_name.to_string()),
            window_name: None,
            sentinel_path: None,
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ensure_session_records_call() {
        let manager = FakeRemoteSessionManager::new();
        let config = SessionConfig {
            host: "aivcs.local".to_string(),
            user: "aivcs".to_string(),
            repo_name: "test-repo".to_string(),
        };

        let session_name = manager.ensure_session(&config).await.unwrap();
        assert!(!session_name.is_empty());

        let calls = manager.drain_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method, "ensure_session");
    }

    #[tokio::test]
    async fn poll_returns_configured_exit_code() {
        let manager = FakeRemoteSessionManager::new()
            .with_behavior("test".to_string(), ExitCodeBehavior::FailCode(42));

        let exit_code = manager.poll_exit_code("sentinel").await.unwrap();
        assert_eq!(exit_code, Some(42));
    }

    #[tokio::test]
    async fn fail_create_propagates_error() {
        let manager = FakeRemoteSessionManager::new().with_behavior(
            "fake_session_aivcs_fail-repo".to_string(),
            ExitCodeBehavior::FailEnsureSession("Connection failed".to_string()),
        );

        let config = SessionConfig {
            host: "aivcs.local".to_string(),
            user: "aivcs".to_string(),
            repo_name: "fail-repo".to_string(),
        };

        let result = manager.ensure_session(&config).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn kill_session_removes_entry() {
        let manager = FakeRemoteSessionManager::new();

        manager.kill_session("test_session").await.unwrap();

        let calls = manager.drain_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method, "kill_session");
    }

    #[tokio::test]
    async fn never_complete_returns_none_forever() {
        let manager = FakeRemoteSessionManager::new()
            .with_behavior("test".to_string(), ExitCodeBehavior::NeverComplete);

        let exit_code1 = manager.poll_exit_code("sentinel").await.unwrap();
        let exit_code2 = manager.poll_exit_code("sentinel").await.unwrap();

        assert_eq!(exit_code1, None);
        assert_eq!(exit_code2, None);
    }
}
