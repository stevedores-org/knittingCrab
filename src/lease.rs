use std::collections::HashMap;
use std::sync::{Arc, RwLock};
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

pub struct LeaseManager {
    inner: Arc<RwLock<LeaseManagerInner>>,
}

struct LeaseManagerInner {
    leases: HashMap<u64, Lease>,
    next_id: u64,
}

impl LeaseManager {
    pub fn new() -> Self {
        LeaseManager {
            inner: Arc::new(RwLock::new(LeaseManagerInner {
                leases: HashMap::new(),
                next_id: 1,
            })),
        }
    }

    pub fn grant_lease(
        &self,
        task_id: u64,
        worker_id: u64,
        cpu: u32,
        ram: u64,
        gpu: bool,
        duration: Duration,
    ) -> Lease {
        let mut inner = self.inner.write().unwrap();
        let id = inner.next_id;
        inner.next_id += 1;
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
        inner.leases.insert(id, lease.clone());
        lease
    }

    pub fn revoke_lease(&self, lease_id: u64) -> Option<Lease> {
        let mut inner = self.inner.write().unwrap();
        inner.leases.remove(&lease_id)
    }

    pub fn expired_leases(&self) -> Vec<Lease> {
        let inner = self.inner.read().unwrap();
        inner.leases.values().filter(|l| l.is_expired()).cloned().collect()
    }

    pub fn heartbeat(&self, lease_id: u64, extension: Duration) -> bool {
        let mut inner = self.inner.write().unwrap();
        if let Some(lease) = inner.leases.get_mut(&lease_id) {
            lease.expires_at = Instant::now() + extension;
            true
        } else {
            false
        }
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
        let mgr = LeaseManager::new();
        let lease = mgr.grant_lease(42, 1, 2, 512, false, Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(5));
        let expired = mgr.expired_leases();
        assert!(expired.iter().any(|l| l.id == lease.id));
    }

    #[test]
    fn heartbeat_extends_lease() {
        let mgr = LeaseManager::new();
        let lease = mgr.grant_lease(1, 1, 1, 256, false, Duration::from_millis(10));
        std::thread::sleep(Duration::from_millis(5));
        let extended = mgr.heartbeat(lease.id, Duration::from_secs(60));
        assert!(extended);
        // Should no longer be expired after extension
        assert!(mgr.expired_leases().is_empty());
    }

    #[test]
    fn revoke_removes_lease() {
        let mgr = LeaseManager::new();
        let lease = mgr.grant_lease(1, 1, 1, 256, false, Duration::from_secs(60));
        let revoked = mgr.revoke_lease(lease.id);
        assert!(revoked.is_some());
        assert!(mgr.revoke_lease(lease.id).is_none());
    }
}
