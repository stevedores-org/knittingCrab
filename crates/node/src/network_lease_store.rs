use crate::connection::NodeConnection;
use knitting_crab_core::error::CoreError;
use knitting_crab_core::ids::TaskId;
use knitting_crab_core::lease::Lease;
use knitting_crab_core::traits::LeaseStore;
use knitting_crab_transport::CoordinatorRequest;

#[derive(Clone)]
pub struct NetworkLeaseStore {
    conn: NodeConnection,
}

impl NetworkLeaseStore {
    pub fn new(conn: NodeConnection) -> Self {
        Self { conn }
    }
}

#[async_trait::async_trait]
impl LeaseStore for NetworkLeaseStore {
    async fn insert(&self, lease: Lease) -> Result<(), CoreError> {
        let req = CoordinatorRequest::InsertLease { lease };
        match self.conn.request(req).await {
            Ok(knitting_crab_transport::CoordinatorResponse::Ok) => Ok(()),
            Ok(knitting_crab_transport::CoordinatorResponse::Error(e)) => {
                if e.contains("already") {
                    Err(CoreError::AlreadyLeased)
                } else {
                    Err(CoreError::Internal(e))
                }
            }
            Ok(_) => Err(CoreError::Internal(
                "unexpected response to insert lease".to_string(),
            )),
            Err(e) => Err(CoreError::Internal(format!("network error: {}", e))),
        }
    }

    async fn get(&self, task_id: TaskId) -> Result<Option<Lease>, CoreError> {
        let req = CoordinatorRequest::GetLease { task_id };
        match self.conn.request(req).await {
            Ok(knitting_crab_transport::CoordinatorResponse::Lease(lease)) => Ok(lease),
            Ok(knitting_crab_transport::CoordinatorResponse::Error(e)) => {
                Err(CoreError::Internal(e))
            }
            Ok(_) => Err(CoreError::Internal(
                "unexpected response to get lease".to_string(),
            )),
            Err(e) => Err(CoreError::Internal(format!("network error: {}", e))),
        }
    }

    async fn update(&self, lease: Lease) -> Result<(), CoreError> {
        let req = CoordinatorRequest::UpdateLease { lease };
        match self.conn.request(req).await {
            Ok(knitting_crab_transport::CoordinatorResponse::Ok) => Ok(()),
            Ok(knitting_crab_transport::CoordinatorResponse::Error(e)) => {
                Err(CoreError::Internal(e))
            }
            Ok(_) => Err(CoreError::Internal(
                "unexpected response to update lease".to_string(),
            )),
            Err(e) => Err(CoreError::Internal(format!("network error: {}", e))),
        }
    }

    async fn remove(&self, task_id: TaskId) -> Result<(), CoreError> {
        let req = CoordinatorRequest::RemoveLease { task_id };
        match self.conn.request(req).await {
            Ok(knitting_crab_transport::CoordinatorResponse::Ok) => Ok(()),
            Ok(knitting_crab_transport::CoordinatorResponse::Error(e)) => {
                Err(CoreError::Internal(e))
            }
            Ok(_) => Err(CoreError::Internal(
                "unexpected response to remove lease".to_string(),
            )),
            Err(e) => Err(CoreError::Internal(format!("network error: {}", e))),
        }
    }

    async fn active_leases(&self) -> Result<Vec<Lease>, CoreError> {
        let req = CoordinatorRequest::ActiveLeases;
        match self.conn.request(req).await {
            Ok(knitting_crab_transport::CoordinatorResponse::Leases(leases)) => Ok(leases),
            Ok(knitting_crab_transport::CoordinatorResponse::Error(e)) => {
                Err(CoreError::Internal(e))
            }
            Ok(_) => Err(CoreError::Internal(
                "unexpected response to active leases".to_string(),
            )),
            Err(e) => Err(CoreError::Internal(format!("network error: {}", e))),
        }
    }
}
