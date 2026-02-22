//! Heartbeat monitoring for worker health and lease management.
//!
//! Tracks worker heartbeats to detect crashes and hangs.
//! Automatically requeues work when leases expire.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Tracks heartbeat status of a worker on a specific task.
#[derive(Debug, Clone)]
pub struct HeartbeatEntry {
    pub task_id: u64,
    pub worker_id: u64,
    pub last_heartbeat: Instant,
    pub expected_interval: Duration,
}

impl HeartbeatEntry {
    /// Check if the heartbeat has timed out.
    pub fn is_timed_out(&self) -> bool {
        self.last_heartbeat.elapsed() > self.expected_interval
    }

    /// Get how long overdue the heartbeat is.
    pub fn overdue_duration(&self) -> Duration {
        let elapsed = self.last_heartbeat.elapsed();
        if elapsed > self.expected_interval {
            elapsed - self.expected_interval
        } else {
            Duration::ZERO
        }
    }
}

/// Monitors worker heartbeats to detect crashes and hangs.
pub struct HeartbeatMonitor {
    entries: HashMap<u64, HeartbeatEntry>, // lease_id -> HeartbeatEntry
    lease_id_to_task: HashMap<u64, u64>,   // lease_id -> task_id
}

impl HeartbeatMonitor {
    pub fn new() -> Self {
        HeartbeatMonitor {
            entries: HashMap::new(),
            lease_id_to_task: HashMap::new(),
        }
    }

    /// Register a new lease for heartbeat monitoring.
    pub fn register(
        &mut self,
        lease_id: u64,
        task_id: u64,
        worker_id: u64,
        expected_interval: Duration,
    ) {
        let entry = HeartbeatEntry {
            task_id,
            worker_id,
            last_heartbeat: Instant::now(),
            expected_interval,
        };
        self.entries.insert(lease_id, entry);
        self.lease_id_to_task.insert(lease_id, task_id);
    }

    /// Record a heartbeat from a worker.
    pub fn ping(&mut self, lease_id: u64) -> bool {
        if let Some(entry) = self.entries.get_mut(&lease_id) {
            entry.last_heartbeat = Instant::now();
            true
        } else {
            false
        }
    }

    /// Get all timed-out leases (worker likely crashed or hung).
    pub fn get_timed_out(&self) -> Vec<u64> {
        self.entries
            .iter()
            .filter(|(_, entry)| entry.is_timed_out())
            .map(|(lease_id, _)| *lease_id)
            .collect()
    }

    /// Remove monitoring for a lease (task completed or canceled).
    pub fn unregister(&mut self, lease_id: u64) {
        self.entries.remove(&lease_id);
        self.lease_id_to_task.remove(&lease_id);
    }

    /// Get task ID for a lease.
    pub fn get_task_id(&self, lease_id: u64) -> Option<u64> {
        self.lease_id_to_task.get(&lease_id).copied()
    }
}

impl Default for HeartbeatMonitor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_entry_tracks_timeout() {
        let entry = HeartbeatEntry {
            task_id: 1,
            worker_id: 1,
            last_heartbeat: Instant::now() - Duration::from_secs(5),
            expected_interval: Duration::from_secs(2),
        };
        assert!(entry.is_timed_out());
    }

    #[test]
    fn heartbeat_entry_no_timeout_when_fresh() {
        let entry = HeartbeatEntry {
            task_id: 1,
            worker_id: 1,
            last_heartbeat: Instant::now(),
            expected_interval: Duration::from_secs(5),
        };
        assert!(!entry.is_timed_out());
    }

    #[test]
    fn monitor_registers_and_pings() {
        let mut monitor = HeartbeatMonitor::new();
        monitor.register(1, 100, 1, Duration::from_secs(5));

        // Should ping successfully
        assert!(monitor.ping(1));
        // Should not find timed out lease
        assert!(monitor.get_timed_out().is_empty());
    }

    #[test]
    fn monitor_detects_timeout() {
        let mut monitor = HeartbeatMonitor::new();
        let lease_id = 1;
        let task_id = 100;

        // Register with past timestamp to simulate timeout
        monitor.entries.insert(
            lease_id,
            HeartbeatEntry {
                task_id,
                worker_id: 1,
                last_heartbeat: Instant::now() - Duration::from_secs(10),
                expected_interval: Duration::from_secs(2),
            },
        );
        monitor.lease_id_to_task.insert(lease_id, task_id);

        let timed_out = monitor.get_timed_out();
        assert_eq!(timed_out.len(), 1);
        assert_eq!(timed_out[0], lease_id);
    }

    #[test]
    fn monitor_unregisters() {
        let mut monitor = HeartbeatMonitor::new();
        monitor.register(1, 100, 1, Duration::from_secs(5));
        assert_eq!(monitor.get_task_id(1), Some(100));

        monitor.unregister(1);
        assert_eq!(monitor.get_task_id(1), None);
    }

    #[test]
    fn monitor_ping_refreshes_heartbeat() {
        let mut monitor = HeartbeatMonitor::new();
        monitor.register(1, 100, 1, Duration::from_secs(2));

        // Manually set old timestamp
        if let Some(entry) = monitor.entries.get_mut(&1) {
            entry.last_heartbeat = Instant::now() - Duration::from_secs(1);
        }

        // Before ping, not timed out
        assert!(!monitor.get_timed_out().contains(&1));

        // After ping, still not timed out (heartbeat refreshed)
        assert!(monitor.ping(1));
        assert!(!monitor.get_timed_out().contains(&1));
    }
}
