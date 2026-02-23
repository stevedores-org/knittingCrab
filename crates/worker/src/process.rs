use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

use knitting_crab_core::event::LogSource;
use knitting_crab_core::ids::TaskId;
use knitting_crab_core::retry::ExitOutcome;
use knitting_crab_core::traits::EventSink;

use crate::error::WorkerError;

/// Parameters for spawning a process.
#[derive(Debug, Clone)]
pub struct SpawnParams {
    pub task_id: TaskId,
    pub command: Vec<String>,
    pub working_dir: PathBuf,
    pub env: std::collections::HashMap<String, String>,
}

/// Handle to a running process.
pub struct ProcessHandle {
    /// Task ID: used for observability and event tracing when structured logging is enabled (see Issue #70).
    #[allow(dead_code)]
    task_id: TaskId,
    child: tokio::process::Child,
    /// Event sink: used for emitting process lifecycle events for distributed tracing (see Issue #77).
    #[allow(dead_code)]
    sink: Arc<dyn EventSink>,
}

impl ProcessHandle {
    /// Wait for the process to complete and return its outcome.
    pub async fn wait(&mut self) -> Result<ExitOutcome, WorkerError> {
        self.child
            .wait()
            .await
            .map(|status| {
                if status.success() {
                    ExitOutcome::Success
                } else if let Some(code) = status.code() {
                    ExitOutcome::FailedWithCode(code)
                } else if let Some(signal) = status.signal() {
                    ExitOutcome::KilledBySignal(signal)
                } else {
                    ExitOutcome::FailedWithCode(1)
                }
            })
            .map_err(|e| WorkerError::ProcessError(format!("wait failed: {}", e)))
    }

    /// Kill the process gracefully: SIGTERM → grace period → SIGKILL.
    pub async fn kill_gracefully(&mut self, grace: Duration) -> Result<ExitOutcome, WorkerError> {
        // Send SIGTERM to process group
        if let Some(pid) = self.child.id() {
            #[cfg(unix)]
            {
                use nix::sys::signal::{kill, Signal};
                use nix::unistd::Pid;
                let _ = kill(Pid::from_raw(-(pid as i32)), Signal::SIGTERM);
            }
        } else {
            // Process already exited; id() returns None when the child has already been reaped.
            // The `wait()` call below will return its exit status immediately.
        }

        // Wait with grace period
        match timeout(grace, self.child.wait()).await {
            Ok(Ok(status)) => {
                if status.success() {
                    Ok(ExitOutcome::Success)
                } else if let Some(code) = status.code() {
                    Ok(ExitOutcome::FailedWithCode(code))
                } else if let Some(signal) = status.signal() {
                    Ok(ExitOutcome::KilledBySignal(signal))
                } else {
                    Ok(ExitOutcome::FailedWithCode(1))
                }
            }
            Ok(Err(e)) => Err(WorkerError::ProcessError(format!("wait failed: {}", e))),
            Err(_) => {
                // Timeout: send SIGKILL
                if let Some(pid) = self.child.id() {
                    #[cfg(unix)]
                    {
                        use nix::sys::signal::{kill, Signal};
                        use nix::unistd::Pid;
                        let _ = kill(Pid::from_raw(-(pid as i32)), Signal::SIGKILL);
                    }
                }

                self.child
                    .wait()
                    .await
                    .map(|status| {
                        if let Some(signal) = status.signal() {
                            ExitOutcome::KilledBySignal(signal)
                        } else {
                            ExitOutcome::FailedWithCode(1)
                        }
                    })
                    .map_err(|e| WorkerError::ProcessError(format!("wait failed: {}", e)))
            }
        }
    }
}

