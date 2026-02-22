use chrono::{DateTime, Utc};
use dashmap::DashMap;
use knitting_crab_core::ids::WorkerId;
use knitting_crab_core::resource::ResourceAllocation;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct NodeInfo {
    pub worker_id: WorkerId,
    pub hostname: String,
    pub capacity: ResourceAllocation,
    pub registered_at: DateTime<Utc>,
}

pub struct NodeRegistry {
    nodes: Arc<DashMap<WorkerId, NodeInfo>>,
    last_seen: Arc<DashMap<WorkerId, Instant>>,
}

impl Default for NodeRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl NodeRegistry {
    pub fn new() -> Self {
        Self {
            nodes: Arc::new(DashMap::new()),
            last_seen: Arc::new(DashMap::new()),
        }
    }

    pub fn register(&self, info: NodeInfo) {
        let worker_id = info.worker_id;
        self.nodes.insert(worker_id, info);
        self.last_seen.insert(worker_id, Instant::now());
    }

    pub fn heartbeat(&self, worker_id: &WorkerId) {
        self.last_seen.insert(*worker_id, Instant::now());
    }

    pub fn stale_nodes(&self, timeout: Duration) -> Vec<WorkerId> {
        let now = Instant::now();
        let mut stale = Vec::new();
        for entry in self.last_seen.iter() {
            if now.duration_since(*entry.value()) > timeout {
                stale.push(*entry.key());
            }
        }
        stale
    }

    pub fn remove(&self, worker_id: &WorkerId) {
        self.nodes.remove(worker_id);
        self.last_seen.remove(worker_id);
    }

    pub fn get_node_info(&self, worker_id: &WorkerId) -> Option<NodeInfo> {
        self.nodes.get(worker_id).map(|ref_multi| ref_multi.clone())
    }

    pub fn all_nodes(&self) -> Vec<NodeInfo> {
        self.nodes
            .iter()
            .map(|ref_multi| ref_multi.value().clone())
            .collect()
    }
}

impl Clone for NodeRegistry {
    fn clone(&self) -> Self {
        Self {
            nodes: Arc::clone(&self.nodes),
            last_seen: Arc::clone(&self.last_seen),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_node() {
        let registry = NodeRegistry::new();
        let info = NodeInfo {
            worker_id: WorkerId::new(),
            hostname: "localhost".to_string(),
            capacity: ResourceAllocation::default(),
            registered_at: Utc::now(),
        };
        registry.register(info.clone());
        assert!(registry.get_node_info(&info.worker_id).is_some());
    }

    #[test]
    fn heartbeat_updates_last_seen() {
        let registry = NodeRegistry::new();
        let worker_id = WorkerId::new();
        let info = NodeInfo {
            worker_id: worker_id.clone(),
            hostname: "localhost".to_string(),
            capacity: ResourceAllocation::default(),
            registered_at: Utc::now(),
        };
        registry.register(info);
        let first_time = registry.last_seen.get(&worker_id).map(|r| *r.value());

        // Small delay to ensure time difference
        std::thread::sleep(Duration::from_millis(10));
        registry.heartbeat(&worker_id);
        let second_time = registry.last_seen.get(&worker_id).map(|r| *r.value());

        assert!(second_time > first_time);
    }

    #[test]
    fn stale_nodes_detection() {
        let registry = NodeRegistry::new();
        let worker_id = WorkerId::new();
        let info = NodeInfo {
            worker_id: worker_id.clone(),
            hostname: "localhost".to_string(),
            capacity: ResourceAllocation::default(),
            registered_at: Utc::now(),
        };
        registry.register(info);

        // Manually set last_seen to old time
        registry
            .last_seen
            .insert(worker_id.clone(), Instant::now() - Duration::from_secs(100));

        let stale = registry.stale_nodes(Duration::from_secs(30));
        assert!(stale.contains(&worker_id));
    }

    #[test]
    fn remove_node() {
        let registry = NodeRegistry::new();
        let worker_id = WorkerId::new();
        let info = NodeInfo {
            worker_id: worker_id.clone(),
            hostname: "localhost".to_string(),
            capacity: ResourceAllocation::default(),
            registered_at: Utc::now(),
        };
        registry.register(info);
        registry.remove(&worker_id);
        assert!(registry.get_node_info(&worker_id).is_none());
    }
}
