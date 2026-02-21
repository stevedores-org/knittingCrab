use knitting_crab::{FakeWorker, Priority, ResourceModel, Scheduler, TaskResult, WorkItem, Worker};
use std::time::Duration;

fn make_task(id: u64, priority: Priority, deps: Vec<u64>) -> WorkItem {
    WorkItem::new(
        id,
        format!("goal{}", id),
        "repo",
        "main",
        priority,
        deps,
        1,
        512,
        false,
        None,
        3,
    )
}

/// Runs tasks through the scheduler using the given worker until no more tasks are ready.
/// Returns all TaskResults collected.
async fn run_scheduler(sched: &Scheduler, worker: &impl Worker) -> Vec<TaskResult> {
    let mut results = Vec::new();
    while let Some(task) = sched.next_task() {
        let lease = sched
            .schedule_task(task.clone(), 1, Duration::from_secs(60))
            .unwrap();
        let result = worker.execute(&task).await;
        results.push(result.clone());
        sched.complete_task(lease.id, result.success, result.exit_code, result.reason);
    }
    results
}

#[tokio::test]
async fn ten_tasks_all_succeed() {
    let sched = Scheduler::new(ResourceModel::new(8, 8192, 1, 4));
    let worker = FakeWorker::always_succeed(Duration::from_millis(1));

    for i in 1..=10 {
        sched
            .enqueue(make_task(i, Priority::Batch, vec![]))
            .unwrap();
    }

    let results = run_scheduler(&sched, &worker).await;
    assert_eq!(results.len(), 10);
    assert!(results.iter().all(|r| r.success));
}

#[tokio::test]
async fn partial_failure_with_retry() {
    use knitting_crab::worker::FakeOutcome;

    let sched = Scheduler::new(ResourceModel::new(8, 8192, 1, 4));
    // Task 1: fail once then succeed (outcomes consumed in order; empty = succeed)
    let worker = FakeWorker::new(
        Duration::from_millis(1),
        vec![FakeOutcome::Fail {
            exit_code: 1,
            reason: "transient".into(),
        }],
    );

    let mut task = make_task(1, Priority::Batch, vec![]);
    task.max_retries = 2;
    sched.enqueue(task).unwrap();

    // First execution: should fail
    let next = sched.next_task().unwrap();
    let lease = sched
        .schedule_task(next.clone(), 1, Duration::from_secs(60))
        .unwrap();
    let result = worker.execute(&next).await;
    assert!(!result.success);
    sched.complete_task(lease.id, result.success, result.exit_code, result.reason);

    // Task should be re-queued for retry
    let retry = sched.next_task();
    assert!(
        retry.is_some(),
        "Task should be re-queued after retryable failure"
    );

    let retry_task = retry.unwrap();
    let lease2 = sched
        .schedule_task(retry_task.clone(), 1, Duration::from_secs(60))
        .unwrap();
    let result2 = worker.execute(&retry_task).await;
    assert!(result2.success, "Second attempt should succeed");
    sched.complete_task(
        lease2.id,
        result2.success,
        result2.exit_code,
        result2.reason,
    );

    // No more tasks
    assert!(sched.next_task().is_none());
}

#[tokio::test]
async fn resource_saturation() {
    // Only 2 CPU cores — can run at most 2 tasks concurrently (each needs 1 core)
    let sched = Scheduler::new(ResourceModel::new(2, 8192, 1, 4));
    let worker = FakeWorker::always_succeed(Duration::from_millis(1));

    for i in 1..=4 {
        sched
            .enqueue(make_task(i, Priority::Batch, vec![]))
            .unwrap();
    }

    // Schedule 2 tasks — should succeed
    let t1 = sched.next_task().unwrap();
    let l1 = sched
        .schedule_task(t1.clone(), 1, Duration::from_secs(60))
        .unwrap();
    let t2 = sched.next_task().unwrap();
    let l2 = sched
        .schedule_task(t2.clone(), 1, Duration::from_secs(60))
        .unwrap();

    // Third task should not be schedulable (no resources)
    assert!(
        sched.next_task().is_none(),
        "No resources for a third concurrent task"
    );

    // Complete first two, freeing resources
    let r1 = worker.execute(&t1).await;
    sched.complete_task(l1.id, r1.success, r1.exit_code, r1.reason);
    let r2 = worker.execute(&t2).await;
    sched.complete_task(l2.id, r2.success, r2.exit_code, r2.reason);

    // Now remaining tasks should be schedulable
    let remaining = run_scheduler(&sched, &worker).await;
    assert_eq!(remaining.len(), 2);
}

#[tokio::test]
async fn dag_execution_order() {
    // Task 2 depends on task 1; task 3 depends on task 2.
    // Execution must respect: 1 → 2 → 3.
    let sched = Scheduler::new(ResourceModel::new(8, 8192, 1, 4));
    let worker = FakeWorker::always_succeed(Duration::from_millis(1));

    sched
        .enqueue(make_task(1, Priority::Batch, vec![]))
        .unwrap();
    sched
        .enqueue(make_task(2, Priority::Batch, vec![1]))
        .unwrap();
    sched
        .enqueue(make_task(3, Priority::Batch, vec![2]))
        .unwrap();

    let results = run_scheduler(&sched, &worker).await;
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].task_id, 1);
    assert_eq!(results[1].task_id, 2);
    assert_eq!(results[2].task_id, 3);
}
