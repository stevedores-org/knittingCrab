use thiserror::Error;

#[derive(Error, Debug)]
pub enum NodeError {
    #[error("Connection error: {0}")]
    Connection(String),

    #[error("Transport error: {0}")]
    Transport(String),

    #[error("Core error: {0}")]
    Core(String),

    #[error("Registration failed: {0}")]
    RegistrationFailed(String),

    #[error("Not registered")]
    NotRegistered,

    #[error("Reconnection failed after {attempts} attempts")]
    ReconnectionFailed { attempts: u32 },
}

pub type NodeResult<T> = Result<T, NodeError>;
