use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use knitting_crab_core::{EventSink, ExecutionLocation, Queue};
use knitting_crab_core::{
    ExitOutcome, Priority, ResourceAllocation, RetryPolicy, TaskDescriptor, TaskId, WorkerId,
};
use knitting_crab_worker::fake_worker::FakeBehavior;
use knitting_crab_worker::{CancelToken, FakeWorker, ProcessExecutor, SpawnParams};

fn make_descriptor(task_id: TaskId) -> TaskDescriptor {
    TaskDescriptor {
        task_id,
        command: vec!["echo".to_string(), "hello".to_string()],
        working_dir: PathBuf::from("."),
        env: HashMap::new(),
        resources: ResourceAllocation::default(),
        policy: RetryPolicy::default(),
        attempt: 0,
        is_critical: false,
        priority: Priority::Normal,
        dependencies: vec![],
        location: Default::default(),
    }
}

#[tokio::test]
async fn queue_round_trip_works_with_fake_worker() {
    let worker = FakeWorker::new();
    let task_id = TaskId::new();
    let descriptor = make_descriptor(task_id);

    worker.enqueue(descriptor.clone());

    let dequeued = worker
        .dequeue(WorkerId::new())
        .await
        .expect("dequeue should not fail")
        .expect("task should exist in queue");

    assert_eq!(dequeued.task_id, descriptor.task_id);
    assert_eq!(dequeued.command, descriptor.command);
}

#[tokio::test]
async fn process_executor_respects_configured_fake_behavior() {
    let worker = FakeWorker::new();
    let task_id = TaskId::new();

    worker.set_behavior(
        task_id,
        FakeBehavior::Fail {
            exit_code: 7,
            delay_ms: 1,
        },
    );

    let params = SpawnParams {
        task_id,
        command: vec!["echo".to_string(), "ignored".to_string()],
        working_dir: PathBuf::from("."),
        env: HashMap::new(),
        location: ExecutionLocation::default(),
    };

    let (cancel_token, cancel_guard) = CancelToken::new();
    let sink: Arc<dyn EventSink> = Arc::new(worker.clone());
    let result = worker
        .execute(params, sink, cancel_guard)
        .await
        .expect("fake execution should return an exit outcome");

    drop(cancel_token);

    assert_eq!(result, ExitOutcome::FailedWithCode(7));
}
