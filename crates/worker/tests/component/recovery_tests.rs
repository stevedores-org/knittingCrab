use knitting_crab_core::ids::WorkerId;
use knitting_crab_core::resource::ResourceAllocation;
use knitting_crab_core::retry::RetryPolicy;
use knitting_crab_core::traits::EventSink;
use knitting_crab_core::TaskDescriptor;
use knitting_crab_worker::fake_worker::{FakeBehavior, FakeWorker};
use knitting_crab_worker::WorkerRuntime;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn crashed_worker_tasks_recovered() {
    let queue = FakeWorker::new();
    let lease_store = knitting_crab_worker::InMemoryLeaseStore::new();
    let resource_monitor = FakeWorker::new();
    let event_sink = FakeWorker::new();
    let process_executor = FakeWorker::new();

    let task_id = knitting_crab_core::TaskId::new();
    process_executor.set_behavior(task_id, FakeBehavior::Crash);

    let task = TaskDescriptor {
        task_id,
        command: vec!["crash".to_string()],
        working_dir: PathBuf::from("/tmp"),
        env: Default::default(),
        resources: ResourceAllocation::default(),
        policy: RetryPolicy::default(),
        attempt: 0,
    };
    queue.enqueue(task.clone());

    let worker_id = WorkerId::new();
    let runtime = WorkerRuntime::new(
        worker_id,
        queue.clone(),
        lease_store,
        resource_monitor,
        Arc::new(event_sink.clone()) as Arc<dyn EventSink>,
        process_executor,
    );

    // Execute the task (should fail)
    if let Ok(Some(dequeued_task)) = queue.dequeue(worker_id).await {
        let _ = runtime.execute_task(&dequeued_task).await;
    }

    // Check that it was marked for retry
    let events = event_sink.drain_events();
    let will_retry_count = events
        .iter()
        .filter(|e| matches!(e, knitting_crab_core::TaskEvent::WillRetry { .. }))
        .count();

    assert!(will_retry_count > 0, "expected task to be retried");
}

#[tokio::test(flavor = "multi_thread")]
async fn hung_task_killed_and_retried() {
    let queue = FakeWorker::new();
    let lease_store = knitting_crab_worker::InMemoryLeaseStore::new();
    let resource_monitor = FakeWorker::new();
    let event_sink = FakeWorker::new();
    let process_executor = FakeWorker::new();

    let task_id = knitting_crab_core::TaskId::new();
    process_executor.set_behavior(task_id, FakeBehavior::Hang);

    let task = TaskDescriptor {
        task_id,
        command: vec!["hang".to_string()],
        working_dir: PathBuf::from("/tmp"),
        env: Default::default(),
        resources: ResourceAllocation::default(),
        policy: RetryPolicy::default(),
        attempt: 0,
    };
    queue.enqueue(task.clone());

    let worker_id = WorkerId::new();
    let runtime = WorkerRuntime::new(
        worker_id,
        queue.clone(),
        lease_store,
        resource_monitor,
        Arc::new(event_sink.clone()) as Arc<dyn EventSink>,
        process_executor,
    )
    .with_lease_ttl(Duration::from_millis(100));

    // Execute the task
    if let Ok(Some(dequeued_task)) = queue.dequeue(worker_id).await {
        let _ = runtime.execute_task(&dequeued_task).await;
    }

    // Wait for events to settle
    tokio::time::sleep(Duration::from_millis(200)).await;

    let events = event_sink.drain_events();
    let completed_count = events
        .iter()
        .filter(|e| matches!(e, knitting_crab_core::TaskEvent::Completed { .. }))
        .count();

    assert!(completed_count > 0, "expected task to complete after cancel");
}
