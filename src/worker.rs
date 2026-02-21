use std::path::{Path, PathBuf};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::time::{self, Duration};
use uuid::Uuid;

use crate::error::{Result, SchedulerError};
use crate::types::{WorkItem, WorkerEvent};

/// An isolated execution environment for a single agent task.
///
/// Each worker gets its own subdirectory under `workspace_base` so concurrent
/// tasks cannot interfere with one another's file outputs.
pub struct AgentWorker {
    /// Unique identifier for this worker instance.
    pub id: String,
    workspace_dir: PathBuf,
}

impl AgentWorker {
    /// Creates a new worker with a fresh workspace directory.
    ///
    /// The directory `workspace_base/<worker_id>/` is created on construction.
    pub fn new(workspace_base: &Path) -> Self {
        let id = Uuid::new_v4().to_string();
        let workspace_dir = workspace_base.join(&id);
        if let Err(e) = std::fs::create_dir_all(&workspace_dir) {
            // Non-fatal: log and continue; tasks may fail to write artifacts.
            tracing::warn!(error = %e, "Failed to create worker workspace dir");
        }
        Self { id, workspace_dir }
    }

    /// Executes `task` as a subprocess, streaming events over `event_tx`.
    ///
    /// Returns the process exit code.  The task's `timeout_secs` is enforced
    /// via [`tokio::time::timeout`].
    pub async fn execute(
        &self,
        task: &WorkItem,
        event_tx: mpsc::Sender<WorkerEvent>,
    ) -> Result<i32> {
        let (program, args) = task
            .command
            .split_first()
            .ok_or_else(|| SchedulerError::WorkerSpawnFailed("empty command".to_owned()))?;

        let mut child = Command::new(program)
            .args(args)
            .envs(task.env.iter())
            .current_dir(&self.workspace_dir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| SchedulerError::WorkerSpawnFailed(e.to_string()))?;

        // Notify listeners that the task has started.
        let _ = event_tx
            .send(WorkerEvent::Started {
                task_id: task.id.clone(),
                worker_id: self.id.clone(),
            })
            .await;

        // Stream stdout.
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SchedulerError::WorkerSpawnFailed("no stdout handle".to_owned()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| SchedulerError::WorkerSpawnFailed("no stderr handle".to_owned()))?;

        let task_id_stdout = task.id.clone();
        let tx_stdout = event_tx.clone();
        let stdout_handle = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let _ = tx_stdout
                    .send(WorkerEvent::LogLine {
                        task_id: task_id_stdout.clone(),
                        line,
                    })
                    .await;
            }
        });

        let task_id_stderr = task.id.clone();
        let tx_stderr = event_tx.clone();
        let stderr_handle = tokio::spawn(async move {
            let mut reader = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let _ = tx_stderr
                    .send(WorkerEvent::LogLine {
                        task_id: task_id_stderr.clone(),
                        line,
                    })
                    .await;
            }
        });

        // Wait for child with timeout.
        let timeout = Duration::from_secs(task.timeout_secs);
        let wait_result = time::timeout(timeout, child.wait()).await;

        // Ensure streaming tasks finish.
        let _ = stdout_handle.await;
        let _ = stderr_handle.await;

        match wait_result {
            Ok(Ok(status)) => {
                let exit_code = status.code().unwrap_or(-1);
                if exit_code == 0 {
                    let _ = event_tx
                        .send(WorkerEvent::Completed {
                            task_id: task.id.clone(),
                            exit_code,
                        })
                        .await;
                } else {
                    let _ = event_tx
                        .send(WorkerEvent::Failed {
                            task_id: task.id.clone(),
                            error: format!("exit code {exit_code}"),
                        })
                        .await;
                }
                Ok(exit_code)
            }
            Ok(Err(e)) => {
                let _ = event_tx
                    .send(WorkerEvent::Failed {
                        task_id: task.id.clone(),
                        error: e.to_string(),
                    })
                    .await;
                Err(SchedulerError::Io(e))
            }
            Err(_elapsed) => {
                // Kill the child process on timeout.
                let _ = child.kill().await;
                let _ = event_tx
                    .send(WorkerEvent::Failed {
                        task_id: task.id.clone(),
                        error: format!("timed out after {}s", task.timeout_secs),
                    })
                    .await;
                Err(SchedulerError::TaskTimeout {
                    task_id: task.id.clone(),
                    secs: task.timeout_secs,
                })
            }
        }
    }
}

impl Drop for AgentWorker {
    /// Removes the workspace directory when the worker is dropped.
    fn drop(&mut self) {
        if self.workspace_dir.exists() {
            if let Err(e) = std::fs::remove_dir_all(&self.workspace_dir) {
                tracing::warn!(
                    error = %e,
                    path = %self.workspace_dir.display(),
                    "Failed to clean up worker workspace"
                );
            }
        }
    }
}
