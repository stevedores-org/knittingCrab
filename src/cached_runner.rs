use crate::cache::ArtifactCache;
use crate::work_item::WorkItem;
use crate::worker::{TaskResult, Worker};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Caches build outputs (like cargo target directories) to accelerate subsequent runs.
/// When a task is executed, outputs matching the cache key are restored before execution,
/// and new outputs are cached after successful execution.
pub struct CachedTaskRunner<W: Worker> {
    inner: W,
    cache: ArtifactCache,
    /// Paths relative to task working_dir to cache/restore (e.g., ["target", "node_modules"])
    cache_paths: Vec<String>,
}

impl<W: Worker> CachedTaskRunner<W> {
    /// Creates a new cached task runner.
    /// `cache_dir` is where build artifacts will be stored.
    /// `cache_size_bytes` is the maximum cache size before LRU eviction.
    pub fn new(inner: W, cache_dir: impl Into<PathBuf>, cache_size_bytes: u64) -> io::Result<Self> {
        let cache = ArtifactCache::new(cache_dir, cache_size_bytes)?;
        Ok(CachedTaskRunner {
            inner,
            cache,
            cache_paths: vec!["target".to_string()], // Default: cache cargo target directory
        })
    }

    /// Sets which paths to cache/restore (relative to working_dir).
    /// Example: `["target", "node_modules", ".next"]`
    pub fn with_cache_paths(mut self, paths: Vec<String>) -> Self {
        self.cache_paths = paths;
        self
    }

    /// Restores cached artifacts for a task (before execution).
    /// Returns true if cache was hit, false if miss or error.
    #[allow(unused)]
    fn restore_cache(&mut self, task_id: u64, cache_key: &str, _work_dir: &Path) -> bool {
        // Try to retrieve the cached tarball for this task + cache key
        let cache_id = format!("{}_{}", task_id, cache_key);

        match self.cache.retrieve(&cache_id) {
            Ok(Some(_tarball_data)) => {
                // Successfully retrieved cached tarball
                // In a real implementation, this would extract the tarball
                // For MVP, we just return true to indicate cache hit
                // Actual extraction would restore the paths like "target/"
                true
            }
            Ok(None) => {
                // Cache miss
                false
            }
            Err(_) => {
                // Error reading cache, treat as miss
                false
            }
        }
    }

    /// Caches task outputs after successful execution.
    /// Collects files from cache_paths and stores them.
    #[allow(unused)]
    fn cache_outputs(&mut self, task_id: u64, cache_key: &str, work_dir: &Path) -> io::Result<()> {
        let _cache_id = format!("{}_{}", task_id, cache_key);

        // Collect outputs from configured cache_paths
        let mut total_size = 0u64;
        let mut _output_data: Vec<u8> = Vec::new();

        for path_str in &self.cache_paths {
            let full_path = work_dir.join(path_str);
            if full_path.exists() {
                // In a real implementation, this would tar/zip the directory
                // For MVP, we just estimate size
                if let Ok(metadata) = fs::metadata(&full_path) {
                    total_size += metadata.len();
                }
            }
        }

        if total_size > 0 {
            // Store a placeholder that represents the cached outputs
            // Real implementation would compress and store actual files
            let placeholder = format!(
                "cached_outputs for task {} with {} bytes from paths: {:?}",
                task_id, total_size, self.cache_paths
            );
            let _ = self.cache.store(placeholder.as_bytes());
        }

        Ok(())
    }
}

impl<W: Worker> Worker for CachedTaskRunner<W> {
    async fn execute(&self, task: &WorkItem) -> TaskResult {
        // Note: In a real implementation, we'd:
        // 1. Compute cache key from task (repo SHA, command, env, toolchain)
        // 2. Check if cache hit exists
        // 3. Restore cache if hit
        // 4. Execute task
        // 5. Store outputs to cache on success

        // For MVP, we just delegate to inner worker
        // The cache integration would happen at scheduler level
        self.inner.execute(task).await
    }
}

/// Statistics about cache performance.
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    /// Total cache hits
    pub hits: u64,
    /// Total cache misses
    pub misses: u64,
    /// Total bytes stored in cache
    pub total_bytes: u64,
    /// Total execution time saved by cache hits (in seconds)
    pub time_saved_secs: f64,
}

impl CacheStats {
    /// Hit rate as a percentage (0-100)
    pub fn hit_rate_percent(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            (self.hits as f64 / total as f64) * 100.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work_item::Priority;
    use crate::worker::FakeWorker;
    use std::time::Duration;

    fn sample_task(id: u64) -> WorkItem {
        WorkItem::new_simple(
            id,
            "cargo build",
            "test-repo",
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

    #[test]
    fn cache_stats_hit_rate() {
        let stats = CacheStats {
            hits: 75,
            misses: 25,
            ..Default::default()
        };

        assert_eq!(stats.hit_rate_percent(), 75.0);
    }

    #[test]
    fn cache_stats_zero_requests() {
        let stats = CacheStats::default();
        assert_eq!(stats.hit_rate_percent(), 0.0);
    }

    #[test]
    fn cache_stats_all_hits() {
        let stats = CacheStats {
            hits: 100,
            misses: 0,
            ..Default::default()
        };

        assert_eq!(stats.hit_rate_percent(), 100.0);
    }

    #[tokio::test]
    async fn cached_runner_delegates_execution() {
        use tempfile::TempDir;

        let temp_cache = TempDir::new().unwrap();
        let inner = FakeWorker::always_succeed(Duration::from_millis(1));
        let runner = CachedTaskRunner::new(inner, temp_cache.path(), 1_000_000).unwrap();

        let task = sample_task(1);
        let result = runner.execute(&task).await;

        assert!(result.success);
    }

    #[test]
    fn cache_runner_with_custom_paths() {
        use tempfile::TempDir;

        let temp_cache = TempDir::new().unwrap();
        let inner = FakeWorker::always_succeed(Duration::from_millis(1));
        let runner = CachedTaskRunner::new(inner, temp_cache.path(), 1_000_000)
            .unwrap()
            .with_cache_paths(vec![
                "target".to_string(),
                "node_modules".to_string(),
                ".next".to_string(),
            ]);

        assert_eq!(runner.cache_paths.len(), 3);
        assert!(runner.cache_paths.contains(&"node_modules".to_string()));
    }

    #[test]
    fn cache_paths_configuration() {
        use tempfile::TempDir;

        let temp_cache = TempDir::new().unwrap();
        let inner = FakeWorker::always_succeed(Duration::from_millis(1));

        let runner = CachedTaskRunner::new(inner, temp_cache.path(), 1_000_000).unwrap();

        // Default should include target
        assert!(runner.cache_paths.contains(&"target".to_string()));
    }
}
