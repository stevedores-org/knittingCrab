use knitting_crab_core::LogSource;
use knitting_crab_core::ids::TaskId;
use knitting_crab_core::retry::ExitOutcome;
use knitting_crab_core::traits::EventSink;
use knitting_crab_worker::{spawn, SpawnParams};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use async_trait::async_trait;

struct TestSink {
    logs: Mutex<Vec<knitting_crab_core::LogLine>>,
}

#[async_trait]
impl EventSink for TestSink {
    async fn emit_event(
        &self,
        _event: knitting_crab_core::TaskEvent,
    ) -> Result<(), knitting_crab_core::CoreError> {
        Ok(())
    }

    async fn emit_log(
        &self,
        log: knitting_crab_core::LogLine,
    ) -> Result<(), knitting_crab_core::CoreError> {
        self.logs.lock().unwrap().push(log);
        Ok(())
    }
}

#[tokio::test]
async fn spawn_real_subprocess() {
    let sink = std::sync::Arc::new(TestSink {
        logs: Mutex::new(Vec::new()),
    });

    let params = SpawnParams {
        task_id: TaskId::new(),
        command: vec!["echo".to_string(), "hello world".to_string()],
        working_dir: PathBuf::from("/tmp"),
        env: Default::default(),
    };

    let mut handle = spawn(params, sink.clone()).await.unwrap();
    let outcome = handle.wait().await.unwrap();

    assert_eq!(outcome, ExitOutcome::Success);

    // Give log reader time to process
    tokio::time::sleep(Duration::from_millis(100)).await;

    let logs = sink.logs.lock().unwrap();
    assert!(!logs.is_empty());
    let output = logs.iter().map(|l| &l.content).collect::<Vec<_>>();
    assert!(output.iter().any(|s| s.contains("hello world")));
}
