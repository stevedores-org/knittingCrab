pub mod backpressure_integration;
pub mod error;
pub mod event;
pub mod heartbeat;
pub mod ids;
pub mod lease;
pub mod queue_backpressure;
pub mod resource;
pub mod retry;
pub mod traits;

pub use backpressure_integration::{TaskExecutionFilter, TaskRejectionReason};
pub use error::CoreError;
pub use event::{LogLine, LogSource, TaskEvent};
pub use heartbeat::HeartbeatRecord;
pub use ids::{LeaseId, TaskId, WorkerId};
pub use lease::{Lease, LeaseState};
pub use queue_backpressure::{
    BackpressureConfig, DegradationMode, QueueBackpressureManager, QueueStats,
};
pub use resource::ResourceAllocation;
pub use retry::{ExitOutcome, RetryDecision, RetryPolicy};
pub use traits::{EventSink, LeaseStore, Queue, ResourceMonitor, TaskDescriptor};
