use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct Lease {
    pub id: u64,
    pub task_id: u64,
    pub worker_id: u64,
    pub granted_at: Instant,
    pub expires_at: Instant,
    pub cpu_cores: u32,
    pub ram_mb: u64,
    pub gpu: bool,
}

impl Lease {
    pub fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at
    }
}

/// Manages task leases. This struct has no internal locking — the caller
/// (Scheduler) is responsible for synchronization via its own RwLock.
pub struct LeaseManager {
    leases: HashMap<u64, Lease>,
    next_id: u64,
}

impl LeaseManager {
    pub fn new() -> Self {
        LeaseManager {
            leases: HashMap::new(),
            next_id: 1,
        }
    }

    /// Grants a lease for a task. Returns an error if the task already has an active lease.
    pub fn grant_lease(
        &mut self,
        task_id: u64,
        worker_id: u64,
        cpu: u32,
        ram: u64,
        gpu: bool,
        duration: Duration,
    ) -> Result<Lease, String> {
        if self.has_active_lease(task_id) {
            return Err(format!("Task {} already has an active lease", task_id));
        }
        let id = self.next_id;
        self.next_id += 1;
        let now = Instant::now();
        let lease = Lease {
            id,
            task_id,
            worker_id,
            granted_at: now,
            expires_at: now + duration,
            cpu_cores: cpu,
            ram_mb: ram,
            gpu,
        };
        self.leases.insert(id, lease.clone());
        Ok(lease)
    }

    pub fn revoke_lease(&mut self, lease_id: u64) -> Option<Lease> {
        self.leases.remove(&lease_id)
    }

    pub fn expired_leases(&self) -> Vec<Lease> {
        self.leases
            .values()
            .filter(|l| l.is_expired())
            .cloned()
            .collect()
    }

    pub fn heartbeat(&mut self, lease_id: u64, extension: Duration) -> bool {
        if let Some(lease) = self.leases.get_mut(&lease_id) {
            lease.expires_at = Instant::now() + extension;
            true
        } else {
            false
        }
    }

    fn has_active_lease(&self, task_id: u64) -> bool {
        self.leases.values().any(|l| l.task_id == task_id)
    }
}

impl Default for LeaseManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_expires_requeues_task() {
        let mut mgr = LeaseManager::new();
        let lease = mgr
            .grant_lease(42, 1, 2, 512, false, Duration::from_millis(1))
            .unwrap();
        std::thread::sleep(Duration::from_millis(5));
        let expired = mgr.expired_leases();
        assert!(expired.iter().any(|l| l.id == lease.id));
    }

    #[test]
    fn heartbeat_extends_lease() {
        let mut mgr = LeaseManager::new();
        let lease = mgr
            .grant_lease(1, 1, 1, 256, false, Duration::from_millis(10))
            .unwrap();
        std::thread::sleep(Duration::from_millis(5));
        let extended = mgr.heartbeat(lease.id, Duration::from_secs(60));
        assert!(extended);
        assert!(mgr.expired_leases().is_empty());
    }

    #[test]
    fn revoke_removes_lease() {
        let mut mgr = LeaseManager::new();
        let lease = mgr
            .grant_lease(1, 1, 1, 256, false, Duration::from_secs(60))
            .unwrap();
        let revoked = mgr.revoke_lease(lease.id);
        assert!(revoked.is_some());
        assert!(mgr.revoke_lease(lease.id).is_none());
    }

    #[test]
    fn duplicate_lease_prevention() {
        let mut mgr = LeaseManager::new();
        mgr.grant_lease(42, 1, 1, 256, false, Duration::from_secs(60))
            .unwrap();
        let result = mgr.grant_lease(42, 2, 1, 256, false, Duration::from_secs(60));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("already has an active lease"));
    }
}
