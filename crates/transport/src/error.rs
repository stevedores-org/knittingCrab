use thiserror::Error;

#[derive(Error, Debug)]
pub enum TransportError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialize(#[from] serde_json::Error),

    #[error("Connection closed")]
    ConnectionClosed,

    #[error("Message too large: {0} bytes (max 16 MiB)")]
    MessageTooLarge(usize),

    #[error("Protocol error: {0}")]
    Protocol(String),
}

pub type TransportResult<T> = Result<T, TransportError>;
