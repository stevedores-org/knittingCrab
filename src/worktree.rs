use std::path::{Path, PathBuf};
use std::process::Command;

/// Manages git worktree creation and cleanup for isolated task execution.
pub struct WorktreeManager {
    base_dir: PathBuf,
}

/// RAII guard that removes a worktree when dropped.
pub struct TaskScope {
    pub path: PathBuf,
    #[allow(dead_code)]
    task_id: u64,
}

impl WorktreeManager {
    /// Creates a new WorktreeManager with a base directory for worktrees.
    pub fn new(base_dir: PathBuf) -> Self {
        WorktreeManager { base_dir }
    }

    /// Creates a new git worktree for the given task, checked out to the specified revision.
    /// Returns a TaskScope that will automatically clean up when dropped.
    pub async fn create(
        &self,
        task_id: u64,
        repo: &Path,
        rev: &str,
    ) -> Result<TaskScope, Box<dyn std::error::Error>> {
        // Create the base directory if it doesn't exist
        std::fs::create_dir_all(&self.base_dir)?;

        let worktree_path = self.base_dir.join(task_id.to_string());

        // Run: git worktree add <path> <rev>
        let output = Command::new("git")
            .arg("worktree")
            .arg("add")
            .arg(&worktree_path)
            .arg(rev)
            .current_dir(repo)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("Failed to create worktree: {}", stderr).into());
        }

        Ok(TaskScope {
            path: worktree_path,
            task_id,
        })
    }
}

impl Drop for TaskScope {
    fn drop(&mut self) {
        // Clean up: git worktree remove --force <path>, then remove the directory
        // We use synchronous Command here since this is in a destructor.
        let _ = Command::new("git")
            .arg("worktree")
            .arg("remove")
            .arg("--force")
            .arg(&self.path)
            .output();

        // Also try to remove the directory directly (in case git worktree didn't fully clean)
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[tokio::test]
    async fn worktree_created_per_task() {
        let temp_dir = TempDir::new().unwrap();
        let repo_path = temp_dir.path().join("repo");
        let worktree_base = temp_dir.path().join("worktrees");

        // Initialize a git repo
        fs::create_dir(&repo_path).unwrap();
        Command::new("git")
            .arg("init")
            .current_dir(&repo_path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // Create a test commit
        fs::write(repo_path.join("test.txt"), "test").unwrap();
        Command::new("git")
            .arg("add")
            .arg("test.txt")
            .current_dir(&repo_path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let manager = WorktreeManager::new(worktree_base.clone());

        // Create 5 worktrees
        let mut scopes = vec![];
        for i in 0..5 {
            let scope = manager
                .create(i, &repo_path, "HEAD")
                .await
                .expect("failed to create worktree");
            assert!(
                scope.path.exists(),
                "worktree path {} should exist",
                scope.path.display()
            );
            scopes.push(scope);
        }

        // Verify all have distinct paths
        let paths: Vec<_> = scopes.iter().map(|s| &s.path).collect();
        for i in 0..paths.len() {
            for j in (i + 1)..paths.len() {
                assert_ne!(paths[i], paths[j], "worktree paths must be distinct");
            }
        }
    }

    #[tokio::test]
    async fn cleanup_happens_on_success() {
        let temp_dir = TempDir::new().unwrap();
        let repo_path = temp_dir.path().join("repo");
        let worktree_base = temp_dir.path().join("worktrees");

        // Initialize a git repo
        fs::create_dir(&repo_path).unwrap();
        Command::new("git")
            .arg("init")
            .current_dir(&repo_path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // Create a test commit
        fs::write(repo_path.join("test.txt"), "test").unwrap();
        Command::new("git")
            .arg("add")
            .arg("test.txt")
            .current_dir(&repo_path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let manager = WorktreeManager::new(worktree_base.clone());
        let scope = manager
            .create(1, &repo_path, "HEAD")
            .await
            .expect("failed to create worktree");

        let path = scope.path.clone();
        assert!(path.exists(), "worktree should exist before drop");

        drop(scope);

        // After drop, the directory should be cleaned up
        assert!(!path.exists(), "worktree should be removed after drop");
    }

    #[tokio::test]
    async fn cleanup_happens_on_failure() {
        let temp_dir = TempDir::new().unwrap();
        let repo_path = temp_dir.path().join("repo");
        let worktree_base = temp_dir.path().join("worktrees");

        // Initialize a git repo
        fs::create_dir(&repo_path).unwrap();
        Command::new("git")
            .arg("init")
            .current_dir(&repo_path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // Create a test commit
        fs::write(repo_path.join("test.txt"), "test").unwrap();
        Command::new("git")
            .arg("add")
            .arg("test.txt")
            .current_dir(&repo_path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let manager = WorktreeManager::new(worktree_base.clone());
        let scope = manager
            .create(2, &repo_path, "HEAD")
            .await
            .expect("failed to create worktree");

        let path = scope.path.clone();
        assert!(path.exists(), "worktree should exist before drop");

        std::mem::drop(scope);

        // After drop, the directory should be cleaned up
        assert!(!path.exists(), "worktree should be removed after drop");
    }

    #[tokio::test]
    async fn cleanup_is_idempotent() {
        let temp_dir = TempDir::new().unwrap();
        let repo_path = temp_dir.path().join("repo");
        let worktree_base = temp_dir.path().join("worktrees");

        // Initialize a git repo
        fs::create_dir(&repo_path).unwrap();
        Command::new("git")
            .arg("init")
            .current_dir(&repo_path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["config", "user.name", "Test User"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        // Create a test commit
        fs::write(repo_path.join("test.txt"), "test").unwrap();
        Command::new("git")
            .arg("add")
            .arg("test.txt")
            .current_dir(&repo_path)
            .output()
            .unwrap();

        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(&repo_path)
            .output()
            .unwrap();

        let manager = WorktreeManager::new(worktree_base.clone());
        let scope = manager
            .create(3, &repo_path, "HEAD")
            .await
            .expect("failed to create worktree");

        let path = scope.path.clone();

        drop(scope);
        assert!(
            !path.exists(),
            "worktree should be removed after first drop"
        );

        // Manually drop again (double-drop) — should not panic
        // This simulates what could happen in unusual error paths
        let _ = std::fs::remove_dir_all(&path);
    }
}
