use chrono::{DateTime, Utc};
use dashmap::DashMap;
use knitting_crab_core::ids::WorkerId;
use knitting_crab_core::resource::ResourceAllocation;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeHealth {
    Healthy,
    Degraded,
    Unhealthy,
}

const DEGRADED_THRESHOLD: u32 = 1;
const UNHEALTHY_THRESHOLD: u32 = 3;

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
    probe_failures: Arc<DashMap<WorkerId, u32>>,
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
            probe_failures: Arc::new(DashMap::new()),
        }
    }

    pub fn register(&self, info: NodeInfo) {
        let worker_id = info.worker_id;
        self.nodes.insert(worker_id, info);
        self.last_seen.insert(worker_id, Instant::now());
        self.probe_failures.insert(worker_id, 0);
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
        self.probe_failures.remove(worker_id);
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

    pub fn record_probe_success(&self, id: &WorkerId) {
        self.probe_failures.insert(*id, 0);
    }

    pub fn record_probe_failure(&self, id: &WorkerId) {
        let current = self.probe_failures.get(id).map(|r| *r.value()).unwrap_or(0);
        self.probe_failures.insert(*id, current.saturating_add(1));
    }

    pub fn health_status(&self, id: &WorkerId) -> NodeHealth {
        let failures = self.probe_failures.get(id).map(|r| *r.value()).unwrap_or(0);

        if failures >= UNHEALTHY_THRESHOLD {
            NodeHealth::Unhealthy
        } else if failures >= DEGRADED_THRESHOLD {
            NodeHealth::Degraded
        } else {
            NodeHealth::Healthy
        }
    }

    pub fn unhealthy_nodes(&self) -> Vec<WorkerId> {
        let mut unhealthy = Vec::new();
        for entry in self.probe_failures.iter() {
            if *entry.value() >= UNHEALTHY_THRESHOLD {
                unhealthy.push(*entry.key());
            }
        }
        unhealthy
    }
}

impl Clone for NodeRegistry {
    fn clone(&self) -> Self {
        Self {
            nodes: Arc::clone(&self.nodes),
            last_seen: Arc::clone(&self.last_seen),
            probe_failures: Arc::clone(&self.probe_failures),
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
            worker_id,
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
            worker_id,
            hostname: "localhost".to_string(),
            capacity: ResourceAllocation::default(),
            registered_at: Utc::now(),
        };
        registry.register(info);

        // Manually set last_seen to old time
        registry
            .last_seen
            .insert(worker_id, Instant::now() - Duration::from_secs(100));

        let stale = registry.stale_nodes(Duration::from_secs(30));
        assert!(stale.contains(&worker_id));
    }

    #[test]
    fn remove_node() {
        let registry = NodeRegistry::new();
        let worker_id = WorkerId::new();
        let info = NodeInfo {
            worker_id,
            hostname: "localhost".to_string(),
            capacity: ResourceAllocation::default(),
            registered_at: Utc::now(),
        };
        registry.register(info);
        registry.remove(&worker_id);
        assert!(registry.get_node_info(&worker_id).is_none());
    }

    #[test]
    fn health_starts_healthy_after_register() {
        let registry = NodeRegistry::new();
        let worker_id = WorkerId::new();
        let info = NodeInfo {
            worker_id,
            hostname: "localhost".to_string(),
            capacity: ResourceAllocation::default(),
            registered_at: Utc::now(),
        };
        registry.register(info);
        assert_eq!(registry.health_status(&worker_id), NodeHealth::Healthy);
    }

    #[test]
    fn degraded_after_one_probe_failure() {
        let registry = NodeRegistry::new();
        let worker_id = WorkerId::new();
        let info = NodeInfo {
            worker_id,
            hostname: "localhost".to_string(),
            capacity: ResourceAllocation::default(),
            registered_at: Utc::now(),
        };
        registry.register(info);
        registry.record_probe_failure(&worker_id);
        assert_eq!(registry.health_status(&worker_id), NodeHealth::Degraded);
    }

    #[test]
    fn unhealthy_after_three_probe_failures() {
        let registry = NodeRegistry::new();
        let worker_id = WorkerId::new();
        let info = NodeInfo {
            worker_id,
            hostname: "localhost".to_string(),
            capacity: ResourceAllocation::default(),
            registered_at: Utc::now(),
        };
        registry.register(info);
        registry.record_probe_failure(&worker_id);
        registry.record_probe_failure(&worker_id);
        registry.record_probe_failure(&worker_id);
        assert_eq!(registry.health_status(&worker_id), NodeHealth::Unhealthy);
    }

    #[test]
    fn reset_to_healthy_after_probe_success() {
        let registry = NodeRegistry::new();
        let worker_id = WorkerId::new();
        let info = NodeInfo {
            worker_id,
            hostname: "localhost".to_string(),
            capacity: ResourceAllocation::default(),
            registered_at: Utc::now(),
        };
        registry.register(info);
        registry.record_probe_failure(&worker_id);
        registry.record_probe_failure(&worker_id);
        assert_eq!(registry.health_status(&worker_id), NodeHealth::Degraded);

        registry.record_probe_success(&worker_id);
        assert_eq!(registry.health_status(&worker_id), NodeHealth::Healthy);
    }
}
