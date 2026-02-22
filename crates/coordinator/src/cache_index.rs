use dashmap::DashMap;
use knitting_crab_core::ids::WorkerId;
use knitting_crab_transport::CacheLocation;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct CacheIndexEntry {
    pub worker_id: WorkerId,
    pub hostname: String,
    pub path: PathBuf,
}

pub struct CacheIndex {
    entries: Arc<DashMap<String, Vec<CacheIndexEntry>>>,
}

impl Default for CacheIndex {
    fn default() -> Self {
        Self::new()
    }
}

impl CacheIndex {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(DashMap::new()),
        }
    }

    pub fn announce(&self, key: String, entry: CacheIndexEntry) {
        self.entries.entry(key).or_default().push(entry);
    }

    pub fn query(&self, key: &str) -> Vec<CacheLocation> {
        self.entries
            .get(key)
            .map(|ref_multi| {
                ref_multi
                    .iter()
                    .map(|e| CacheLocation {
                        worker_id: e.worker_id,
                        hostname: e.hostname.clone(),
                        path: e.path.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn evict_node(&self, worker_id: &WorkerId) {
        for mut entry in self.entries.iter_mut() {
            entry.value_mut().retain(|e| e.worker_id != *worker_id);
        }
        // Clean up empty entries
        self.entries.retain(|_, v| !v.is_empty());
    }
}

impl Clone for CacheIndex {
    fn clone(&self) -> Self {
        Self {
            entries: Arc::clone(&self.entries),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announce_and_query() {
        let cache = CacheIndex::new();
        let worker_id = WorkerId::new();
        let entry = CacheIndexEntry {
            worker_id: worker_id.clone(),
            hostname: "host1".to_string(),
            path: PathBuf::from("/cache/data"),
        };
        cache.announce("key1".to_string(), entry);
        let results = cache.query("key1");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].hostname, "host1");
    }

    #[test]
    fn query_nonexistent_returns_empty() {
        let cache = CacheIndex::new();
        let results = cache.query("nonexistent");
        assert!(results.is_empty());
    }

    #[test]
    fn evict_node_removes_entries() {
        let cache = CacheIndex::new();
        let worker_id = WorkerId::new();
        let entry = CacheIndexEntry {
            worker_id: worker_id.clone(),
            hostname: "host1".to_string(),
            path: PathBuf::from("/cache/data"),
        };
        cache.announce("key1".to_string(), entry);
        cache.evict_node(&worker_id);
        let results = cache.query("key1");
        assert!(results.is_empty());
    }

    #[test]
    fn multiple_locations_for_same_key() {
        let cache = CacheIndex::new();
        let worker1 = WorkerId::new();
        let worker2 = WorkerId::new();
        cache.announce(
            "key1".to_string(),
            CacheIndexEntry {
                worker_id: worker1,
                hostname: "host1".to_string(),
                path: PathBuf::from("/cache/data"),
            },
        );
        cache.announce(
            "key1".to_string(),
            CacheIndexEntry {
                worker_id: worker2,
                hostname: "host2".to_string(),
                path: PathBuf::from("/cache/data"),
            },
        );
        let results = cache.query("key1");
        assert_eq!(results.len(), 2);
    }
}
