use thiserror::Error;

#[derive(Error, Debug)]
pub enum WorkerError {
    #[error("core error: {0}")]
    Core(#[from] knitting_crab_core::CoreError),

    #[error("spawn failed: {0}")]
    SpawnFailed(String),

    #[error("process error: {0}")]
    ProcessError(String),

    #[error("io error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("signal error: {0}")]
    SignalError(String),

    #[error("timeout")]
    Timeout,

    #[error("internal error: {0}")]
    Internal(String),
}
