use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

use knitting_crab::{
    ArtifactCache, CacheEntry, CacheKey, LeaseManager, Plan, Priority, ResourceBudget,
    ResourceModel, Scheduler, SchedulerPolicy, WorkItem, WorkItemBuilder,
};

// ── Helpers ───────────────────────────────────────────────────────────────────

fn simple_item(name: &str, priority: Priority) -> WorkItem {
    WorkItemBuilder::new(name, "repo", "main", vec!["echo".to_owned(), "hi".to_owned()])
        .priority(priority)
        .resource_budget(ResourceBudget::new(1.0, 512, 0))
        .timeout_secs(30)
        .build()
}

fn item_with_deps(name: &str, deps: Vec<String>) -> WorkItem {
    WorkItemBuilder::new(
        name,
        "repo",
        "main",
        vec!["echo".to_owned(), name.to_owned()],
    )
    .resource_budget(ResourceBudget::new(1.0, 256, 0))
    .timeout_secs(30)
    .dependencies(deps)
    .build()
}

// ── DAG Tests ─────────────────────────────────────────────────────────────────

/// A DAG with a cycle A → B → A should fail validation.
#[tokio::test]
async fn test_dag_cycle_detection() {
    let mut plan = Plan::new();
    let a = simple_item("A", Priority::Normal);
    let b = simple_item("B", Priority::Normal);
    let a_id = a.id.clone();
    let b_id = b.id.clone();

    plan.add_task(a).unwrap();
    plan.add_task(b).unwrap();
    plan.add_dependency(&a_id, &b_id).unwrap(); // A depends on B
    plan.add_dependency(&b_id, &a_id).unwrap(); // B depends on A → cycle

    assert!(
        matches!(
            plan.validate(),
            Err(knitting_crab::SchedulerError::CycleDetected)
        ),
        "Expected CycleDetected error"
    );
}

/// In a linear chain A → B → C only A should be ready initially.
#[tokio::test]
async fn test_dag_ready_tasks() {
    let mut plan = Plan::new();

    let a = item_with_deps("A", vec![]);
    let b_deps = vec![a.id.clone()];
    let b = item_with_deps("B", b_deps.clone());
    let c_deps = vec![b.id.clone()];
    let c = item_with_deps("C", c_deps);

    let a_id = a.id.clone();
    let b_id = b.id.clone();
    let c_id = c.id.clone();
    plan.add_task(a).unwrap();
    plan.add_task(b).unwrap();
    plan.add_task(c).unwrap();
    // A → B means A is a dep of B (B depends on A).
    plan.add_dependency(&b_id, &a_id).unwrap();
    plan.add_dependency(&c_id, &b_id).unwrap();

    let completed: HashSet<String> = HashSet::new();
    let ready = plan.ready_tasks(&completed);
    assert_eq!(ready.len(), 1);
    assert_eq!(ready[0].id, a_id);

    // After A completes, B should be ready.
    let mut completed = HashSet::new();
    completed.insert(a_id.clone());
    let ready = plan.ready_tasks(&completed);
    let names: Vec<&str> = ready.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"B"), "B should be ready after A");

    // After A and B complete, C should be ready.
    completed.insert(b_id.clone());
    let ready = plan.ready_tasks(&completed);
    let names: Vec<&str> = ready.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"C"), "C should be ready after A and B");
    let _ = c_id; // suppress unused-variable warning
}

// ── Resource Tests ────────────────────────────────────────────────────────────

/// Allocating resources reduces availability; releasing restores it.
#[tokio::test]
async fn test_resource_model_allocation() {
    let mut rm = ResourceModel::new(4.0, 8192, 2);
    let budget = ResourceBudget::new(4.0, 8192, 2);

    assert!(rm.can_allocate(&budget));
    rm.allocate(&budget).unwrap();

    // Now fully used – cannot allocate even 1 core.
    assert!(!rm.can_allocate(&ResourceBudget::new(1.0, 1, 0)));

    rm.release(&budget);
    assert!(rm.can_allocate(&budget));
}

// ── Lease Tests ───────────────────────────────────────────────────────────────

/// A lease with a 1-second TTL should be expired after 2 seconds.
#[tokio::test]
async fn test_lease_expiry() {
    let lease = knitting_crab::Lease::new("task-1", "worker-1", 1, 60);
    assert!(!lease.is_expired(), "Should not be expired immediately");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(lease.is_expired(), "Should be expired after 2s");
}

/// Renewing a heartbeat should prevent `heartbeat_overdue` from firing.
#[tokio::test]
async fn test_heartbeat_renewal() {
    let mut lease = knitting_crab::Lease::new("task-2", "worker-2", 300, 2);
    // Sleep just under the heartbeat interval.
    tokio::time::sleep(Duration::from_millis(500)).await;
    lease.renew_heartbeat();
    // After renewal the clock resets; overdue should be false.
    assert!(!lease.heartbeat_overdue(), "Should not be overdue after renewal");
}

// ── Policy Tests ──────────────────────────────────────────────────────────────

/// Applying aging repeatedly must increase `priority_age_bonus`.
#[tokio::test]
async fn test_policy_aging() {
    let policy = SchedulerPolicy::default(); // aging_interval_ticks = 10
    let mut tasks = vec![
        simple_item("T1", Priority::Normal),
        simple_item("T2", Priority::Normal),
    ];

    // Apply many ticks so the bonus is guaranteed to increase.
    for tick in 0..100_u32 {
        policy.apply_aging(&mut tasks, tick);
    }

    assert!(
        tasks[0].priority_age_bonus > 0,
        "Age bonus should have increased"
    );
}

