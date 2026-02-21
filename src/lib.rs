//! # knitting-crab
//!
//! A resource-aware scheduler for local autonomous agent workloads, optimised
//! for Apple Silicon.  It provides:
//!
//! - A typed priority queue with aging to prevent starvation.
//! - A DAG-based dependency planner backed by [`petgraph`].
//! - Fine-grained resource accounting (CPU, RAM, Metal GPU slots).
//! - A lease system with heartbeat monitoring for fault tolerance.
//! - A content-addressed artifact cache keyed by SHA-256.
//! - An async worker abstraction over Tokio subprocesses.

pub mod cache;
pub mod dag;
pub mod error;
pub mod lease;
pub mod policy;
pub mod resources;
pub mod scheduler;
pub mod types;
pub mod worker;

pub use cache::{ArtifactCache, CacheEntry, CacheKey};
pub use dag::Plan;
pub use error::{Result, SchedulerError};
pub use lease::{Lease, LeaseManager};
pub use policy::SchedulerPolicy;
pub use resources::ResourceModel;
pub use scheduler::{Scheduler, SchedulerStats};
pub use types::{AgentBudget, Priority, ResourceBudget, TaskStatus, WorkItem, WorkItemBuilder, WorkerEvent};
pub use worker::AgentWorker;
