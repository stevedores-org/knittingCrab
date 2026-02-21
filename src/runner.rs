use crate::work_item::WorkItem;
use crate::worker::{TaskResult, Worker};
use std::time::{Duration, Instant};
use tokio::process::Command;

/// Configuration for the process runner.
pub struct ProcessRunnerConfig {
    /// Maximum time a process can run before being killed.
    pub timeout: Duration,
    /// Maximum bytes to capture from stdout.
    pub max_stdout_bytes: usize,
    /// Maximum bytes to capture from stderr.
    pub max_stderr_bytes: usize,
}

impl Default for ProcessRunnerConfig {
    fn default() -> Self {
        ProcessRunnerConfig {
            timeout: Duration::from_secs(300),
            max_stdout_bytes: 1_048_576, // 1 MB
            max_stderr_bytes: 1_048_576,
        }
    }
}

/// Executes tasks as child processes via `tokio::process::Command`.
pub struct ProcessRunner {
    config: ProcessRunnerConfig,
}

impl ProcessRunner {
    pub fn new(config: ProcessRunnerConfig) -> Self {
        ProcessRunner { config }
    }

    /// Builds the command string from the work item's goal field.
    /// The goal is interpreted as a shell command.
    fn build_command(task: &WorkItem) -> Command {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(&task.goal);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        cmd
    }

    fn truncate(bytes: &[u8], max: usize) -> String {
        let slice = if bytes.len() > max {
            &bytes[..max]
        } else {
            bytes
        };
        String::from_utf8_lossy(slice).into_owned()
    }
}

impl Worker for ProcessRunner {
    async fn execute(&self, task: &WorkItem) -> TaskResult {
        let start = Instant::now();
        let mut cmd = Self::build_command(task);

        let child: tokio::process::Child = match cmd.spawn() {
            Ok(child) => child,
            Err(e) => {
                return TaskResult {
                    task_id: task.id,
                    success: false,
                    exit_code: None,
                    reason: Some(format!("Failed to spawn process: {}", e)),
                    elapsed: start.elapsed(),
                };
            }
        };

        // Wait with timeout
        match tokio::time::timeout(self.config.timeout, child.wait_with_output()).await {
            Ok(Ok(output)) => {
                let code = output.status.code();
                let success = output.status.success();
                let stdout = Self::truncate(&output.stdout, self.config.max_stdout_bytes);
                let stderr = Self::truncate(&output.stderr, self.config.max_stderr_bytes);

                let reason = if success {
                    None
                } else {
                    Some(format!(
                        "exit_code={}, stderr={}",
                        code.map(|c: i32| c.to_string())
                            .unwrap_or_else(|| "signal".into()),
                        stderr
                    ))
                };

                TaskResult {
                    task_id: task.id,
                    success,
                    exit_code: code,
                    reason: if success && stdout.is_empty() {
                        None
                    } else {
                        reason
                    },
                    elapsed: start.elapsed(),
                }
            }
            Ok(Err(e)) => TaskResult {
                task_id: task.id,
                success: false,
                exit_code: None,
                reason: Some(format!("Process I/O error: {}", e)),
                elapsed: start.elapsed(),
            },
            Err(_) => {
                // Timeout — the child is dropped which sends SIGKILL on Unix
                TaskResult {
                    task_id: task.id,
                    success: false,
                    exit_code: None,
                    reason: Some(format!("Process timed out after {:?}", self.config.timeout)),
                    elapsed: start.elapsed(),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work_item::Priority;

    fn task_with_command(id: u64, cmd: &str) -> WorkItem {
        WorkItem::new(
            id,
            cmd,
            "repo",
            "main",
            Priority::Batch,
            vec![],
            1,
            512,
            false,
            None,
            0,
        )
    }

    #[tokio::test]
    async fn successful_command() {
        let runner = ProcessRunner::new(ProcessRunnerConfig::default());
        let task = task_with_command(1, "echo hello");
        let result = runner.execute(&task).await;
        assert!(result.success);
        assert_eq!(result.exit_code, Some(0));
    }

    #[tokio::test]
    async fn failing_command() {
        let runner = ProcessRunner::new(ProcessRunnerConfig::default());
        let task = task_with_command(1, "exit 42");
        let result = runner.execute(&task).await;
        assert!(!result.success);
        assert_eq!(result.exit_code, Some(42));
    }

    #[tokio::test]
    async fn timeout_kills_process() {
        let runner = ProcessRunner::new(ProcessRunnerConfig {
            timeout: Duration::from_millis(50),
            ..ProcessRunnerConfig::default()
        });
        let task = task_with_command(1, "sleep 60");
        let result = runner.execute(&task).await;
        assert!(!result.success);
        assert!(result.reason.unwrap().contains("timed out"));
        assert!(result.elapsed < Duration::from_secs(5));
    }

    #[tokio::test]
    async fn output_capture() {
        let runner = ProcessRunner::new(ProcessRunnerConfig::default());
        let task = task_with_command(1, "echo stdout_test && echo stderr_test >&2 && exit 1");
        let result = runner.execute(&task).await;
        assert!(!result.success);
        let reason = result.reason.unwrap();
        assert!(reason.contains("stderr_test"));
    }

    #[tokio::test]
    async fn output_truncation() {
        let runner = ProcessRunner::new(ProcessRunnerConfig {
            max_stderr_bytes: 10,
            ..ProcessRunnerConfig::default()
        });
        // Generate lots of stderr output
        let task = task_with_command(
            1,
            "python3 -c 'import sys; sys.stderr.write(\"x\" * 1000)' && exit 1",
        );
        let result = runner.execute(&task).await;
        assert!(!result.success);
        // The reason should contain truncated stderr
        let reason = result.reason.unwrap();
        assert!(reason.len() < 500, "stderr should be truncated");
    }
}
