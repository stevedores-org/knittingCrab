#[derive(Debug, Clone)]
pub struct ResourceModel {
    pub total_cpu_cores: u32,
    pub available_cpu_cores: u32,
    pub total_ram_mb: u64,
    pub available_ram_mb: u64,
    pub total_gpu_slots: u32,
    pub available_gpu_slots: u32,
    pub total_network_slots: u32,
    pub available_network_slots: u32,
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

    pub fn allocate(&mut self, cpu: u32, ram: u64, gpu: bool) -> Result<(), String> {
        if !self.can_allocate(cpu, ram, gpu) {
            return Err(format!(
                "Insufficient resources: need {} CPU, {} MB RAM, gpu={}; available {} CPU, {} MB RAM, {} GPU slots",
                cpu, ram, gpu,
                self.available_cpu_cores, self.available_ram_mb, self.available_gpu_slots
            ));
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
        assert_eq!(r.available_cpu_cores, 4);
        assert_eq!(r.available_ram_mb, 4096);
        assert_eq!(r.available_gpu_slots, 1);
    }
}
