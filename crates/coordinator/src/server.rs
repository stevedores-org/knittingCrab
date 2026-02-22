use crate::error::{CoordinatorError, CoordinatorResult};
use crate::state::CoordinatorState;
use knitting_crab_core::error::CoreError;
use knitting_crab_core::ids::WorkerId;
use knitting_crab_core::traits::Queue;
use knitting_crab_transport::{CoordinatorRequest, CoordinatorResponse, FramedTransport};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::interval;

pub struct CoordinatorServer {
    state: CoordinatorState,
    addr: SocketAddr,
}

impl CoordinatorServer {
    pub fn new(state: CoordinatorState, addr: SocketAddr) -> Self {
        Self { state, addr }
    }

    pub async fn serve(self) -> CoordinatorResult<()> {
        let listener = TcpListener::bind(self.addr)
            .await
            .map_err(|e| CoordinatorError::Internal(format!("bind error: {}", e)))?;

        let state_clone = self.state.clone();
        tokio::spawn(async move {
            let _ = Self::node_reaper_loop(
                state_clone,
                Duration::from_secs(60),
                Duration::from_secs(10),
            )
            .await;
        });

        loop {
            let (stream, _) = listener
                .accept()
                .await
                .map_err(|e| CoordinatorError::Internal(format!("accept error: {}", e)))?;

            let state = self.state.clone();
            tokio::spawn(async move {
                let _ = Self::handle_connection(state, stream).await;
            });
        }
    }

    async fn handle_connection(
        state: CoordinatorState,
        stream: TcpStream,
    ) -> CoordinatorResult<()> {
        let mut transport = FramedTransport::new(stream);
        let mut registered_worker_id: Option<WorkerId> = None;

        loop {
            match transport.recv_message::<CoordinatorRequest>().await {
                Ok(Some(req)) => {
                    let resp = Self::handle_request(&state, req, &mut registered_worker_id).await;
                    transport
                        .send_message(&resp)
                        .await
                        .map_err(|e| CoordinatorError::Transport(format!("{}", e)))?;
                }
                Ok(None) => {
                    // Clean EOF: deregister node if registered
                    if let Some(worker_id) = registered_worker_id {
                        state.node_registry.remove(&worker_id);
                        state.cache_index.evict_node(&worker_id);
                    }
                    break;
                }
                Err(e) => {
                    if let Some(worker_id) = registered_worker_id {
                        state.node_registry.remove(&worker_id);
                        state.cache_index.evict_node(&worker_id);
                    }
                    return Err(CoordinatorError::Transport(format!("{}", e)));
                }
            }
        }
        Ok(())
    }

    async fn handle_request(
        state: &CoordinatorState,
        req: CoordinatorRequest,
        registered_worker_id: &mut Option<WorkerId>,
    ) -> CoordinatorResponse {
        match req {
            CoordinatorRequest::RegisterNode {
                worker_id,
                hostname,
                capacity,
            } => {
                *registered_worker_id = Some(worker_id);
                state
                    .node_registry
                    .register(crate::node_registry::NodeInfo {
                        worker_id,
                        hostname,
                        capacity,
                        registered_at: chrono::Utc::now(),
                    });
                CoordinatorResponse::Ok
            }
            CoordinatorRequest::NodeHeartbeat { worker_id } => {
                state.node_registry.heartbeat(&worker_id);
                CoordinatorResponse::Ok
            }
            CoordinatorRequest::Dequeue { worker_id } => {
                match state.dequeue_task(worker_id).await {
                    Ok(task) => CoordinatorResponse::Dequeued(Box::new(task)),
                    Err(e) => CoordinatorResponse::Error(format!("dequeue failed: {}", e)),
                }
            }
            CoordinatorRequest::Requeue { task_id, attempt } => {
                match state.queue.requeue(task_id, attempt).await {
                    Ok(()) => CoordinatorResponse::Ok,
                    Err(e) => CoordinatorResponse::Error(format!("requeue failed: {}", e)),
                }
            }
            CoordinatorRequest::Discard { task_id } => match state.queue.discard(task_id).await {
                Ok(()) => CoordinatorResponse::Ok,
                Err(e) => CoordinatorResponse::Error(format!("discard failed: {}", e)),
            },
            CoordinatorRequest::InsertLease { lease } => match state.insert_lease(lease).await {
                Ok(()) => CoordinatorResponse::Ok,
                Err(CoreError::AlreadyLeased) => {
                    CoordinatorResponse::Error("already leased".to_string())
                }
                Err(e) => CoordinatorResponse::Error(format!("insert lease failed: {}", e)),
            },
            CoordinatorRequest::GetLease { task_id } => match state.get_lease(task_id).await {
                Ok(lease) => CoordinatorResponse::Lease(lease),
                Err(e) => CoordinatorResponse::Error(format!("get lease failed: {}", e)),
            },
            CoordinatorRequest::UpdateLease { lease } => match state.update_lease(lease).await {
                Ok(()) => CoordinatorResponse::Ok,
                Err(e) => CoordinatorResponse::Error(format!("update lease failed: {}", e)),
            },
            CoordinatorRequest::RemoveLease { task_id } => {
                match state.remove_lease(task_id).await {
                    Ok(()) => CoordinatorResponse::Ok,
                    Err(e) => CoordinatorResponse::Error(format!("remove lease failed: {}", e)),
                }
            }
            CoordinatorRequest::ActiveLeases => match state.active_leases().await {
                Ok(leases) => CoordinatorResponse::Leases(leases),
                Err(e) => CoordinatorResponse::Error(format!("active leases failed: {}", e)),
            },
            CoordinatorRequest::EmitEvent { event: _ } => {
                // Event sink not yet implemented; fire and forget
                CoordinatorResponse::Ok
            }
            CoordinatorRequest::EmitLog { log: _ } => {
                // Log sink not yet implemented; fire and forget
                CoordinatorResponse::Ok
            }
            CoordinatorRequest::QueryCache { cache_key } => {
                let locations = state.cache_index.query(&cache_key);
                CoordinatorResponse::CacheLocations(locations)
            }
            CoordinatorRequest::AnnounceCache { cache_key, path } => {
                if let Some(worker_id) = registered_worker_id {
                    if let Some(node_info) = state.node_registry.get_node_info(worker_id) {
                        let entry = crate::cache_index::CacheIndexEntry {
                            worker_id: node_info.worker_id,
                            hostname: node_info.hostname,
                            path,
                        };
                        state.cache_index.announce(cache_key, entry);
                        CoordinatorResponse::Ok
                    } else {
                        CoordinatorResponse::Error("node not found".to_string())
                    }
                } else {
                    CoordinatorResponse::Error("not registered".to_string())
                }
            }
        }
    }