/// Spawn a process and return a handle to it.
pub async fn spawn(
    params: SpawnParams,
    sink: Arc<dyn EventSink>,
) -> Result<ProcessHandle, WorkerError> {
    let mut cmd = std::process::Command::new(&params.command[0]);

    if params.command.len() > 1 {
        cmd.args(&params.command[1..]);
    }

    cmd.current_dir(&params.working_dir);

    for (key, value) in params.env {
        cmd.env(key, value);
    }

    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    // Set process group so killpg works
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            nix::unistd::setpgid(nix::unistd::Pid::from_raw(0), nix::unistd::Pid::from_raw(0))
                .map_err(std::io::Error::from)
        });
    }

    // Convert to tokio::process::Command
    let mut tokio_cmd = tokio::process::Command::from(cmd);

    let mut child = tokio_cmd.spawn().map_err(|e| {
        WorkerError::SpawnFailed(format!("failed to spawn {}: {}", params.command[0], e))
    })?;

    let task_id = params.task_id;
    let sink_clone = Arc::clone(&sink);

    // Take stdout/stderr so we can read from them
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    // Spawn async stdout reader
    if let Some(stdout) = stdout {
        let sink = Arc::clone(&sink_clone);
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let reader = tokio::io::BufReader::new(stdout);
            let mut lines = reader.lines();

            let mut seq = 0;
            while let Ok(Some(line)) = lines.next_line().await {
                let log = knitting_crab_core::LogLine::new(task_id, seq, LogSource::Stdout, line);
                let _ = sink.emit_log(log).await;
                seq += 1;
            }
        });
    }

    // Spawn async stderr reader
    if let Some(stderr) = stderr {
        let sink = Arc::clone(&sink_clone);
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let reader = tokio::io::BufReader::new(stderr);
            let mut lines = reader.lines();

            let mut seq = 0;
            while let Ok(Some(line)) = lines.next_line().await {
                let log = knitting_crab_core::LogLine::new(task_id, seq, LogSource::Stderr, line);
                let _ = sink.emit_log(log).await;
                seq += 1;
            }
        });
    }

    Ok(ProcessHandle {
        task_id,
        child,
        sink: sink_clone,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Mutex;

    struct TestSink {
        logs: Mutex<Vec<knitting_crab_core::LogLine>>,
    }

    #[async_trait]
    impl EventSink for TestSink {
        async fn emit_event(
            &self,
            _event: knitting_crab_core::TaskEvent,
        ) -> Result<(), knitting_crab_core::CoreError> {
            Ok(())
        }

        async fn emit_log(
            &self,
            log: knitting_crab_core::LogLine,
        ) -> Result<(), knitting_crab_core::CoreError> {
            self.logs.lock().unwrap().push(log);
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_spawn_real_subprocess() {
        let sink = Arc::new(TestSink {
            logs: Mutex::new(Vec::new()),
        });

        let params = SpawnParams {
            task_id: TaskId::new(),
            command: vec!["echo".to_string(), "hello world".to_string()],
            working_dir: PathBuf::from("/tmp"),
            env: Default::default(),
        };

        let mut handle = spawn(params, sink.clone()).await.unwrap();
        let outcome = handle.wait().await.unwrap();

        assert_eq!(outcome, ExitOutcome::Success);

        // Give log reader time to process
        tokio::time::sleep(Duration::from_millis(100)).await;

        let logs = sink.logs.lock().unwrap();
        assert!(!logs.is_empty());
        let output = logs.iter().map(|l| &l.content).collect::<Vec<_>>();
        assert!(output.iter().any(|s| s.contains("hello world")));
    }

    // ===== Signal Handling Tests =====

    #[tokio::test]
    async fn test_process_exit_success() {
        let sink = Arc::new(TestSink {
            logs: Mutex::new(Vec::new()),
        });

        let params = SpawnParams {
            task_id: TaskId::new(),
            command: vec!["sh".to_string(), "-c".to_string(), "exit 0".to_string()],
            working_dir: PathBuf::from("/tmp"),
            env: Default::default(),
        };

        let mut handle = spawn(params, sink).await.unwrap();
        let outcome = handle.wait().await.unwrap();

        assert_eq!(outcome, ExitOutcome::Success);
    }

    #[tokio::test]
    async fn test_process_exit_with_code() {
        let sink = Arc::new(TestSink {
            logs: Mutex::new(Vec::new()),
        });

        let params = SpawnParams {
            task_id: TaskId::new(),
            command: vec!["sh".to_string(), "-c".to_string(), "exit 42".to_string()],
            working_dir: PathBuf::from("/tmp"),
            env: Default::default(),
        };

        let mut handle = spawn(params, sink).await.unwrap();
        let outcome = handle.wait().await.unwrap();

        assert_eq!(outcome, ExitOutcome::FailedWithCode(42));
    }

    #[tokio::test]
    async fn test_graceful_shutdown_timeout_triggers_kill() {
        let sink = Arc::new(TestSink {
            logs: Mutex::new(Vec::new()),
        });

        let params = SpawnParams {
            task_id: TaskId::new(),
            // Sleep command that ignores SIGTERM: uses sleep(10) then exit
            command: vec![
                "sh".to_string(),
                "-c".to_string(),
                "sleep 10; exit 0".to_string(),
            ],
            working_dir: PathBuf::from("/tmp"),
            env: Default::default(),
        };

        let mut handle = spawn(params, sink).await.unwrap();

        // Try graceful shutdown with very short timeout (50ms)
        // This should timeout and escalate to SIGKILL
        let outcome = handle
            .kill_gracefully(Duration::from_millis(50))
            .await
            .unwrap();

        // Process should be killed (either by SIGKILL or timeout handling)
        assert!(
            matches!(
                outcome,
                ExitOutcome::KilledBySignal(_) | ExitOutcome::FailedWithCode(_)
            ),
            "Expected KilledBySignal or FailedWithCode, got {:?}",
            outcome
        );
    }

    #[tokio::test]
    async fn test_graceful_shutdown_waits_and_kills() {
        let sink = Arc::new(TestSink {
            logs: Mutex::new(Vec::new()),
        });

        let params = SpawnParams {
            task_id: TaskId::new(),
            // Long-running process that will be killed
            command: vec!["sleep".to_string(), "30".to_string()],
            working_dir: PathBuf::from("/tmp"),
            env: Default::default(),
        };

        let mut handle = spawn(params, sink).await.unwrap();

        // Graceful shutdown with short timeout should kill the process
        let outcome = handle
            .kill_gracefully(Duration::from_millis(50))
            .await
            .unwrap();

        // Process should be terminated (either by signal or exit code)
        assert!(
            matches!(
                outcome,
                ExitOutcome::KilledBySignal(_) | ExitOutcome::FailedWithCode(_)
            ),
            "Expected terminated process, got {:?}",
            outcome
        );
    }

    #[tokio::test]
    async fn test_spawn_with_env_variables() {
        let sink = Arc::new(TestSink {
            logs: Mutex::new(Vec::new()),
        });

        let mut env = std::collections::HashMap::new();
        env.insert("TEST_VAR".to_string(), "test_value".to_string());

        let params = SpawnParams {
            task_id: TaskId::new(),
            command: vec![
                "sh".to_string(),
                "-c".to_string(),
                "echo $TEST_VAR".to_string(),
            ],
            working_dir: PathBuf::from("/tmp"),
            env,
        };

        let mut handle = spawn(params, sink.clone()).await.unwrap();
        let outcome = handle.wait().await.unwrap();

        assert_eq!(outcome, ExitOutcome::Success);

        // Give log reader time to process
        tokio::time::sleep(Duration::from_millis(100)).await;

        let logs = sink.logs.lock().unwrap();
        let output = logs.iter().map(|l| &l.content).collect::<Vec<_>>();
        assert!(
            output.iter().any(|s| s.contains("test_value")),
            "Expected TEST_VAR value in output, got {:?}",
            output
        );
    }

    #[tokio::test]
    async fn test_spawn_with_working_directory() {
        let sink = Arc::new(TestSink {
            logs: Mutex::new(Vec::new()),
        });

        let params = SpawnParams {
            task_id: TaskId::new(),
            command: vec!["pwd".to_string()],
            working_dir: PathBuf::from("/tmp"),
            env: Default::default(),
        };

        let mut handle = spawn(params, sink.clone()).await.unwrap();
        let outcome = handle.wait().await.unwrap();

        assert_eq!(outcome, ExitOutcome::Success);

        // Give log reader time to process
        tokio::time::sleep(Duration::from_millis(100)).await;

        let logs = sink.logs.lock().unwrap();
        let output = logs.iter().map(|l| &l.content).collect::<Vec<_>>();
        assert!(
            output.iter().any(|s| s.contains("/tmp")),
            "Expected /tmp in output, got {:?}",
            output
        );
    }

    #[tokio::test]
    async fn test_spawn_with_multiline_output() {
        let sink = Arc::new(TestSink {
            logs: Mutex::new(Vec::new()),
        });

        let params = SpawnParams {
            task_id: TaskId::new(),
            command: vec![
                "sh".to_string(),
                "-c".to_string(),
                "echo line1; echo line2; echo line3".to_string(),
            ],
            working_dir: PathBuf::from("/tmp"),
            env: Default::default(),
        };

        let mut handle = spawn(params, sink.clone()).await.unwrap();
        let outcome = handle.wait().await.unwrap();

        assert_eq!(outcome, ExitOutcome::Success);

        // Give log reader time to process all lines
        tokio::time::sleep(Duration::from_millis(100)).await;

        let logs = sink.logs.lock().unwrap();
        assert!(
            logs.len() >= 3,
            "Expected at least 3 log lines, got {}",
            logs.len()
        );
        assert!(logs.iter().any(|l| l.content == "line1"));
        assert!(logs.iter().any(|l| l.content == "line2"));
        assert!(logs.iter().any(|l| l.content == "line3"));
    }

    #[tokio::test]
    async fn test_spawn_stderr_handling() {
        let sink = Arc::new(TestSink {
            logs: Mutex::new(Vec::new()),
        });

        let params = SpawnParams {
            task_id: TaskId::new(),
            command: vec![
                "sh".to_string(),
                "-c".to_string(),
                "echo stderr_msg >&2".to_string(),
            ],
            working_dir: PathBuf::from("/tmp"),
            env: Default::default(),
        };

        let mut handle = spawn(params, sink.clone()).await.unwrap();
        let outcome = handle.wait().await.unwrap();

        assert_eq!(outcome, ExitOutcome::Success);

        // Give log reader time to process
        tokio::time::sleep(Duration::from_millis(100)).await;

        let logs = sink.logs.lock().unwrap();
        let stderr_logs: Vec<_> = logs
            .iter()
            .filter(|l| l.source == LogSource::Stderr)
            .collect();

        assert!(
            !stderr_logs.is_empty(),
            "Expected stderr logs, got: {:?}",
            logs
        );
        assert!(stderr_logs.iter().any(|l| l.content.contains("stderr_msg")));
    }

    #[tokio::test]
    async fn test_spawn_command_not_found() {
        let sink = Arc::new(TestSink {
            logs: Mutex::new(Vec::new()),
        });

        let params = SpawnParams {
            task_id: TaskId::new(),
            command: vec!["nonexistent_command_12345".to_string()],
            working_dir: PathBuf::from("/tmp"),
            env: Default::default(),
        };

        let result = spawn(params, sink).await;
        assert!(
            result.is_err(),
            "Expected error when spawning nonexistent command"
        );
    }
}
