use crate::error::SchedulerError;

#[derive(Debug, Clone)]
pub struct ResourceModel {
    total_cpu_cores: u32,
    available_cpu_cores: u32,
    total_ram_mb: u64,
    available_ram_mb: u64,
    total_gpu_slots: u32,
    available_gpu_slots: u32,
    #[allow(dead_code)]
    total_network_slots: u32,
    #[allow(dead_code)]
    available_network_slots: u32,
}

impl ResourceModel {
    pub fn new(total_cpu: u32, total_ram_mb: u64, gpu_slots: u32, network_slots: u32) -> Self {
        ResourceModel {
            total_cpu_cores: total_cpu,
            available_cpu_cores: total_cpu,
            total_ram_mb,
            available_ram_mb: total_ram_mb,
            total_gpu_slots: gpu_slots,
            available_gpu_slots: gpu_slots,
            total_network_slots: network_slots,
            available_network_slots: network_slots,
        }
    }

    pub fn can_allocate(&self, cpu: u32, ram: u64, gpu: bool) -> bool {
        self.available_cpu_cores >= cpu
            && self.available_ram_mb >= ram
            && (!gpu || self.available_gpu_slots >= 1)
    }

    pub fn allocate(&mut self, cpu: u32, ram: u64, gpu: bool) -> Result<(), SchedulerError> {
        if !self.can_allocate(cpu, ram, gpu) {
            return Err(SchedulerError::InsufficientResources {
                need_cpu: cpu,
                need_ram: ram,
                need_gpu: gpu,
                avail_cpu: self.available_cpu_cores,
                avail_ram: self.available_ram_mb,
                avail_gpu: self.available_gpu_slots,
            });
        }
        self.available_cpu_cores -= cpu;
        self.available_ram_mb -= ram;
        if gpu {
            self.available_gpu_slots -= 1;
        }
        Ok(())
    }

    pub fn release(&mut self, cpu: u32, ram: u64, gpu: bool) {
        self.available_cpu_cores = (self.available_cpu_cores + cpu).min(self.total_cpu_cores);
        self.available_ram_mb = (self.available_ram_mb + ram).min(self.total_ram_mb);
        if gpu {
            self.available_gpu_slots = (self.available_gpu_slots + 1).min(self.total_gpu_slots);
        }
    }

    pub fn utilization(&self) -> f64 {
        if self.total_cpu_cores == 0 {
            return 0.0;
        }
        let used = self.total_cpu_cores - self.available_cpu_cores;
        used as f64 / self.total_cpu_cores as f64
    }

    pub fn available_cpu(&self) -> u32 {
        self.available_cpu_cores
    }

    pub fn total_cpu(&self) -> u32 {
        self.total_cpu_cores
    }

    pub fn available_ram(&self) -> u64 {
        self.available_ram_mb
    }

    pub fn available_gpu(&self) -> u32 {
        self.available_gpu_slots
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn does_not_overcommit_cpu() {
        let mut r = ResourceModel::new(4, 8192, 1, 4);
        assert!(r.allocate(4, 1024, false).is_ok());
        assert!(r.allocate(1, 512, false).is_err());
    }

    #[test]
    fn ram_budget_is_enforced() {
        let mut r = ResourceModel::new(8, 1024, 0, 4);
        assert!(r.allocate(1, 1024, false).is_ok());
        assert!(r.allocate(1, 1, false).is_err());
    }

    #[test]
    fn exclusive_gpu_slot_enforced() {
        let mut r = ResourceModel::new(8, 8192, 1, 4);
        assert!(r.allocate(1, 512, true).is_ok());
        assert!(r.allocate(1, 512, true).is_err());
    }

    #[test]
    fn release_restores_resources() {
        let mut r = ResourceModel::new(4, 4096, 1, 4);
        r.allocate(2, 1024, true).unwrap();
        r.release(2, 1024, true);
        assert_eq!(r.available_cpu(), 4);
        assert_eq!(r.available_ram(), 4096);
        assert_eq!(r.available_gpu(), 1);
    }

    #[test]
    fn insufficient_resources_error_type() {
        let mut r = ResourceModel::new(2, 1024, 0, 4);
        let err = r.allocate(4, 512, false).unwrap_err();
        assert!(matches!(
            err,
            SchedulerError::InsufficientResources { need_cpu: 4, .. }
        ));
    }
}
