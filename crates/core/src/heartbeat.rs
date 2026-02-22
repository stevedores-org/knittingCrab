use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::ids::LeaseId;

/// A heartbeat record indicating that a lease is still active.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatRecord {
    pub lease_id: LeaseId,
    pub timestamp: DateTime<Utc>,
}

impl HeartbeatRecord {
    /// Create a new heartbeat record for a lease.
    pub fn new(lease_id: LeaseId) -> Self {
        Self {
            lease_id,
            timestamp: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heartbeat_creation() {
        let lease_id = crate::ids::LeaseId::new();
        let hb = HeartbeatRecord::new(lease_id);
        assert_eq!(hb.lease_id, lease_id);
    }
}
