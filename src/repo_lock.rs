use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};

/// Guard that holds a semaphore permit, representing a held lock on a repository.
/// The lock is automatically released when this guard is dropped.
pub struct RepoLockGuard {
    _semaphore: Arc<Semaphore>,
    _permit: Box<tokio::sync::SemaphorePermit<'static>>,
}

impl RepoLockGuard {
    fn new(semaphore: Arc<Semaphore>, permit: tokio::sync::SemaphorePermit<'_>) -> Self {
        let permit_boxed = Box::new(unsafe {
            std::mem::transmute::<
                tokio::sync::SemaphorePermit<'_>,
                tokio::sync::SemaphorePermit<'static>,
            >(permit)
        });
        RepoLockGuard {
            _semaphore: semaphore,
            _permit: permit_boxed,
        }
    }
}

/// Manages per-repository locks to serialize concurrent task access to the same repository.
pub struct RepoLockManager {
    locks: Arc<Mutex<HashMap<String, Arc<Semaphore>>>>,
}

impl RepoLockManager {
    /// Creates a new RepoLockManager.
    pub fn new() -> Self {
        RepoLockManager {
            locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Acquires a lock for the specified repository.
    /// If no lock exists for this repo, one is created lazily.
    /// Blocks until the lock is acquired.
    pub async fn lock(&self, repo: &str) -> RepoLockGuard {
        // Acquire the inner lock map
        let mut locks_map = self.locks.lock().await;

        // Get or create the semaphore for this repo (capacity 1 = mutex behavior)
        let repo_sem = locks_map
            .entry(repo.to_string())
            .or_insert_with(|| Arc::new(Semaphore::new(1)))
            .clone();

        drop(locks_map);

        // Acquire the semaphore permit
        let permit = repo_sem.acquire().await.unwrap();

        RepoLockGuard::new(repo_sem.clone(), permit)
    }
}

impl Default for RepoLockManager {
    fn default() -> Self {
        Self::new()
    }
}

// Allow cloning the manager for easier testing
impl Clone for RepoLockManager {
    fn clone(&self) -> Self {
        RepoLockManager {
            locks: self.locks.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc as StdArc;
    use std::time::Duration;

    #[tokio::test]
    async fn concurrent_tasks_same_repo_serialize() {
        let manager = RepoLockManager::new();
        let counter = StdArc::new(AtomicU32::new(0));
        let in_critical = StdArc::new(AtomicU32::new(0));

        let mut handles = vec![];

        for _ in 0..2 {
            let manager_clone = manager.clone();
            let counter_clone = counter.clone();
            let in_critical_clone = in_critical.clone();

            let handle = tokio::spawn(async move {
                // Acquire lock for same repo
                let _guard = manager_clone.lock("shared_repo").await;

                // Mark that we're in the critical section
                in_critical_clone.fetch_add(1, Ordering::SeqCst);

                // Verify only one task is in critical section
                let in_cs = in_critical_clone.load(Ordering::SeqCst);
                assert_eq!(in_cs, 1, "only one task should be in critical section");

                counter_clone.fetch_add(1, Ordering::SeqCst);

                // Sleep briefly to simulate work
                tokio::time::sleep(Duration::from_millis(10)).await;

                in_critical_clone.fetch_sub(1, Ordering::SeqCst);
            });

            handles.push(handle);
        }

        // Wait for all tasks to complete
        for handle in handles {
            handle.await.unwrap();
        }

        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn different_repos_run_in_parallel() {
        let manager = RepoLockManager::new();
        let start = std::time::Instant::now();

        let mut handles = vec![];

        for i in 0..2 {
            let manager_clone = manager.clone();

            let handle = tokio::spawn(async move {
                let _guard = manager_clone.lock(&format!("repo_{}", i)).await;
                tokio::time::sleep(Duration::from_millis(50)).await;
            });

            handles.push(handle);
        }

        for handle in handles {
            handle.await.unwrap();
        }

        let elapsed = start.elapsed();

        // If they ran in parallel, should take ~50ms. If serialized, ~100ms.
        // We check that elapsed is less than 100ms to confirm parallel execution.
        assert!(
            elapsed < Duration::from_millis(100),
            "different repos should run in parallel, took {:?}",
            elapsed
        );
    }
}
