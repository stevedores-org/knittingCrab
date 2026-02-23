use knitting_crab_core::event::TaskEvent;
use knitting_crab_core::ids::{TaskId, WorkerId};
use knitting_crab_core::lease::Lease;
use knitting_crab_core::resource::ResourceAllocation;
use knitting_crab_core::traits::TaskDescriptor;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", content = "payload")]
pub enum CoordinatorRequest {
    RegisterNode {
        worker_id: WorkerId,
        hostname: String,
        capacity: ResourceAllocation,
    },
    NodeHeartbeat {
        worker_id: WorkerId,
    },
    Dequeue {
        worker_id: WorkerId,
    },
    Requeue {
        task_id: TaskId,
        attempt: u32,
    },
    Discard {
        task_id: TaskId,
    },
    InsertLease {
        lease: Lease,
    },
    GetLease {
        task_id: TaskId,
    },
    UpdateLease {
        lease: Lease,
    },
    RemoveLease {
        task_id: TaskId,
    },
    ActiveLeases,
    EmitEvent {
        event: TaskEvent,
    },
    EmitLog {
        log: knitting_crab_core::event::LogLine,
    },
    QueryCache {
        cache_key: String,
    },
    AnnounceCache {
        cache_key: String,
        path: PathBuf,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type", content = "payload")]
#[allow(clippy::large_enum_variant)]
pub enum CoordinatorResponse {
    Ok,
    Dequeued(Option<TaskDescriptor>),
    Lease(Option<Lease>),
    Leases(Vec<Lease>),
    CacheLocations(Vec<CacheLocation>),
    Error(String),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CacheLocation {
    pub worker_id: WorkerId,
    pub hostname: String,
    pub path: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_register_node() {
        let req = CoordinatorRequest::RegisterNode {
            worker_id: WorkerId::new(),
            hostname: "localhost".to_string(),
            capacity: ResourceAllocation::default(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let deserialized: CoordinatorRequest = serde_json::from_str(&json).unwrap();
        assert!(matches!(
            deserialized,
            CoordinatorRequest::RegisterNode { .. }
        ));
    }

    #[test]
    fn round_trip_dequeue() {
        let req = CoordinatorRequest::Dequeue {
            worker_id: WorkerId::new(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let deserialized: CoordinatorRequest = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, CoordinatorRequest::Dequeue { .. }));
    }

    #[test]
    fn round_trip_response_ok() {
        let resp = CoordinatorResponse::Ok;
        let json = serde_json::to_string(&resp).unwrap();
        let deserialized: CoordinatorResponse = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, CoordinatorResponse::Ok));
    }

    #[test]
    fn round_trip_response_error() {
        let resp = CoordinatorResponse::Error("test error".to_string());
        let json = serde_json::to_string(&resp).unwrap();
        let deserialized: CoordinatorResponse = serde_json::from_str(&json).unwrap();
        assert!(matches!(deserialized, CoordinatorResponse::Error(_)));
    }

    #[test]
    fn round_trip_cache_location() {
        let loc = CacheLocation {
            worker_id: WorkerId::new(),
            hostname: "host1".to_string(),
            path: PathBuf::from("/cache/data"),
        };
        let json = serde_json::to_string(&loc).unwrap();
        let deserialized: CacheLocation = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.hostname, "host1");
    }
}
