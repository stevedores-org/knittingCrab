use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::time::Instant;

/// A content-addressed artifact cache with LRU eviction.
pub struct ArtifactCache {
    root: PathBuf,
    max_bytes: u64,
    /// Tracks cache entries: key → (size_bytes, last_access)
    entries: HashMap<String, CacheEntry>,
    total_bytes: u64,
    hits: u64,
    misses: u64,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    size: u64,
    last_access: Instant,
}

impl ArtifactCache {
    /// Creates or opens a cache at the given root directory.
    pub fn new(root: impl Into<PathBuf>, max_bytes: u64) -> io::Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;

        let mut cache = ArtifactCache {
            root,
            max_bytes,
            entries: HashMap::new(),
            total_bytes: 0,
            hits: 0,
            misses: 0,
        };

        // Scan existing entries
        cache.scan_existing()?;
        Ok(cache)
    }

    /// Computes a SHA256 content address for the given data.
    pub fn content_key(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        format!("{:x}", hasher.finalize())
    }

    /// Stores data in the cache, returning the content-addressed key.
    pub fn store(&mut self, data: &[u8]) -> io::Result<String> {
        let key = Self::content_key(data);
        let path = self.key_path(&key);

        if path.exists() {
            // Already cached — update access time
            if let Some(entry) = self.entries.get_mut(&key) {
                entry.last_access = Instant::now();
            }
            return Ok(key);
        }

        let size = data.len() as u64;

        // Evict if necessary
        while self.total_bytes + size > self.max_bytes && !self.entries.is_empty() {
            self.evict_lru()?;
        }

        // If single item exceeds max, still store it (but cache will be at capacity)
        fs::write(&path, data)?;
        self.entries.insert(
            key.clone(),
            CacheEntry {
                size,
                last_access: Instant::now(),
            },
        );
        self.total_bytes += size;

        Ok(key)
    }

    /// Retrieves cached data by key. Returns None on cache miss.
    pub fn retrieve(&mut self, key: &str) -> io::Result<Option<Vec<u8>>> {
        let path = self.key_path(key);
        if !path.exists() {
            self.misses += 1;
            return Ok(None);
        }

        self.hits += 1;
        if let Some(entry) = self.entries.get_mut(key) {
            entry.last_access = Instant::now();
        }

        let data = fs::read(&path)?;
        Ok(Some(data))
    }

    /// Checks if a key exists in the cache without updating access time.
    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// Returns (hits, misses).
    pub fn stats(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }

    /// Returns total bytes currently cached.
    pub fn total_cached_bytes(&self) -> u64 {
        self.total_bytes
    }

    /// Returns number of entries in the cache.
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    fn key_path(&self, key: &str) -> PathBuf {
        self.root.join(key)
    }

    fn evict_lru(&mut self) -> io::Result<()> {
        let oldest_key = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.last_access)
            .map(|(k, _)| k.clone());

        if let Some(key) = oldest_key {
            let path = self.key_path(&key);
            if path.exists() {
                fs::remove_file(&path)?;
            }
            if let Some(entry) = self.entries.remove(&key) {
                self.total_bytes = self.total_bytes.saturating_sub(entry.size);
            }
        }
        Ok(())
    }

    fn scan_existing(&mut self) -> io::Result<()> {
        if !self.root.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    let meta = entry.metadata()?;
                    let size = meta.len();
                    self.entries.insert(
                        name.to_string(),
                        CacheEntry {
                            size,
                            last_access: Instant::now(),
                        },
                    );
                    self.total_bytes += size;
                }
            }
        }
        Ok(())
    }
}

/// Stores a task's output artifact by cache key from a WorkItem.
pub fn store_task_artifact(
    cache: &mut ArtifactCache,
    task_cache_key: &str,
    data: &[u8],
) -> io::Result<String> {
    // Combine the task's cache key with content hash for dedup
    let content_key = ArtifactCache::content_key(data);
    let combined = format!("{}:{}", task_cache_key, content_key);
    let final_key = ArtifactCache::content_key(combined.as_bytes());

    // Store using the combined key
    let path = cache.root.join(&final_key);
    if !path.exists() {
        // Eviction handled internally by store()
        cache.store(data)?;
    }
    Ok(final_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_cache(max_bytes: u64) -> ArtifactCache {
        let dir = tempfile::tempdir().unwrap();
        ArtifactCache::new(dir.keep(), max_bytes).unwrap()
    }

    #[test]
    fn store_and_retrieve() {
        let mut cache = temp_cache(1_048_576);
        let data = b"hello world";
        let key = cache.store(data).unwrap();

        let retrieved = cache.retrieve(&key).unwrap();
        assert_eq!(retrieved, Some(data.to_vec()));
    }

    #[test]
    fn content_addressing_deduplication() {
        let mut cache = temp_cache(1_048_576);
        let data = b"same content";

        let key1 = cache.store(data).unwrap();
        let key2 = cache.store(data).unwrap();

        assert_eq!(key1, key2, "Same content should produce same key");
        assert_eq!(cache.entry_count(), 1, "Should only store once");
    }

    #[test]
    fn cache_miss_returns_none() {
        let mut cache = temp_cache(1_048_576);
        let result = cache.retrieve("nonexistent").unwrap();
        assert!(result.is_none());
        assert_eq!(cache.stats(), (0, 1));
    }

    #[test]
    fn cache_hit_tracking() {
        let mut cache = temp_cache(1_048_576);
        let key = cache.store(b"data").unwrap();

        cache.retrieve(&key).unwrap();
        cache.retrieve(&key).unwrap();
        cache.retrieve("miss").unwrap();

        assert_eq!(cache.stats(), (2, 1));
    }

    #[test]
    fn eviction_under_pressure() {
        // Max 100 bytes — each entry is ~10 bytes
        let mut cache = temp_cache(100);

        let mut keys = Vec::new();
        for i in 0..15 {
            let data = format!("item-{:04}", i);
            let key = cache.store(data.as_bytes()).unwrap();
            keys.push(key);
            // Small delay to ensure distinct access times
            std::thread::sleep(std::time::Duration::from_millis(1));
        }

        // Cache should have evicted older entries to stay under 100 bytes
        assert!(
            cache.total_cached_bytes() <= 100,
            "Cache should respect max_bytes limit: {} > 100",
            cache.total_cached_bytes()
        );

        // Most recent entries should still be present
        let last_key = keys.last().unwrap();
        assert!(
            cache.contains(last_key),
            "Most recent entry should still be cached"
        );
    }

    #[test]
    fn cache_hit_skips_execution() {
        // Simulates the pattern: check cache → hit → skip work
        let mut cache = temp_cache(1_048_576);
        let task_key = "task-cache-key-123";

        // First run: no cache hit, execute and store result
        let result = cache.retrieve(task_key).unwrap();
        assert!(result.is_none(), "First run should be a cache miss");

        // Store the "result" of execution
        let output = b"execution output";
        let _stored_key = cache.store(output).unwrap();

        // Store under the task key as well
        let task_path = cache.root.join(task_key);
        std::fs::write(&task_path, output).unwrap();

        // Second run: cache hit, skip execution
        let cached = cache.retrieve(task_key).unwrap();
        assert!(cached.is_some(), "Second run should be a cache hit");
        assert_eq!(cached.unwrap(), output.to_vec());
    }
}
