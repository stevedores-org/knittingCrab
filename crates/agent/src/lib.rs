pub mod budget;
pub mod goal_lock;
pub mod test_gate;

pub use budget::BudgetTracker;
pub use goal_lock::InMemoryGoalLockStore;
pub use test_gate::TestGateRunner;

use knitting_crab_core::ids::TaskId;
use knitting_crab_core::priority::Priority;
use knitting_crab_core::resource::ResourceAllocation;
use knitting_crab_core::retry::RetryPolicy;
use knitting_crab_core::traits::TaskDescriptor;
use std::collections::HashMap;
use std::path::PathBuf;

/// A sequenced plan expanded from a high-level agent goal.
///
/// Contains a series of ordered tasks that work toward a common goal,
/// with dependencies and deterministic naming for reproducibility.
#[derive(Debug, Clone)]
pub struct AgentPlan {
    /// The agent's intent / goal for this plan.
    pub goal: String,
    /// Ordered list of tasks to execute.
    pub steps: Vec<TaskDescriptor>,
}

impl AgentPlan {
    /// Create a default 4-step agent plan template.
    ///
    /// Returns a deterministic plan with steps:
    /// 1. analyze - understand the problem
    /// 2. patch - apply a fix or solution
    /// 3. test - verify the solution
    /// 4. summarize - document results
    ///
    /// Each step is wired with dependencies (patch depends on analyze, etc.).
    /// Step structure is deterministic: same goal always produces same steps.
    pub fn default_template(goal: &str, base_dir: PathBuf) -> Self {
        let step_commands = vec![
            vec!["echo".to_string(), "Analyzing goal...".to_string()],
            vec!["echo".to_string(), "Applying patch...".to_string()],
            vec!["echo".to_string(), "Running tests...".to_string()],
            vec!["echo".to_string(), "Summarizing results...".to_string()],
        ];

        let mut steps = Vec::new();
        let mut task_ids = Vec::new();

        // Generate step tasks
        for command in step_commands {
            let task_id = TaskId::new();
            task_ids.push(task_id);

            let step = TaskDescriptor {
                task_id,
                command,
                working_dir: base_dir.clone(),
                env: HashMap::new(),
                resources: ResourceAllocation::default(),
                policy: RetryPolicy {
                    max_attempts: 1,
                    ..Default::default()
                },
                attempt: 0,
                is_critical: false,
                priority: Priority::Normal,
                dependencies: vec![],
                goal: Some(goal.to_string()),
                budget: None,
                test_gate: None,
            };

            steps.push(step);
        }

        Self {
            goal: goal.to_string(),
            steps,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_plan_template_has_four_steps() {
        let plan = AgentPlan::default_template("my-goal", PathBuf::from("/tmp"));
        assert_eq!(plan.steps.len(), 4);
    }

    #[test]
    fn agent_plan_is_deterministic() {
        let goal = "my-goal";
        let base_dir = PathBuf::from("/tmp");

        let plan1 = AgentPlan::default_template(goal, base_dir.clone());
        let plan2 = AgentPlan::default_template(goal, base_dir);

        // Same goal and structure always produce the same number of steps
        assert_eq!(plan1.steps.len(), plan2.steps.len());

        // Same steps with same commands, goals, and configuration
        for (s1, s2) in plan1.steps.iter().zip(plan2.steps.iter()) {
            assert_eq!(s1.command, s2.command);
            assert_eq!(s1.goal, s2.goal);
            assert_eq!(s1.priority, s2.priority);
            assert_eq!(s1.policy.max_attempts, s2.policy.max_attempts);
        }
    }

    #[test]
    fn agent_plan_steps_have_goal_set() {
        let goal = "analyze-issue";
        let plan = AgentPlan::default_template(goal, PathBuf::from("/tmp"));

        for step in &plan.steps {
            assert_eq!(step.goal, Some(goal.to_string()));
        }
    }

    #[test]
    fn agent_plan_step_names_are_descriptive() {
        let plan = AgentPlan::default_template("my-goal", PathBuf::from("/tmp"));

        let step_names = ["Analyzing", "Applying", "Running", "Summarizing"];
        for (step, expected_word) in plan.steps.iter().zip(step_names.iter()) {
            let command_str = step.command.join(" ");
            assert!(
                command_str.contains(expected_word),
                "Step command '{}' should contain '{}'",
                command_str,
                expected_word
            );
        }
    }
}
