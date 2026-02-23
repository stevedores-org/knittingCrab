use async_trait::async_trait;
use knitting_crab_core::event::{LogLine, TaskEvent};
use knitting_crab_core::execution_location::ExecutionLocation;
use knitting_crab_core::ids::TaskId;
use knitting_crab_core::traits::EventSink;
use knitting_crab_worker::process::{spawn, SpawnParams};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

struct NoOpSink;

#[async_trait]
impl EventSink for NoOpSink {
    async fn emit_event(
        &self,
        _event: TaskEvent,
    ) -> Result<(), knitting_crab_core::error::CoreError> {
        Ok(())
    }
    async fn emit_log(&self, _log: LogLine) -> Result<(), knitting_crab_core::error::CoreError> {
        Ok(())
    }
}

#[tokio::test]
#[cfg(unix)]
async fn test_wait_cancellation_deadlock() {
    let sink = Arc::new(NoOpSink);
    let params = SpawnParams {
        task_id: TaskId::new(),
        command: vec!["sleep".to_string(), "10".to_string()],
        working_dir: std::env::temp_dir(),
        env: Default::default(),
        location: ExecutionLocation::default(),
    };

    let mut handle = spawn(params, sink).await.expect("Failed to spawn process");

    // Simulate the worker runtime loop: wait for process or cancellation
    let cancel_result = timeout(Duration::from_millis(100), handle.wait()).await;

    // We expect a timeout because the process sleeps for 10s
    assert!(
        cancel_result.is_err(),
        "Expected timeout/cancellation of wait()"
    );

    println!("Wait cancelled, attempting to kill gracefully...");

    // Now try to kill it. If deadlock exists, this will hang.
    // We set a timeout on kill_gracefully to detect the deadlock.
    let kill_result = timeout(
        Duration::from_secs(2),
        handle.kill_gracefully(Duration::from_millis(100)),
    )
    .await;

    match kill_result {
        Ok(Ok(_)) => println!("Successfully killed process"),
        Ok(Err(e)) => panic!("Kill failed: {}", e),
        Err(_) => panic!("Deadlock detected: kill_gracefully timed out!"),
    }
}