/// Exponential back-off: retry 0 = base, 1 = 2×base, 2 = 4×base.
#[tokio::test]
async fn test_policy_backoff() {
    let policy = SchedulerPolicy::default(); // retry_backoff_base_secs = 2
    let base = policy.retry_backoff_base_secs;

    assert_eq!(policy.compute_backoff(0), Duration::from_secs(base));
    assert_eq!(policy.compute_backoff(1), Duration::from_secs(base * 2));
    assert_eq!(policy.compute_backoff(2), Duration::from_secs(base * 4));
}

// ── Cache Tests ───────────────────────────────────────────────────────────────

/// Putting and getting a cache entry by the same key should succeed; a
/// different key should return `None`.
#[tokio::test]
async fn test_cache_put_get() {
    let cache = ArtifactCache::new(100, 3600);
    let env: HashMap<String, String> = HashMap::new();

    let key = CacheKey::new("abc123", &["cargo".to_owned(), "test".to_owned()], &env, "1.78");
    let entry = CacheEntry {
        key_hash: key.to_hash(),
        created_at: SystemTime::now(),
        exit_code: 0,
        stdout_snippet: "all tests passed".to_owned(),
        artifacts: vec![],
    };

    cache.put(key.clone(), entry);
    assert!(cache.get(&key).is_some(), "Should hit cache");

    let other_key = CacheKey::new("different", &["cargo".to_owned(), "test".to_owned()], &env, "1.78");
    assert!(cache.get(&other_key).is_none(), "Different key should miss");
}

/// An entry whose retention period has elapsed should be removed by
/// `evict_expired`.
#[tokio::test]
async fn test_cache_eviction() {
    let cache = ArtifactCache::new(100, 1); // 1-second retention
    let env: HashMap<String, String> = HashMap::new();
    let key = CacheKey::new("sha", &["cmd".to_owned()], &env, "v1");
    let entry = CacheEntry {
        key_hash: key.to_hash(),
        created_at: SystemTime::now(),
        exit_code: 0,
        stdout_snippet: String::new(),
        artifacts: vec![],
    };
    cache.put(key, entry);
    assert_eq!(cache.len(), 1);

    tokio::time::sleep(Duration::from_secs(2)).await;
    cache.evict_expired();
    assert!(cache.is_empty(), "Cache should be empty after eviction");
}

// ── Scheduler Tests ───────────────────────────────────────────────────────────

/// Submitting two tasks where task2 depends on task1: the first tick should
/// dispatch only task1; after completing task1, the second tick dispatches
/// task2.
#[tokio::test]
async fn test_scheduler_submit_and_tick() {
    let policy = SchedulerPolicy::default();
    let resources = ResourceModel::new(8.0, 16384, 0);
    let scheduler = Scheduler::new(policy, resources);

    let task1 = item_with_deps("Task1", vec![]);
    let task1_id = task1.id.clone();

    let task2 = item_with_deps("Task2", vec![task1_id.clone()]);
    let task2_id = task2.id.clone();

    scheduler.submit(task1).unwrap();
    scheduler.submit(task2).unwrap();

    // First tick: only task1 is ready (task2 depends on it).
    let dispatched = scheduler.tick().unwrap();
    assert_eq!(dispatched.len(), 1);
    assert_eq!(dispatched[0].id, task1_id);

    // Mark task1 complete.
    scheduler.complete_task(&task1_id, 0).unwrap();

    // Second tick: task2 should now be dispatched.
    let dispatched = scheduler.tick().unwrap();
    assert_eq!(dispatched.len(), 1);
    assert_eq!(dispatched[0].id, task2_id);
}

/// Locking the same goal twice should yield `GoalAlreadyLocked`.
#[tokio::test]
async fn test_goal_lock_prevents_duplicate() {
    let manager = LeaseManager::new();
    manager.lock_goal("myrepo", "main", "abc123").unwrap();

    let result = manager.lock_goal("myrepo", "main", "abc123");
    assert!(
        matches!(
            result,
            Err(knitting_crab::SchedulerError::GoalAlreadyLocked { .. })
        ),
        "Expected GoalAlreadyLocked"
    );
}

/// `stats()` should reflect submitted and completed task counts.
#[tokio::test]
async fn test_scheduler_stats() {
    let policy = SchedulerPolicy::default();
    let resources = ResourceModel::new(8.0, 16384, 0);
    let scheduler = Scheduler::new(policy, resources);

    let t1 = simple_item("S1", Priority::Normal);
    let t2 = simple_item("S2", Priority::Normal);
    let t1_id = t1.id.clone();

    scheduler.submit(t1).unwrap();
    scheduler.submit(t2).unwrap();

    let stats = scheduler.stats();
    assert_eq!(stats.pending_count, 2);
    assert_eq!(stats.completed_count, 0);

    // Dispatch both (no deps).
    scheduler.tick().unwrap();
    scheduler.complete_task(&t1_id, 0).unwrap();

    let stats = scheduler.stats();
    assert_eq!(stats.completed_count, 1);
}
