use crate::connection::NodeConnection;
use crate::error::NodeResult;
use crate::network_cache::NetworkCacheClient;
use crate::network_event_sink::NetworkEventSink;
use crate::network_lease_store::NetworkLeaseStore;
use crate::network_queue::NetworkQueue;
use knitting_crab_core::ids::WorkerId;
use knitting_crab_core::resource::ResourceAllocation;
use knitting_crab_worker::{FakeWorker, RealProcessExecutor, WorkerRuntime};
use std::net::SocketAddr;
use tokio::time::Duration;

pub struct WorkerNode {
    pub worker_id: WorkerId,
    pub coordinator_addr: SocketAddr,
    pub capacity: ResourceAllocation,
    pub hostname: String,
    pub heartbeat_interval: Duration,
}

impl WorkerNode {
    pub fn new(coordinator_addr: SocketAddr, capacity: ResourceAllocation) -> Self {
        let hostname = std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_string());

        Self {
            worker_id: WorkerId::new(),
            coordinator_addr,
            capacity,
            hostname,
            heartbeat_interval: Duration::from_secs(5),
        }
    }

    pub async fn connect(self) -> NodeResult<ConnectedNode> {
        let conn = NodeConnection::connect(
            self.coordinator_addr,
            self.worker_id,
            self.hostname.clone(),
            self.capacity.clone(),
        )
        .await?;

        conn.start_reconnect_loop();
        conn.start_heartbeat_loop(self.heartbeat_interval);

        Ok(ConnectedNode {
            worker_id: self.worker_id,
            conn,
        })
    }
}

pub struct ConnectedNode {
    pub worker_id: WorkerId,
    conn: NodeConnection,
}

impl ConnectedNode {
    pub fn build_runtime(
        self,
    ) -> WorkerRuntime<
        NetworkQueue,
        NetworkLeaseStore,
        FakeWorker,
        NetworkEventSink,
        RealProcessExecutor,
    > {
        let queue = NetworkQueue::new(self.conn.clone());
        let lease_store = NetworkLeaseStore::new(self.conn.clone());
        let resource_monitor = FakeWorker::new();
        let event_sink = NetworkEventSink::new(self.conn.clone());
        let executor = RealProcessExecutor;

        WorkerRuntime::new(
            self.worker_id,
            queue,
            lease_store,
            resource_monitor,
            event_sink,
            executor,
        )
    }

    pub fn cache_client(&self) -> NetworkCacheClient {
        NetworkCacheClient::new(self.conn.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_node_creation() {
        let addr = "127.0.0.1:9999".parse().unwrap();
        let node = WorkerNode::new(addr, ResourceAllocation::default());
        assert_eq!(node.coordinator_addr, addr);
    }

    #[test]
    fn worker_node_uses_env_hostname() {
        std::env::set_var("HOSTNAME", "test-host");
        let addr = "127.0.0.1:9999".parse().unwrap();
        let node = WorkerNode::new(addr, ResourceAllocation::default());
        assert_eq!(node.hostname, "test-host");
    }
}
