use crate::error::{NodeError, NodeResult};
use knitting_crab_core::ids::WorkerId;
use knitting_crab_core::resource::ResourceAllocation;
use knitting_crab_transport::{CoordinatorRequest, CoordinatorResponse, FramedTransport};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::time::{interval, sleep, Duration};

#[derive(Clone)]
pub struct NodeConnection {
    inner: Arc<Mutex<Option<FramedTransport>>>,
    coordinator_addr: SocketAddr,
    worker_id: WorkerId,
    hostname: String,
    capacity: ResourceAllocation,
}

impl NodeConnection {
    pub async fn connect(
        addr: SocketAddr,
        worker_id: WorkerId,
        hostname: String,
        capacity: ResourceAllocation,
    ) -> NodeResult<Self> {
        let stream = TcpStream::connect(addr)
            .await
            .map_err(|e| NodeError::Connection(format!("failed to connect: {}", e)))?;

        let mut transport = FramedTransport::new(stream);

        // Send RegisterNode request
        let req = CoordinatorRequest::RegisterNode {
            worker_id,
            hostname: hostname.clone(),
            capacity: capacity.clone(),
        };
        transport
            .send_message(&req)
            .await
            .map_err(|e| NodeError::Transport(format!("{}", e)))?;

        // Verify Ok response
        match transport.recv_message::<CoordinatorResponse>().await {
            Ok(Some(CoordinatorResponse::Ok)) => {
                let conn = Self {
                    inner: Arc::new(Mutex::new(Some(transport))),
                    coordinator_addr: addr,
                    worker_id,
                    hostname,
                    capacity,
                };
                Ok(conn)
            }
            Ok(Some(CoordinatorResponse::Error(e))) => Err(NodeError::RegistrationFailed(e)),
            Ok(resp) => Err(NodeError::RegistrationFailed(format!(
                "unexpected response: {:?}",
                resp
            ))),
            Err(e) => Err(NodeError::Transport(format!("{}", e))),
        }
    }

    pub async fn request(&self, req: CoordinatorRequest) -> NodeResult<CoordinatorResponse> {
        let mut guard = self.inner.lock().await;

        match guard.as_mut() {
            Some(transport) => {
                transport
                    .send_message(&req)
                    .await
                    .map_err(|e| NodeError::Transport(format!("{}", e)))?;

                match transport
                    .recv_message::<CoordinatorResponse>()
                    .await
                    .map_err(|e| NodeError::Transport(format!("{}", e)))?
                {
                    Some(resp) => Ok(resp),
                    None => {
                        *guard = None;
                        Err(NodeError::Connection(
                            "connection closed by server".to_string(),
                        ))
                    }
                }
            }
            None => Err(NodeError::NotRegistered),
        }
    }

    pub fn start_reconnect_loop(&self) {
        let conn = self.clone();
        tokio::spawn(async move {
            let mut backoff = Duration::from_secs(1);
            let max_backoff = Duration::from_secs(30);

            loop {
                {
                    let guard = conn.inner.lock().await;
                    if guard.is_some() {
                        // Still connected, check periodically
                        drop(guard);
                        sleep(Duration::from_secs(5)).await;
                        continue;
                    }
                }

                // Attempt to reconnect
                sleep(backoff).await;
                if let Ok(stream) = TcpStream::connect(conn.coordinator_addr).await {
                    let mut transport = FramedTransport::new(stream);
                    let req = CoordinatorRequest::RegisterNode {
                        worker_id: conn.worker_id,
                        hostname: conn.hostname.clone(),
                        capacity: conn.capacity.clone(),
                    };
                    if let Ok(()) = transport.send_message(&req).await {
                        if let Ok(Some(CoordinatorResponse::Ok)) =
                            transport.recv_message::<CoordinatorResponse>().await
                        {
                            let mut guard = conn.inner.lock().await;
                            *guard = Some(transport);
                            backoff = Duration::from_secs(1);
                            continue;
                        }
                    }
                }

                backoff = std::cmp::min(backoff * 2, max_backoff);
            }
        });
    }

    pub fn start_heartbeat_loop(&self, interval_dur: Duration) {
        let conn = self.clone();
        tokio::spawn(async move {
            let mut ticker = interval(interval_dur);
            loop {
                ticker.tick().await;
                let req = CoordinatorRequest::NodeHeartbeat {
                    worker_id: conn.worker_id,
                };
                let _ = conn.request(req).await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn connection_fails_on_invalid_addr() {
        let result = NodeConnection::connect(
            "127.0.0.1:1".parse().unwrap(),
            WorkerId::new(),
            "localhost".to_string(),
            ResourceAllocation::default(),
        )
        .await;
        assert!(result.is_err());
    }
}
