use crate::connection::NodeConnection;
use knitting_crab_core::error::CoreError;
use knitting_crab_core::event::{LogLine, TaskEvent};
use knitting_crab_core::traits::EventSink;
use knitting_crab_transport::CoordinatorRequest;

#[derive(Clone)]
pub struct NetworkEventSink {
    conn: NodeConnection,
}

impl NetworkEventSink {
    pub fn new(conn: NodeConnection) -> Self {
        Self { conn }
    }
}

#[async_trait::async_trait]
impl EventSink for NetworkEventSink {
    async fn emit_event(&self, event: TaskEvent) -> Result<(), CoreError> {
        let req = CoordinatorRequest::EmitEvent { event };
        // Fire and forget: event loss is preferable to failing the task
        let _ = self.conn.request(req).await;
        Ok(())
    }

    async fn emit_log(&self, log: LogLine) -> Result<(), CoreError> {
        let req = CoordinatorRequest::EmitLog { log };
        // Fire and forget: log loss is preferable to failing the task
        let _ = self.conn.request(req).await;
        Ok(())
    }
}
