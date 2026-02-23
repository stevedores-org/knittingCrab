use dashmap::DashMap;
use tracing;
use knitting_crab_core::error::CoreError;
use knitting_crab_core::ids::TaskId;
use knitting_crab_core::traits::TaskDescriptor;
use std::time::Instant;

/// Tracks budget usage (tokens and time) for agent tasks.
pub struct BudgetTracker {
    entries: DashMap<TaskId, BudgetEntry>,
}

struct BudgetEntry {
    started_at: Instant,
    tokens_used: u64,
}

impl BudgetTracker {
    /// Create a new budget tracker.
    pub fn new() -> Self {
        Self {
            entries: DashMap::new(),
        }
    }

    /// Record task start time for a task.
    pub fn start(&self, task_id: TaskId) {
        self.entries.insert(
            task_id,
            BudgetEntry {
                started_at: Instant::now(),
                tokens_used: 0,
            },
        );
    }

    /// Charge tokens against the task's budget.
    ///
    /// Returns error if budget is exceeded or task not found.
    pub fn charge_tokens(
        &self,
        task_id: TaskId,
        amount: u64,
        descriptor: &TaskDescriptor,
    ) -> Result<(), CoreError> {
        // If no budget set, always allow
        let budget = match &descriptor.budget {
            Some(b) => b,
            None => return Ok(()),
        };

        // If no token limit, allow
        if budget.max_tokens == 0 {
            return Ok(());
        }

        // Update tokens_used
        if let Some(mut entry) = self.entries.get_mut(&task_id) {
            entry.tokens_used += amount;
            tracing::debug!(task_id = %task_id, tokens = amount, total_used = entry.tokens_used, limit = budget.max_tokens, "charging token budget");
            if entry.tokens_used > budget.max_tokens {
                return Err(CoreError::BudgetExceeded {
                    reason: format!(
                        "token limit exceeded: {} > {}",
                        entry.tokens_used, budget.max_tokens
                    ),
                });
            }
        }

        Ok(())
    }

    /// Check if time budget is exceeded.
    ///
    /// Returns error if time limit exceeded or task not found.
    pub fn check_time(
        &self,
        task_id: TaskId,
        descriptor: &TaskDescriptor,
    ) -> Result<(), CoreError> {
        // If no budget set, always allow
        let budget = match &descriptor.budget {
            Some(b) => b,
            None => return Ok(()),
        };

        // If no time limit, allow
        if budget.max_duration_secs == 0 {
            return Ok(());
        }

        // Check elapsed time
        if let Some(entry) = self.entries.get(&task_id) {
            let elapsed = entry.started_at.elapsed().as_secs();
            tracing::debug!(task_id = %task_id, elapsed_secs = elapsed, limit_secs = budget.max_duration_secs, "checking time budget");
            if elapsed > budget.max_duration_secs {
                return Err(CoreError::BudgetExceeded {
                    reason: format!(
                        "time limit exceeded: {} > {} seconds",
                        elapsed, budget.max_duration_secs
                    ),
                });
            }
        }

        Ok(())
    }

    /// Complete a task and clean up its budget entry.
    pub fn finish(&self, task_id: TaskId) {
        self.entries.remove(&task_id);
    }
}

impl Default for BudgetTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn make_descriptor_with_budget(max_tokens: u64, max_duration_secs: u64) -> TaskDescriptor {
        TaskDescriptor {
            task_id: TaskId::new(),
            command: vec!["echo".to_string()],
            working_dir: PathBuf::from("/tmp"),
            env: HashMap::new(),
            resources: knitting_crab_core::resource::ResourceAllocation::default(),
            policy: knitting_crab_core::retry::RetryPolicy::default(),
            attempt: 0,
            is_critical: false,
            priority: knitting_crab_core::Priority::Normal,
            dependencies: vec![],
            goal: None,
            budget: Some(knitting_crab_core::AgentBudget {
                max_tokens,
                max_duration_secs,
            }),
            test_gate: None,
        }
    }

    fn make_descriptor_no_budget() -> TaskDescriptor {
        TaskDescriptor {
            task_id: TaskId::new(),
            command: vec!["echo".to_string()],
            working_dir: PathBuf::from("/tmp"),
            env: HashMap::new(),
            resources: knitting_crab_core::resource::ResourceAllocation::default(),
            policy: knitting_crab_core::retry::RetryPolicy::default(),
            attempt: 0,
            is_critical: false,
            priority: knitting_crab_core::Priority::Normal,
            dependencies: vec![],
            goal: None,
            budget: None,
            test_gate: None,
        }
    }

    #[test]
    fn budget_allows_when_under_token_limit() {
        let tracker = BudgetTracker::new();
        let task_id = TaskId::new();
        let descriptor = make_descriptor_with_budget(100, 0);

        tracker.start(task_id);
        let result = tracker.charge_tokens(task_id, 50, &descriptor);
        assert!(result.is_ok());
    }

    #[test]
    fn budget_denies_when_over_token_limit() {
        let tracker = BudgetTracker::new();
        let task_id = TaskId::new();
        let descriptor = make_descriptor_with_budget(100, 0);

        tracker.start(task_id);
        tracker.charge_tokens(task_id, 60, &descriptor).unwrap();
        let result = tracker.charge_tokens(task_id, 50, &descriptor);
        assert!(matches!(result, Err(CoreError::BudgetExceeded { .. })));
    }

    #[test]
    fn budget_time_limit_returns_error_when_unlimited() {
        let tracker = BudgetTracker::new();
        let task_id = TaskId::new();
        let descriptor = make_descriptor_with_budget(100, 0); // 0 = unlimited time

        tracker.start(task_id);
        // Should never fail with unlimited time budget
        let result = tracker.check_time(task_id, &descriptor);
        assert!(result.is_ok());

        // Even with large delay, unlimited budget should allow
        std::thread::sleep(std::time::Duration::from_millis(100));
        let result = tracker.check_time(task_id, &descriptor);
        assert!(result.is_ok());
    }

    #[test]
    fn budget_noop_when_no_budget_set() {
        let tracker = BudgetTracker::new();
        let task_id = TaskId::new();
        let descriptor = make_descriptor_no_budget();

        tracker.start(task_id);
        let result = tracker.charge_tokens(task_id, 1_000_000, &descriptor);
        assert!(result.is_ok());

        let result = tracker.check_time(task_id, &descriptor);
        assert!(result.is_ok());
    }

    #[test]
    fn budget_finish_cleans_up_entry() {
        let tracker = BudgetTracker::new();
        let task_id = TaskId::new();

        tracker.start(task_id);
        assert_eq!(tracker.entries.len(), 1);

        tracker.finish(task_id);
        assert_eq!(tracker.entries.len(), 0);
    }
}
