//! Integration with AIVCS (AI Agent Version Control System) ledger.
//!
//! Provides an event sink that records task lifecycle events into a content-addressed
//! AIVCS run ledger, enabling auditability and sovereign feedback loops.

use crate::error::CoreError;
use crate::event::{LogLine, TaskEvent};
use crate::traits::EventSink;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Event structure expected by the AIVCS ledger API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AivcsLedgerEvent {
    pub run_id: String,
    pub event_type: String,
    pub payload: serde_json::Value,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Sink that forwards events to an AIVCS ledger service.
pub struct AivcsEventSink {
    #[allow(dead_code)]
    endpoint: String,
    #[allow(dead_code)]
    client: reqwest::Client,
}

impl AivcsEventSink {
    /// Create a new AIVCS sink pointing to the given JSON-RPC or REST endpoint.
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            client: reqwest::Client::new(),
        }
    }

    /// Internal helper to check if an event should be forwarded based on a run_id.
    #[allow(dead_code)]
    async fn record_if_context_exists(
        &self,
        run_id: &Option<String>,
        event: &TaskEvent,
    ) -> Result<(), CoreError> {
        if let Some(id) = run_id {
            let aivcs_event = AivcsLedgerEvent {
                run_id: id.clone(),
                event_type: match event {
                    TaskEvent::Started { .. } => "TASK_STARTED".to_string(),
                    TaskEvent::Completed { .. } => "TASK_COMPLETED".to_string(),
                    _ => "TASK_EVENT".to_string(),
                },
                payload: serde_json::to_value(event)
                    .map_err(|e| CoreError::Internal(e.to_string()))?,
                timestamp: chrono::Utc::now(),
            };

            // Fire and forget (or log error) for the prototype
            let _ = self
                .client
                .post(&self.endpoint)
                .json(&aivcs_event)
                .send()
                .await;
        }
        Ok(())
    }
}

#[async_trait]
impl EventSink for AivcsEventSink {
    async fn emit_event(&self, _event: TaskEvent) -> Result<(), CoreError> {
        // Implementation detail: we need the run_id context.
        // In a full implementation, the sink would maintain a mapping of TaskId -> aivcs_run_id
        // populated during the 'Acquired' event.
        Ok(())
    }

    async fn emit_log(&self, _log: LogLine) -> Result<(), CoreError> {
        // Logs are generally too high-volume for the ledger; skip for now.
        Ok(())
    }
}
