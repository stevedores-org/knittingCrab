use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

/// The composite key that uniquely identifies a cacheable computation.
#[derive(Debug, Clone)]
pub struct CacheKey {
    /// SHA of the repository state.
    pub repo_sha: String,
    /// Stringified command vector.
    pub command: String,
    /// Hash of the environment variable map.
    pub env_hash: String,
    /// Toolchain version string (e.g. `"rustc 1.78.0"`).
    pub toolchain_version: String,
}

impl CacheKey {
    /// Constructs a `CacheKey`, hashing the environment map into a hex string.
    pub fn new(
        repo_sha: &str,
        command: &[String],
        env: &HashMap<String, String>,
        toolchain_version: &str,
    ) -> Self {
        // Deterministic env hash: sort keys, then sha256.
        let mut pairs: Vec<(&String, &String)> = env.iter().collect();
        pairs.sort_by_key(|(k, _)| *k);
        let mut env_hasher = Sha256::new();
        for (k, v) in &pairs {
            env_hasher.update(k.as_bytes());
            env_hasher.update(b"=");
            env_hasher.update(v.as_bytes());
            env_hasher.update(b"\n");
        }
        let env_hash = hex::encode(env_hasher.finalize());

        Self {
            repo_sha: repo_sha.to_owned(),
            command: command.join(" "),
            env_hash,
            toolchain_version: toolchain_version.to_owned(),
        }
    }

    /// Returns the SHA-256 hex digest of all key fields concatenated.
    pub fn to_hash(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.repo_sha.as_bytes());
        hasher.update(b"|");
        hasher.update(self.command.as_bytes());
        hasher.update(b"|");
        hasher.update(self.env_hash.as_bytes());
        hasher.update(b"|");
        hasher.update(self.toolchain_version.as_bytes());
        hex::encode(hasher.finalize())
    }
}

/// A stored computation result in the artifact cache.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    /// The hash of the [`CacheKey`] that produced this entry.
    pub key_hash: String,
    /// When the entry was inserted.
    pub created_at: SystemTime,
    /// Process exit code of the cached run.
    pub exit_code: i32,
    /// Up to 4 KiB of stdout from the run.
    pub stdout_snippet: String,
    /// Paths of any artifacts stored on disk.
    pub artifacts: Vec<String>,
}

/// A bounded, time-limited in-memory cache of [`CacheEntry`] values, keyed by
/// the hex digest of a [`CacheKey`].
pub struct ArtifactCache {
    entries: Arc<Mutex<HashMap<String, CacheEntry>>>,
    max_entries: usize,
    retention_secs: u64,
}

impl ArtifactCache {
    /// Creates a new cache with the given capacity and retention policy.
    pub fn new(max_entries: usize, retention_secs: u64) -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
            max_entries,
            retention_secs,
        }
    }

    /// Looks up an entry by key, returning a clone if found (and not expired).
    pub fn get(&self, key: &CacheKey) -> Option<CacheEntry> {
        let hash = key.to_hash();
        let map = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let entry = map.get(&hash)?;
        // Validate age.
        if let Ok(age) = entry.created_at.elapsed() {
            if age > Duration::from_secs(self.retention_secs) {
                return None;
            }
        }
        Some(entry.clone())
    }

    /// Stores `entry` under the hash of `key`.
    ///
    /// If the cache is full the oldest entry (by `created_at`) is evicted to
    /// make room before inserting.
    pub fn put(&self, key: CacheKey, entry: CacheEntry) {
        let hash = key.to_hash();
        let mut map = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        // Evict oldest if at capacity.
        if map.len() >= self.max_entries && !map.contains_key(&hash) {
            if let Some(oldest_key) = map
                .iter()
                .min_by_key(|(_, e)| e.created_at)
                .map(|(k, _)| k.clone())
            {
                map.remove(&oldest_key);
            }
        }
        map.insert(hash, entry);
    }

    /// Removes all entries whose age exceeds the configured `retention_secs`.
    pub fn evict_expired(&self) {
        let mut map = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let retention = Duration::from_secs(self.retention_secs);
        map.retain(|_, e| {
            e.created_at
                .elapsed()
                .map(|age| age <= retention)
                .unwrap_or(true)
        });
    }

    /// Returns the number of entries currently stored.
    pub fn len(&self) -> usize {
        let map = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        map.len()
    }

    /// Returns `true` if the cache contains no entries.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
