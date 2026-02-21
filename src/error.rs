use thiserror::Error;

#[derive(Debug, Error)]
pub enum SchedulerError {
    #[error("cycle detected: adding task {task_id} would create a dependency cycle")]
    CycleDetected { task_id: u64 },

    #[error("insufficient resources: need {need_cpu} CPU, {need_ram} MB RAM, gpu={need_gpu}; available {avail_cpu} CPU, {avail_ram} MB RAM, {avail_gpu} GPU slots")]
    InsufficientResources {
        need_cpu: u32,
        need_ram: u64,
        need_gpu: bool,
        avail_cpu: u32,
        avail_ram: u64,
        avail_gpu: u32,
    },

    #[error("lease conflict: task {task_id} already has an active lease")]
    LeaseConflict { task_id: u64 },

    #[error("task not found: {task_id}")]
    TaskNotFound { task_id: u64 },

    #[error("budget exhausted: {reason}")]
    BudgetExhausted { reason: String },

    #[error("invalid input: {reason}")]
    InvalidInput { reason: String },
}
