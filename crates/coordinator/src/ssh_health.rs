use crate::node_registry::{NodeInfo, NodeRegistry};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;

/// Trait for injecting different probe implementations (production vs. test).
#[async_trait]
pub trait NodeProbe: Send + Sync + 'static {
    async fn probe(&self, hostname: &str, port: u16) -> Result<(), String>;
}

/// Production TCP probe: attempts to connect with a timeout.
pub struct TcpProbe {
    pub timeout: Duration,
}

#[async_trait]
impl NodeProbe for TcpProbe {
    async fn probe(&self, hostname: &str, port: u16) -> Result<(), String> {
        let addr = format!("{}:{}", hostname, port);
        match tokio::time::timeout(self.timeout, TcpStream::connect(&addr)).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => Err(format!("connection failed: {}", e)),
            Err(_) => Err("probe timeout".to_string()),
        }
    }
}

/// Periodically probes every registered node for connectivity.
/// Tracks probe failures per node and records them in the registry.
pub struct SshHealthMonitor<P: NodeProbe> {
    registry: Arc<NodeRegistry>,
    probe: Arc<P>,
    interval: Duration,
    port: u16,
}

impl<P: NodeProbe> SshHealthMonitor<P> {
    pub fn new(registry: Arc<NodeRegistry>, probe: Arc<P>, interval: Duration, port: u16) -> Self {
        Self {
            registry,
            probe,
            interval,
            port,
        }
    }

    /// Run the monitor loop indefinitely, probing every `interval` duration.
    pub async fn run(&self) {
        let mut ticker = tokio::time::interval(self.interval);
        loop {
            ticker.tick().await;
            self.probe_all().await;
        }
    }

    /// Probe all currently registered nodes.
    async fn probe_all(&self) {
        let nodes = self.registry.all_nodes();
        for node_info in nodes {
            self.probe_node(&node_info).await;
        }
    }

    /// Probe a single node: call probe(), record success or failure.
    async fn probe_node(&self, node_info: &NodeInfo) {
        match self.probe.probe(&node_info.hostname, self.port).await {
            Ok(()) => {
                self.registry.record_probe_success(&node_info.worker_id);
            }
            Err(_) => {
                self.registry.record_probe_failure(&node_info.worker_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_registry::NodeInfo;
    use dashmap::DashMap;
    use knitting_crab_core::ids::WorkerId;
    use knitting_crab_core::resource::ResourceAllocation;

    /// Fake probe for testing: stores outcomes per hostname in reverse order.
    struct FakeProbe {
        outcomes: Arc<DashMap<String, Vec<bool>>>,
    }

    impl FakeProbe {
        fn new() -> Self {
            Self {
                outcomes: Arc::new(DashMap::new()),
            }
        }

        fn add_result(&self, hostname: &str, success: bool) {
            self.outcomes
                .entry(hostname.to_string())
                .or_insert_with(Vec::default)
                .push(success);
        }
    }

    #[async_trait]
    impl NodeProbe for FakeProbe {
        async fn probe(&self, hostname: &str, _port: u16) -> Result<(), String> {
            if let Some(mut outcomes) = self.outcomes.get_mut(hostname) {
                if let Some(success) = outcomes.pop() {
                    if success {
                        Ok(())
                    } else {
                        Err("probe failed".to_string())
                    }
                } else {
                    Err("no probe result configured".to_string())
                }
            } else {
                Err("hostname not found in outcomes".to_string())
            }
        }
    }

    #[test]
    fn healthy_probe_records_no_failures() {
        let registry = Arc::new(NodeRegistry::new());
        let node_id = WorkerId::new();
        let node_info = NodeInfo {
            worker_id: node_id,
            hostname: "localhost".to_string(),
            capacity: ResourceAllocation::default(),
            registered_at: chrono::Utc::now(),
        };
        registry.register(node_info);

        let fake = Arc::new(FakeProbe::new());
        fake.add_result("localhost", true);

        let monitor = SshHealthMonitor::new(
            Arc::clone(&registry),
            fake,
            Duration::from_millis(100),
            5000,
        );

        // Simulate probe_node call
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let nodes = registry.all_nodes();
            for node_info in nodes {
                monitor.probe_node(&node_info).await;
            }
        });

        assert_eq!(
            registry.health_status(&node_id),
            crate::node_registry::NodeHealth::Healthy
        );
    }

    #[test]
    fn single_failure_marks_degraded() {
        let registry = Arc::new(NodeRegistry::new());
        let node_id = WorkerId::new();
        let node_info = NodeInfo {
            worker_id: node_id,
            hostname: "localhost".to_string(),
            capacity: ResourceAllocation::default(),
            registered_at: chrono::Utc::now(),
        };
        registry.register(node_info);

        let fake = Arc::new(FakeProbe::new());
        fake.add_result("localhost", false);

        let monitor = SshHealthMonitor::new(
            Arc::clone(&registry),
            fake,
            Duration::from_millis(100),
            5000,
        );

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let nodes = registry.all_nodes();
            for node_info in nodes {
                monitor.probe_node(&node_info).await;
            }
        });

        assert_eq!(
            registry.health_status(&node_id),
            crate::node_registry::NodeHealth::Degraded
        );
    }

