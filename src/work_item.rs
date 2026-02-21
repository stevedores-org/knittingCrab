use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Priority {
    Realtime,
    Interactive,
    Batch,
    Offline,
}

impl Priority {
    /// Lower numeric value = higher priority (for max-heap ordering)
    fn rank(&self) -> u8 {
        match self {
            Priority::Realtime => 0,
            Priority::Interactive => 1,
            Priority::Batch => 2,
            Priority::Offline => 3,
        }
    }
}

impl PartialOrd for Priority {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Priority {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.rank().cmp(&other.rank())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskState {
    Pending,
    Scheduled,
    Running,
    Success,
    Failed(String),
    Canceled,
}

#[derive(Debug, Clone)]
pub struct WorkItem {
    pub id: u64,
    pub goal: String,
    pub repo: String,
    pub branch: String,
    pub priority: Priority,
    pub dependencies: Vec<u64>,
    pub required_cpu_cores: u32,
    pub required_ram_mb: u64,
    pub requires_gpu: bool,
    pub deadline: Option<Instant>,
    pub max_retries: u32,
    pub retry_count: u32,
    pub state: TaskState,
    pub cache_key: String,
}

impl WorkItem {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: u64,
        goal: impl Into<String>,
        repo: impl Into<String>,
        branch: impl Into<String>,
        priority: Priority,
        dependencies: Vec<u64>,
        required_cpu_cores: u32,
        required_ram_mb: u64,
        requires_gpu: bool,
        deadline: Option<Instant>,
        max_retries: u32,
    ) -> Self {
        let goal = goal.into();
        let repo = repo.into();
        let branch = branch.into();
        let cache_key = Self::compute_cache_key_from(&goal, &repo, &branch);
        WorkItem {
            id,
            goal,
            repo,
            branch,
            priority,
            dependencies,
            required_cpu_cores,
            required_ram_mb,
            requires_gpu,
            deadline,
            max_retries,
            retry_count: 0,
            state: TaskState::Pending,
            cache_key,
        }
    }

    pub fn compute_cache_key(&self) -> String {
        Self::compute_cache_key_from(&self.goal, &self.repo, &self.branch)
    }

    fn compute_cache_key_from(goal: &str, repo: &str, branch: &str) -> String {
        // Deterministic cache key using FNV-1a (stable across Rust versions and runs).
        const FNV_OFFSET_BASIS: u64 = 14695981039346656037;
        const FNV_PRIME: u64 = 1099511628211;
        let mut hash: u64 = FNV_OFFSET_BASIS;
        for byte in goal
            .bytes()
            .chain(b"|".iter().copied())
            .chain(repo.bytes())
            .chain(b"|".iter().copied())
            .chain(branch.bytes())
        {
            hash ^= byte as u64;
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        format!("{:016x}", hash)
    }
}

impl PartialEq for WorkItem {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for WorkItem {}

/// Higher priority (lower rank) = Greater in ordering, so BinaryHeap (max-heap) pops highest priority first.
impl PartialOrd for WorkItem {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for WorkItem {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Reverse priority rank: lower rank (higher priority) is Greater.
        // Within the same priority, older tasks (lower id) are scheduled first.
        other
            .priority
            .rank()
            .cmp(&self.priority.rank())
            .then_with(|| other.id.cmp(&self.id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BinaryHeap;

    #[test]
    fn priority_orders_correctly() {
        let realtime = WorkItem::new(
            1,
            "a",
            "r",
            "main",
            Priority::Realtime,
            vec![],
            1,
            512,
            false,
            None,
            0,
        );
        let batch = WorkItem::new(
            2,
            "b",
            "r",
            "main",
            Priority::Batch,
            vec![],
            1,
            512,
            false,
            None,
            0,
        );

        let mut heap = BinaryHeap::new();
        heap.push(batch);
        heap.push(realtime);

        let first = heap.pop().unwrap();
        assert_eq!(first.priority, Priority::Realtime);
    }

    #[test]
    fn fairness_older_tasks_scheduled_first_within_same_priority() {
        let mut heap = BinaryHeap::new();
        // Insert in reverse order — id 9 first, id 0 last
        for i in (0..10).rev() {
            heap.push(WorkItem::new(
                i,
                format!("goal{}", i),
                "r",
                "main",
                Priority::Batch,
                vec![],
                1,
                512,
                false,
                None,
                0,
            ));
        }
        // Older tasks (lower id) should be popped first
        let mut prev_id = None;
        while let Some(item) = heap.pop() {
            if let Some(prev) = prev_id {
                assert!(
                    item.id > prev,
                    "Expected ascending id order (older first), got {} after {}",
                    item.id,
                    prev
                );
            }
            prev_id = Some(item.id);
        }
        assert_eq!(prev_id, Some(9));
    }

    #[test]
    fn deadline_tasks_handled() {
        let deadline = Instant::now() + std::time::Duration::from_secs(60);
        let item = WorkItem::new(
            1,
            "goal",
            "repo",
            "main",
            Priority::Interactive,
            vec![],
            1,
            512,
            false,
            Some(deadline),
            3,
        );
        assert!(item.deadline.is_some());
    }

    #[test]
    fn same_task_same_inputs_same_cache_key() {
        let a = WorkItem::new(
            1,
            "goal",
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
        let b = WorkItem::new(
            2,
            "goal",
            "repo",
            "main",
            Priority::Realtime,
            vec![],
            2,
            1024,
            true,
            None,
            0,
        );
        assert_eq!(a.cache_key, b.cache_key);
    }
}
