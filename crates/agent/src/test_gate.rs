use knitting_crab_core::agent::TestGate;
use knitting_crab_core::error::CoreError;
use knitting_crab_core::ids::TaskId;
use tokio::process::Command;

/// Runs a test gate and verifies it passes (exit code 0).
pub struct TestGateRunner;

impl TestGateRunner {
    /// Run a test gate command.
    ///
    /// Returns `Ok(())` if the command exits with code 0.
    /// Returns `CoreError::TestGateFailed` if the command exits with non-zero code.
    pub async fn run(_task_id: TaskId, gate: &TestGate) -> Result<(), CoreError> {
        let mut cmd = Command::new(&gate.command[0]);

        if gate.command.len() > 1 {
            cmd.args(&gate.command[1..]);
        }

        cmd.current_dir(&gate.working_dir);

        let output = cmd.output().await.map_err(CoreError::IoError)?;

        if output.status.success() {
            Ok(())
        } else {
            let exit_code = output.status.code().unwrap_or(-1);
            Err(CoreError::TestGateFailed { exit_code })
        }
    }

    /// Run an optional test gate.
    ///
    /// Returns `Ok(())` if gate is None or if it passes.
    pub async fn run_optional(task_id: TaskId, gate: Option<&TestGate>) -> Result<(), CoreError> {
        match gate {
            Some(g) => Self::run(task_id, g).await,
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_gate_passes_on_zero_exit() {
        let gate = TestGate {
            command: vec!["true".to_string()],
            working_dir: PathBuf::from("/tmp"),
        };

        let result = TestGateRunner::run(TaskId::new(), &gate).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_gate_fails_on_nonzero_exit() {
        let gate = TestGate {
            command: vec!["sh".to_string(), "-c".to_string(), "exit 42".to_string()],
            working_dir: PathBuf::from("/tmp"),
        };

        let result = TestGateRunner::run(TaskId::new(), &gate).await;
        assert!(matches!(
            result,
            Err(CoreError::TestGateFailed { exit_code: 42 })
        ));
    }

    #[tokio::test]
    async fn test_gate_noop_when_none() {
        let result = TestGateRunner::run_optional(TaskId::new(), None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_gate_captures_correct_exit_code() {
        let gate = TestGate {
            command: vec!["sh".to_string(), "-c".to_string(), "exit 7".to_string()],
            working_dir: PathBuf::from("/tmp"),
        };

        let result = TestGateRunner::run(TaskId::new(), &gate).await;
        assert!(matches!(
            result,
            Err(CoreError::TestGateFailed { exit_code: 7 })
        ));
    }

    #[tokio::test]
    async fn test_gate_fails_cleanly_on_bad_command() {
        let gate = TestGate {
            command: vec!["nonexistent-binary-xyz".to_string()],
            working_dir: PathBuf::from("/tmp"),
        };

        let result = TestGateRunner::run(TaskId::new(), &gate).await;
        assert!(matches!(result, Err(CoreError::IoError(_))));
    }
}
