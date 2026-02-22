use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Computes stable, deterministic cache keys for task execution.
/// Cache keys are based on: repository commit SHA, task command, environment, and toolchain.
#[derive(Debug, Clone)]
pub struct CacheKeyBuilder {
    repo_sha: String,              // Git commit SHA of the repository
    goal: String,                  // Task command/goal
    env: BTreeMap<String, String>, // Filtered environment variables
    toolchain: String,             // Rust toolchain version or similar
}

impl CacheKeyBuilder {
    /// Creates a new cache key builder with required fields.
    pub fn new(repo_sha: impl Into<String>, goal: impl Into<String>) -> Self {
        CacheKeyBuilder {
            repo_sha: repo_sha.into(),
            goal: goal.into(),
            env: BTreeMap::new(),
            toolchain: String::new(),
        }
    }

    /// Adds environment variables to the cache key (deterministic via BTreeMap).
    pub fn with_env(mut self, vars: impl IntoIterator<Item = (String, String)>) -> Self {
        for (key, val) in vars {
            self.env.insert(key, val);
        }
        self
    }

    /// Sets the toolchain version (e.g., "1.75.0" or "nightly-2024-02-21").
    pub fn with_toolchain(mut self, toolchain: impl Into<String>) -> Self {
        self.toolchain = toolchain.into();
        self
    }

    /// Computes the final SHA256 cache key.
    /// The key includes all components in a deterministic order.
    pub fn build(&self) -> String {
        let mut hasher = Sha256::new();

        // Hash in a deterministic order: repo_sha, goal, toolchain, then env vars
        hasher.update(self.repo_sha.as_bytes());
        hasher.update(b"|");
        hasher.update(self.goal.as_bytes());
        hasher.update(b"|");
        hasher.update(self.toolchain.as_bytes());
        hasher.update(b"|");

        // Environment variables are in BTreeMap for deterministic ordering
        for (key, val) in &self.env {
            hasher.update(key.as_bytes());
            hasher.update(b"=");
            hasher.update(val.as_bytes());
            hasher.update(b";");
        }

        format!("{:x}", hasher.finalize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_inputs_produce_same_key() {
        let key1 = CacheKeyBuilder::new("abc123", "cargo test")
            .with_toolchain("1.75.0")
            .with_env(vec![("VAR1".to_string(), "value1".to_string())])
            .build();

        let key2 = CacheKeyBuilder::new("abc123", "cargo test")
            .with_toolchain("1.75.0")
            .with_env(vec![("VAR1".to_string(), "value1".to_string())])
            .build();

        assert_eq!(key1, key2);
    }

    #[test]
    fn different_repo_sha_produces_different_key() {
        let key1 = CacheKeyBuilder::new("abc123", "cargo test")
            .with_toolchain("1.75.0")
            .build();

        let key2 = CacheKeyBuilder::new("def456", "cargo test")
            .with_toolchain("1.75.0")
            .build();

        assert_ne!(key1, key2);
    }

    #[test]
    fn different_goal_produces_different_key() {
        let key1 = CacheKeyBuilder::new("abc123", "cargo test")
            .with_toolchain("1.75.0")
            .build();

        let key2 = CacheKeyBuilder::new("abc123", "cargo build")
            .with_toolchain("1.75.0")
            .build();

        assert_ne!(key1, key2);
    }

    #[test]
    fn different_toolchain_produces_different_key() {
        let key1 = CacheKeyBuilder::new("abc123", "cargo test")
            .with_toolchain("1.75.0")
            .build();

        let key2 = CacheKeyBuilder::new("abc123", "cargo test")
            .with_toolchain("1.76.0")
            .build();

        assert_ne!(key1, key2);
    }

    #[test]
    fn different_env_produces_different_key() {
        let key1 = CacheKeyBuilder::new("abc123", "cargo test")
            .with_toolchain("1.75.0")
            .with_env(vec![("VAR1".to_string(), "value1".to_string())])
            .build();

        let key2 = CacheKeyBuilder::new("abc123", "cargo test")
            .with_toolchain("1.75.0")
            .with_env(vec![("VAR1".to_string(), "value2".to_string())])
            .build();

        assert_ne!(key1, key2);
    }

    #[test]
    fn env_vars_are_order_independent() {
        // BTreeMap ensures deterministic ordering regardless of insertion order
        let key1 = CacheKeyBuilder::new("abc123", "cargo test")
            .with_toolchain("1.75.0")
            .with_env(vec![
                ("VAR1".to_string(), "value1".to_string()),
                ("VAR2".to_string(), "value2".to_string()),
            ])
            .build();

        let key2 = CacheKeyBuilder::new("abc123", "cargo test")
            .with_toolchain("1.75.0")
            .with_env(vec![
                ("VAR2".to_string(), "value2".to_string()),
                ("VAR1".to_string(), "value1".to_string()),
            ])
            .build();

        assert_eq!(key1, key2);
    }

    #[test]
    fn cache_key_is_stable_sha256() {
        let key = CacheKeyBuilder::new("abc123", "cargo test")
            .with_toolchain("1.75.0")
            .build();

        // Should be a valid hex string of SHA256 (64 characters)
        assert_eq!(key.len(), 64);
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
