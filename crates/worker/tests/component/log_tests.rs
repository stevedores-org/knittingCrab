use knitting_crab_core::LogSource;
use knitting_crab_worker::FakeWorker;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test]
async fn log_stream_is_ordered_and_lossless() {
    let sink = Arc::new(FakeWorker::new());

    let task_id = knitting_crab_core::TaskId::new();

    // Emit logs in order
    for i in 0..10 {
        let log = knitting_crab_core::LogLine::new(
            task_id,
            i,
            LogSource::Stdout,
            format!("log line {}", i),
        );
        sink.emit_log(log).await.unwrap();
    }

    tokio::time::sleep(Duration::from_millis(100)).await;

    let logs = sink.drain_logs();
    assert_eq!(logs.len(), 10);

    // Check that sequences are ordered and consecutive
    for (i, log) in logs.iter().enumerate() {
        assert_eq!(log.seq, i as u64);
        assert_eq!(log.content, format!("log line {}", i));
    }
}
