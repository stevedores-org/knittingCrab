use crate::connection::NodeConnection;
use knitting_crab_core::error::CoreError;
use knitting_crab_core::ids::{TaskId, WorkerId};
use knitting_crab_core::traits::{Queue, TaskDescriptor};
use knitting_crab_transport::CoordinatorRequest;

#[derive(Clone)]
pub struct NetworkQueue {
    conn: NodeConnection,
}

impl NetworkQueue {
    pub fn new(conn: NodeConnection) -> Self {
        Self { conn }
    }
}

#[async_trait::async_trait]
impl Queue for NetworkQueue {
    async fn dequeue(&self, worker_id: WorkerId) -> Result<Option<TaskDescriptor>, CoreError> {
        let req = CoordinatorRequest::Dequeue { worker_id };
        match self.conn.request(req).await {
            Ok(knitting_crab_transport::CoordinatorResponse::Dequeued(task)) => Ok(*task),
            Ok(knitting_crab_transport::CoordinatorResponse::Error(e)) => {
                Err(CoreError::Internal(e))
            }
            Ok(_) => Err(CoreError::Internal(
                "unexpected response to dequeue".to_string(),
            )),
            Err(e) => Err(CoreError::Internal(format!("network error: {}", e))),
        }
    }

    async fn requeue(&self, task_id: TaskId, attempt: u32) -> Result<(), CoreError> {
        let req = CoordinatorRequest::Requeue { task_id, attempt };
        match self.conn.request(req).await {
            Ok(knitting_crab_transport::CoordinatorResponse::Ok) => Ok(()),
            Ok(knitting_crab_transport::CoordinatorResponse::Error(e)) => {
                Err(CoreError::Internal(e))
            }
            Ok(_) => Err(CoreError::Internal(
                "unexpected response to requeue".to_string(),
            )),
            Err(e) => Err(CoreError::Internal(format!("network error: {}", e))),
        }
    }

    async fn discard(&self, task_id: TaskId) -> Result<(), CoreError> {
        let req = CoordinatorRequest::Discard { task_id };
        match self.conn.request(req).await {
            Ok(knitting_crab_transport::CoordinatorResponse::Ok) => Ok(()),
            Ok(knitting_crab_transport::CoordinatorResponse::Error(e)) => {
                Err(CoreError::Internal(e))
            }
            Ok(_) => Err(CoreError::Internal(
                "unexpected response to discard".to_string(),
            )),
            Err(e) => Err(CoreError::Internal(format!("network error: {}", e))),
        }
    }
}
