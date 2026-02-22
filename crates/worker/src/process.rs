use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::PathBuf;
use std::process::Child as StdChild;
use std::sync::{Arc, Mutex};
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
    #[allow(dead_code)]
    task_id: TaskId,
    child: Arc<Mutex<StdChild>>,
    #[allow(dead_code)]
    sink: Arc<dyn EventSink>,
}

impl ProcessHandle {
    /// Wait for the process to complete and return its outcome.
    pub async fn wait(&mut self) -> Result<ExitOutcome, WorkerError> {
        let child = Arc::clone(&self.child);
        tokio::task::spawn_blocking(move || {
            let mut c = child.lock().unwrap();
            c.wait()
        })
        .await
        .map_err(|e| WorkerError::ProcessError(format!("spawn_blocking failed: {}", e)))?
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
        let pid = {
            let c = self.child.lock().unwrap();
            c.id()
        };

        #[cfg(unix)]
        {
            use nix::sys::signal::{kill, Signal};
            use nix::unistd::Pid;
            let _ = kill(Pid::from_raw(-(pid as i32)), Signal::SIGTERM);
        }

        // Wait with grace period
        let child = Arc::clone(&self.child);
        let wait_result = tokio::task::spawn_blocking(move || {
            let mut c = child.lock().unwrap();
            c.wait()
        });

        match timeout(grace, wait_result).await {
            Ok(Ok(Ok(status))) => {
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
            Ok(Ok(Err(e))) => Err(WorkerError::ProcessError(format!("wait failed: {}", e))),
            Ok(Err(e)) => Err(WorkerError::ProcessError(format!(
                "spawn_blocking failed: {}",
                e
            ))),
            Err(_) => {
                // Timeout: send SIGKILL
                #[cfg(unix)]
                {
                    use nix::sys::signal::{kill, Signal};
                    use nix::unistd::Pid;
                    let _ = kill(Pid::from_raw(-(pid as i32)), Signal::SIGKILL);
                }

                let child = Arc::clone(&self.child);
                match tokio::task::spawn_blocking(move || {
                    let mut c = child.lock().unwrap();
                    c.wait()
                })
                .await
                {
                    Ok(Ok(status)) => {
                        if let Some(signal) = status.signal() {
                            Ok(ExitOutcome::KilledBySignal(signal))
                        } else {
                            Ok(ExitOutcome::FailedWithCode(1))
                        }
                    }
                    _ => Ok(ExitOutcome::FailedWithCode(1)),
                }
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

    let mut child_std = cmd.spawn().map_err(|e| {
        WorkerError::SpawnFailed(format!("failed to spawn {}: {}", params.command[0], e))
    })?;

    let task_id = params.task_id;
    let sink_clone = Arc::clone(&sink);

    // Take stdout/stderr so we can read from them
    let stdout = child_std.stdout.take();
    let stderr = child_std.stderr.take();

    // Spawn async stdout reader
    if let Some(stdout) = stdout {
        let sink = Arc::clone(&sink_clone);
        tokio::spawn(async move {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(stdout);

            for (seq, line_result) in reader.lines().enumerate() {
                if let Ok(line) = line_result {
                    let log = knitting_crab_core::LogLine::new(
                        task_id,
                        seq as u64,
                        LogSource::Stdout,
                        line,
                    );
                    let _ = sink.emit_log(log).await;
                }
            }
        });
    }

    // Spawn async stderr reader
    if let Some(stderr) = stderr {
        let sink = Arc::clone(&sink_clone);
        tokio::spawn(async move {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(stderr);

            for (seq, line_result) in reader.lines().enumerate() {
                if let Ok(line) = line_result {
                    let log = knitting_crab_core::LogLine::new(
                        task_id,
                        seq as u64,
                        LogSource::Stderr,
                        line,
                    );
                    let _ = sink.emit_log(log).await;
                }
            }
        });
    }

    Ok(ProcessHandle {
        task_id,
        child: Arc::new(Mutex::new(child_std)),
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
}
