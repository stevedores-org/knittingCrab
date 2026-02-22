pub mod backpressure_integration;
pub mod circuit_breaker;
pub mod error;
pub mod event;
pub mod event_log;
pub mod heartbeat;
pub mod ids;
pub mod lease;
pub mod persistent_lease;
pub mod queue_backpressure;
pub mod resource;
pub mod retry;
pub mod task_timeout;
pub mod traits;

pub use backpressure_integration::{TaskExecutionFilter, TaskRejectionReason};
pub use circuit_breaker::{
    CircuitBreaker, CircuitBreakerConfig, CircuitBreakerStats, CircuitState,
};
pub use error::CoreError;
pub use event::{LogLine, LogSource, TaskEvent};
pub use event_log::{MemoryEventLog, MultiEventSink, SqliteEventLog};
pub use heartbeat::HeartbeatRecord;
pub use ids::{LeaseId, TaskId, WorkerId};
pub use lease::{Lease, LeaseState};
pub use persistent_lease::SqliteLeaseStore;
pub use queue_backpressure::{
    BackpressureConfig, DegradationMode, QueueBackpressureManager, QueueStats,
};
pub use resource::ResourceAllocation;
pub use retry::{ExitOutcome, RetryDecision, RetryPolicy};
pub use task_timeout::{TaskTimeoutManager, TimeoutHandle, TimeoutPolicy, TimeoutStatus};
pub use traits::{EventSink, LeaseStore, Queue, ResourceMonitor, TaskDescriptor};
