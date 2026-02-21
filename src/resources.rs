use crate::error::{Result, SchedulerError};
use crate::types::ResourceBudget;

/// Tracks total and in-use compute resources for the local machine.
///
/// All mutation must happen through [`allocate`](ResourceModel::allocate) and
/// [`release`](ResourceModel::release) to keep the accounting consistent.
#[derive(Debug, Clone)]
pub struct ResourceModel {
    total_cpu_cores: f32,
    total_ram_mb: u64,
    total_metal_slots: u32,
    used_cpu_cores: f32,
    used_ram_mb: u64,
    used_metal_slots: u32,
}

impl ResourceModel {
    /// Creates a new model with the given total capacities and zero usage.
    pub fn new(total_cpu_cores: f32, total_ram_mb: u64, total_metal_slots: u32) -> Self {
        Self {
            total_cpu_cores,
            total_ram_mb,
            total_metal_slots,
            used_cpu_cores: 0.0,
            used_ram_mb: 0,
            used_metal_slots: 0,
        }
    }

    /// Returns `true` if the system has enough free resources to satisfy
    /// `budget` without going over capacity.
    pub fn can_allocate(&self, budget: &ResourceBudget) -> bool {
        self.available_cpu() >= budget.cpu_cores
            && self.available_ram_mb() >= budget.ram_mb
            && self.available_metal_slots() >= budget.metal_slots
    }

    /// Reserves `budget` from the available pool.
    ///
    /// Returns [`SchedulerError::ResourceLimitExceeded`] when insufficient
    /// resources are available.
    pub fn allocate(&mut self, budget: &ResourceBudget) -> Result<()> {
        if !self.can_allocate(budget) {
            return Err(SchedulerError::ResourceLimitExceeded(format!(
                "requested cpu={} ram={}MB metal={} but available cpu={} ram={}MB metal={}",
                budget.cpu_cores,
                budget.ram_mb,
                budget.metal_slots,
                self.available_cpu(),
                self.available_ram_mb(),
                self.available_metal_slots(),
            )));
        }
        self.used_cpu_cores += budget.cpu_cores;
        self.used_ram_mb += budget.ram_mb;
        self.used_metal_slots += budget.metal_slots;
        Ok(())
    }

    /// Returns previously reserved resources back to the pool.
    ///
    /// Usage counters are clamped to zero to guard against accounting bugs.
    pub fn release(&mut self, budget: &ResourceBudget) {
        self.used_cpu_cores = (self.used_cpu_cores - budget.cpu_cores).max(0.0);
        self.used_ram_mb = self.used_ram_mb.saturating_sub(budget.ram_mb);
        self.used_metal_slots = self.used_metal_slots.saturating_sub(budget.metal_slots);
    }

    /// Returns the number of CPU cores currently free.
    pub fn available_cpu(&self) -> f32 {
        (self.total_cpu_cores - self.used_cpu_cores).max(0.0)
    }

    /// Returns the RAM (in MiB) currently free.
    pub fn available_ram_mb(&self) -> u64 {
        self.total_ram_mb.saturating_sub(self.used_ram_mb)
    }

    /// Returns the number of Metal GPU compute slots currently free.
    pub fn available_metal_slots(&self) -> u32 {
        self.total_metal_slots.saturating_sub(self.used_metal_slots)
    }

    /// Returns a rough overall utilisation percentage in the range `[0, 100]`.
    ///
    /// The three dimensions are averaged with equal weight.
    pub fn utilization_percent(&self) -> f32 {
        let cpu_pct = if self.total_cpu_cores > 0.0 {
            self.used_cpu_cores / self.total_cpu_cores * 100.0
        } else {
            0.0
        };
        let ram_pct = if self.total_ram_mb > 0 {
            self.used_ram_mb as f32 / self.total_ram_mb as f32 * 100.0
        } else {
            0.0
        };
        let metal_pct = if self.total_metal_slots > 0 {
            self.used_metal_slots as f32 / self.total_metal_slots as f32 * 100.0
        } else {
            0.0
        };
        (cpu_pct + ram_pct + metal_pct) / 3.0
    }
}
