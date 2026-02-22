use knitting_crab_core::ids::WorkerId;
use knitting_crab_core::resource::ResourceAllocation;
use knitting_crab_core::retry::{ExitOutcome, RetryPolicy};
use knitting_crab_core::traits::EventSink;
use knitting_crab_core::TaskDescriptor;
use knitting_crab_worker::fake_worker::{FakeBehavior, FakeWorker};
use knitting_crab_worker::WorkerRuntime;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn work_converges_to_completion_under_load() {
    let queue = FakeWorker::new();
    let lease_store = knitting_crab_worker::InMemoryLeaseStore::new();
    let resource_monitor = FakeWorker::new();
    let event_sink = FakeWorker::new();
    let process_executor = FakeWorker::new();

    // Set up behaviors for all tasks
    for i in 0..5 {
        let task_id = knitting_crab_core::TaskId::new();
        process_executor.set_behavior(
            task_id,
            FakeBehavior::Succeed {
                delay_ms: 10 + i * 5,
            },
        );

        let task = TaskDescriptor {
            task_id,
            command: vec!["echo".to_string(), format!("task-{}", i)],
            working_dir: PathBuf::from("/tmp"),
            env: Default::default(),
            resources: ResourceAllocation::default(),
            policy: RetryPolicy::default(),
            attempt: 0,
            is_critical: false,
            priority: knitting_crab_core::Priority::Normal,
            dependencies: vec![],
        };
        queue.enqueue(task);
    }

    let worker_id = WorkerId::new();
    let runtime = WorkerRuntime::new(
        worker_id,
        queue.clone(),
        lease_store,
        resource_monitor,
        Arc::new(event_sink.clone()) as Arc<dyn EventSink>,
        process_executor,
    );

    // Run the worker loop for a bit
    let worker_handle = tokio::spawn({
        let runtime = &runtime;
        async move {
            for _ in 0..50 {
                if let Ok(Some(task)) = queue.dequeue(worker_id).await {
                    let _ = runtime.execute_task(&task).await;
                } else {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        }
    });

    tokio::time::timeout(Duration::from_secs(5), worker_handle)
        .await
        .unwrap()
        .unwrap();

    // Check that tasks completed
    let events = event_sink.drain_events();
    let completed_count = events
        .iter()
        .filter(|e| matches!(e, knitting_crab_core::TaskEvent::Completed { .. }))
        .count();

    assert!(completed_count > 0, "expected some tasks to complete");
}
