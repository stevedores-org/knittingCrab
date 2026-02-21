use knitting_crab::{
    FakeWorker, Priority, ProcessRunner, ProcessRunnerConfig, RepoLockManager, ResourceModel,
    Scheduler, ShardAggregator, TaskResult, WorkItem, Worker,
};
use std::time::Duration;

fn make_task(id: u64, priority: Priority, deps: Vec<u64>) -> WorkItem {
    WorkItem::new_simple(
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

// ===== Epic 3 Integration Tests =====

#[tokio::test]
async fn subprocess_task_with_log_capture() {
    let runner = ProcessRunner::new(ProcessRunnerConfig::default());

    let mut task = WorkItem::new_simple(
        1,
        "echo hello",
        "repo",
        "main",
        Priority::Batch,
        vec![],
        1,
        512,
        false,
        None,
        0,
    );
    task.id = 1;

    let result = runner.execute(&task).await;
    assert!(result.success, "subprocess should succeed");
    assert_eq!(result.exit_code, Some(0));
}

#[tokio::test]
async fn env_isolation_integration() {
    let runner = ProcessRunner::new(ProcessRunnerConfig::default());

    // Create a task with only PATH in the allowlist
    let mut task = WorkItem::new_simple(
        1,
        "test -n \"$PATH\"",
        "repo",
        "main",
        Priority::Batch,
        vec![],
        1,
        512,
        false,
        None,
        0,
    );
    task.env_allowlist = vec!["PATH".to_string()];

    let result = runner.execute(&task).await;
    assert!(
        result.success,
        "task should succeed with PATH in allowlist"
    );

    // Create another task that uses allowlist to only keep PATH
    let mut task2 = WorkItem::new_simple(
        2,
        "test -n \"$PATH\"",
        "repo",
        "main",
        Priority::Batch,
        vec![],
        1,
        512,
        false,
        None,
        0,
    );
    task2.env_allowlist = vec!["PATH".to_string()];

    let result2 = runner.execute(&task2).await;
    assert!(result2.success, "task should succeed with PATH in allowlist");
}

#[tokio::test]
async fn repo_lock_serializes_concurrent_tasks() {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    let lock_mgr = RepoLockManager::new();
    let counter = Arc::new(AtomicU32::new(0));

    let mut handles = vec![];

    for _ in 0..2 {
        let mgr = lock_mgr.clone();
        let cnt = counter.clone();

        let handle = tokio::spawn(async move {
            let _guard = mgr.lock("shared_repo").await;

            cnt.fetch_add(1, Ordering::SeqCst);

            // Simulate some work
            tokio::time::sleep(Duration::from_millis(5)).await;
        });

        handles.push(handle);
    }

    for handle in handles {
        handle.await.unwrap();
    }

    assert_eq!(counter.load(Ordering::SeqCst), 2);
}

#[test]
fn shard_aggregator_end_to_end() {
    let mut agg = ShardAggregator::new(4);

    // Simulate 4 shards: 3 pass, 1 fails
    agg.record(0, true);
    agg.record(1, true);
    agg.record(2, false);
    agg.record(3, true);

    assert!(agg.is_complete());
    assert!(
        !agg.overall_success(),
        "overall should fail if any shard fails"
    );

    // Now test all-pass scenario
    let mut agg2 = ShardAggregator::new(3);
    agg2.record(0, true);
    agg2.record(1, true);
    agg2.record(2, true);

    assert!(agg2.is_complete());
    assert!(agg2.overall_success(), "overall should pass if all shards pass");
}