    async fn node_reaper_loop(
        state: CoordinatorState,
        timeout: Duration,
        interval_dur: Duration,
    ) -> CoordinatorResult<()> {
        let mut ticker = interval(interval_dur);
        loop {
            ticker.tick().await;
            let stale = state.node_registry.stale_nodes(timeout);
            for worker_id in stale {
                // Requeue active leases for this node
                if let Ok(leases) = state.active_leases().await {
                    for lease in leases {
                        if lease.worker_id == worker_id {
                            let _ = state.queue.requeue(lease.task_id, lease.attempt + 1).await;
                        }
                    }
                }
                state.node_registry.remove(&worker_id);
                state.cache_index.evict_node(&worker_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use knitting_crab_core::lease::Lease;
    use std::path::PathBuf;
    use std::time::Duration;

    #[tokio::test]
    async fn register_node_returns_ok() {
        let state = CoordinatorState::new();
        let req = CoordinatorRequest::RegisterNode {
            worker_id: WorkerId::new(),
            hostname: "localhost".to_string(),
            capacity: Default::default(),
        };
        let mut worker_id = None;
        let resp = CoordinatorServer::handle_request(&state, req, &mut worker_id).await;
        assert!(matches!(resp, CoordinatorResponse::Ok));
    }

    #[tokio::test]
    async fn dequeue_empty_returns_none() {
        let state = CoordinatorState::new();
        let req = CoordinatorRequest::Dequeue {
            worker_id: WorkerId::new(),
        };
        let mut worker_id = None;
        let resp = CoordinatorServer::handle_request(&state, req, &mut worker_id).await;
        assert!(matches!(resp, CoordinatorResponse::Dequeued(box None)));
    }

    #[tokio::test]
    async fn insert_lease_succeeds() {
        let state = CoordinatorState::new();
        let lease = Lease::new(
            knitting_crab_core::ids::TaskId::new(),
            WorkerId::new(),
            Duration::from_secs(30),
            0,
        );
        let req = CoordinatorRequest::InsertLease {
            lease: lease.clone(),
        };
        let mut worker_id = None;
        let resp = CoordinatorServer::handle_request(&state, req, &mut worker_id).await;
        assert!(matches!(resp, CoordinatorResponse::Ok));

        // Try to insert the same lease again
        let req2 = CoordinatorRequest::InsertLease { lease };
        let resp2 = CoordinatorServer::handle_request(&state, req2, &mut worker_id).await;
        assert!(matches!(resp2, CoordinatorResponse::Error(ref s) if s.contains("already")));
    }

    #[tokio::test]
    async fn query_cache_returns_locations() {
        let state = CoordinatorState::new();
        let worker_id = WorkerId::new();
        let req = CoordinatorRequest::RegisterNode {
            worker_id,
            hostname: "host1".to_string(),
            capacity: Default::default(),
        };
        let mut registered = Some(worker_id);
        CoordinatorServer::handle_request(&state, req, &mut registered).await;

        let req2 = CoordinatorRequest::AnnounceCache {
            cache_key: "key1".to_string(),
            path: PathBuf::from("/cache/data"),
        };
        CoordinatorServer::handle_request(&state, req2, &mut registered).await;

        let req3 = CoordinatorRequest::QueryCache {
            cache_key: "key1".to_string(),
        };
        let resp = CoordinatorServer::handle_request(&state, req3, &mut None).await;
        assert!(matches!(resp, CoordinatorResponse::CacheLocations(ref locs) if locs.len() == 1));
    }
}
