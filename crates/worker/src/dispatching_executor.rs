//! Dispatcher that routes tasks to local or remote executors.

use crate::cancel_token::CancelGuard;
use crate::error::WorkerError;
use crate::process::SpawnParams;
use crate::worker_runtime::ProcessExecutor;
use async_trait::async_trait;
use knitting_crab_core::execution_location::ExecutionLocation;
use knitting_crab_core::retry::ExitOutcome;
use knitting_crab_core::traits::EventSink;
use std::sync::Arc;

/// Dispatcher that routes tasks to local or remote executors.
pub struct DispatchingExecutor {
    local: Arc<dyn ProcessExecutor>,
    remote: Arc<dyn ProcessExecutor>,
}

impl DispatchingExecutor {
    pub fn new(local: Arc<dyn ProcessExecutor>, remote: Arc<dyn ProcessExecutor>) -> Self {
        Self { local, remote }
    }
}

#[async_trait]
impl ProcessExecutor for DispatchingExecutor {
    async fn execute(
        &self,
        params: SpawnParams,
        sink: Arc<dyn EventSink>,
        cancel_guard: CancelGuard,
    ) -> Result<ExitOutcome, WorkerError> {
        match &params.location {
            ExecutionLocation::Local => self.local.execute(params, sink, cancel_guard).await,
            ExecutionLocation::RemoteSession(_) => {
                self.remote.execute(params, sink, cancel_guard).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_worker::FakeWorker;
    use std::path::PathBuf;

    #[tokio::test]
    async fn local_task_routes_to_local_executor() {
        let local = Arc::new(FakeWorker::new());
        let remote = Arc::new(FakeWorker::new());

        let dispatcher = DispatchingExecutor::new(local, remote);

        let params = SpawnParams {
            task_id: knitting_crab_core::TaskId::new(),
            command: vec!["echo".to_string()],
            working_dir: PathBuf::from("/tmp"),
            env: Default::default(),
            location: ExecutionLocation::Local,
        };

        let sink = Arc::new(FakeWorker::new()) as Arc<dyn EventSink>;
        let (_token, guard) = crate::CancelToken::new();

        let outcome = dispatcher.execute(params, sink, guard).await.unwrap();
        assert_eq!(outcome, ExitOutcome::Success);
    }

    #[tokio::test]
    async fn remote_task_routes_to_remote_executor() {
        let local = Arc::new(FakeWorker::new());
        let remote: Arc<dyn ProcessExecutor> = Arc::new(crate::SshTmuxSessionExecutor::new(
            Arc::new(crate::FakeRemoteSessionManager::new()),
        ));

        let dispatcher = DispatchingExecutor::new(local, remote);

        let target = knitting_crab_core::RemoteSessionTarget {
            host: "aivcs.local".to_string(),
            user: "aivcs".to_string(),
            repo_name: "test-repo".to_string(),
        };

        let params = SpawnParams {
            task_id: knitting_crab_core::TaskId::new(),
            command: vec!["echo".to_string()],
            working_dir: PathBuf::from("/tmp"),
            env: Default::default(),
            location: ExecutionLocation::RemoteSession(target),
        };

        let sink = Arc::new(FakeWorker::new()) as Arc<dyn EventSink>;
        let (_token, guard) = crate::CancelToken::new();

        let outcome = dispatcher.execute(params, sink, guard).await.unwrap();
        assert_eq!(outcome, ExitOutcome::Success);
    }
}