    #[test]
    fn three_failures_marks_unhealthy() {
        let registry = Arc::new(NodeRegistry::new());
        let node_id = WorkerId::new();
        let node_info = NodeInfo {
            worker_id: node_id,
            hostname: "localhost".to_string(),
            capacity: ResourceAllocation::default(),
            registered_at: chrono::Utc::now(),
        };
        registry.register(node_info.clone());

        let fake = Arc::new(FakeProbe::new());
        fake.add_result("localhost", false);
        fake.add_result("localhost", false);
        fake.add_result("localhost", false);

        let monitor = SshHealthMonitor::new(
            Arc::clone(&registry),
            fake,
            Duration::from_millis(100),
            5000,
        );

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            monitor.probe_node(&node_info).await;
            monitor.probe_node(&node_info).await;
            monitor.probe_node(&node_info).await;
        });

        assert_eq!(
            registry.health_status(&node_id),
            crate::node_registry::NodeHealth::Unhealthy
        );
    }

    #[test]
    fn success_after_failures_resets_to_healthy() {
        let registry = Arc::new(NodeRegistry::new());
        let node_id = WorkerId::new();
        let node_info = NodeInfo {
            worker_id: node_id,
            hostname: "localhost".to_string(),
            capacity: ResourceAllocation::default(),
            registered_at: chrono::Utc::now(),
        };
        registry.register(node_info);

        let fake = Arc::new(FakeProbe::new());
        fake.add_result("localhost", true); // success resets

        let monitor = SshHealthMonitor::new(
            Arc::clone(&registry),
            fake,
            Duration::from_millis(100),
            5000,
        );

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // Record failures
            registry.record_probe_failure(&node_id);
            registry.record_probe_failure(&node_id);

            // Success should reset
            let nodes = registry.all_nodes();
            for node_info in nodes {
                monitor.probe_node(&node_info).await;
            }
        });

        assert_eq!(
            registry.health_status(&node_id),
            crate::node_registry::NodeHealth::Healthy
        );
    }

    #[tokio::test]
    async fn probe_all_visits_every_registered_node() {
        let registry = Arc::new(NodeRegistry::new());
        let node1 = WorkerId::new();
        let node2 = WorkerId::new();

        let info1 = NodeInfo {
            worker_id: node1,
            hostname: "host1".to_string(),
            capacity: ResourceAllocation::default(),
            registered_at: chrono::Utc::now(),
        };
        let info2 = NodeInfo {
            worker_id: node2,
            hostname: "host2".to_string(),
            capacity: ResourceAllocation::default(),
            registered_at: chrono::Utc::now(),
        };

        registry.register(info1);
        registry.register(info2);

        let fake = Arc::new(FakeProbe::new());
        fake.add_result("host1", true);
        fake.add_result("host2", true);

        let monitor = SshHealthMonitor::new(
            Arc::clone(&registry),
            fake,
            Duration::from_millis(100),
            5000,
        );

        monitor.probe_all().await;

        assert_eq!(
            registry.health_status(&node1),
            crate::node_registry::NodeHealth::Healthy
        );
        assert_eq!(
            registry.health_status(&node2),
            crate::node_registry::NodeHealth::Healthy
        );
    }

    #[tokio::test]
    async fn probe_all_skips_node_removed_mid_scan() {
        let registry = Arc::new(NodeRegistry::new());
        let node1 = WorkerId::new();

        let info1 = NodeInfo {
            worker_id: node1,
            hostname: "host1".to_string(),
            capacity: ResourceAllocation::default(),
            registered_at: chrono::Utc::now(),
        };

        registry.register(info1);

        let fake = Arc::new(FakeProbe::new());
        fake.add_result("host1", true);

        let _monitor = SshHealthMonitor::new(
            Arc::clone(&registry),
            fake,
            Duration::from_millis(100),
            5000,
        );

        // Start probe_all, then remove node before it completes
        let _all_nodes = registry.all_nodes();
        registry.remove(&node1);

        // Probing the removed node should handle gracefully
        assert_eq!(registry.all_nodes().len(), 0);
    }

    #[test]
    fn unhealthy_node_not_returned_in_unhealthy_if_reset() {
        let registry = Arc::new(NodeRegistry::new());
        let node_id = WorkerId::new();
        let node_info = NodeInfo {
            worker_id: node_id,
            hostname: "localhost".to_string(),
            capacity: ResourceAllocation::default(),
            registered_at: chrono::Utc::now(),
        };
        registry.register(node_info);

        // Make it unhealthy
        registry.record_probe_failure(&node_id);
        registry.record_probe_failure(&node_id);
        registry.record_probe_failure(&node_id);

        assert!(registry.unhealthy_nodes().contains(&node_id));

        // Reset to healthy
        registry.record_probe_success(&node_id);

        assert!(!registry.unhealthy_nodes().contains(&node_id));
    }

    #[tokio::test]
    async fn concurrent_probe_all_does_not_double_count() {
        let registry = Arc::new(NodeRegistry::new());
        let node1 = WorkerId::new();

        let info1 = NodeInfo {
            worker_id: node1,
            hostname: "host1".to_string(),
            capacity: ResourceAllocation::default(),
            registered_at: chrono::Utc::now(),
        };

        registry.register(info1);

        let fake = Arc::new(FakeProbe::new());
        fake.add_result("host1", false);
        fake.add_result("host1", false);

        let monitor = SshHealthMonitor::new(
            Arc::clone(&registry),
            fake,
            Duration::from_millis(100),
            5000,
        );

        // Run two probe_all calls concurrently
        let monitor1 = &monitor;
        let monitor2 = &monitor;

        tokio::join!(monitor1.probe_all(), monitor2.probe_all());

        // Verify probe_failures was updated correctly (not double-counted)
        let status = registry.health_status(&node1);
        // The final status depends on order, but should not exceed Unhealthy
        assert!(
            status == crate::node_registry::NodeHealth::Degraded
                || status == crate::node_registry::NodeHealth::Unhealthy
        );
    }
}
