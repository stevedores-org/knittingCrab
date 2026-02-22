use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Budget constraints for an agent task.
///
/// Both limits are optional (0 means unlimited). If both are set, the task
/// fails if either limit is exceeded.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentBudget {
    /// Maximum tokens this task is allowed to consume (0 = unlimited).
    pub max_tokens: u64,

    /// Maximum duration in seconds (0 = unlimited).
    pub max_duration_secs: u64,
}

/// A test gate that must pass before a task is marked complete.
///
/// The test is run in the working directory and expected to exit with code 0.
/// Non-zero exit codes result in a TestGateFailed error.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestGate {
    /// Command to execute (e.g., ["cargo", "test", "--workspace"]).
    pub command: Vec<String>,

    /// Working directory where the command is run.
    pub working_dir: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_budget_default_is_unlimited() {
        let budget = AgentBudget::default();
        assert_eq!(budget.max_tokens, 0);
        assert_eq!(budget.max_duration_secs, 0);
    }

    #[test]
    fn agent_budget_with_limits() {
        let budget = AgentBudget {
            max_tokens: 1000,
            max_duration_secs: 300,
        };
        assert_eq!(budget.max_tokens, 1000);
        assert_eq!(budget.max_duration_secs, 300);
    }

    #[test]
    fn test_gate_creation() {
        let gate = TestGate {
            command: vec!["cargo".to_string(), "test".to_string()],
            working_dir: PathBuf::from("/tmp"),
        };
        assert_eq!(gate.command.len(), 2);
        assert_eq!(gate.command[0], "cargo");
        assert_eq!(gate.working_dir, PathBuf::from("/tmp"));
    }

    #[test]
    fn test_gate_serialization() {
        let gate = TestGate {
            command: vec!["test".to_string()],
            working_dir: PathBuf::from("/home"),
        };
        let json = serde_json::to_string(&gate).unwrap();
        let deserialized: TestGate = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.command, gate.command);
        assert_eq!(deserialized.working_dir, gate.working_dir);
    }
}
