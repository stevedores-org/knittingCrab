//! aivcs-session: Deterministic tmux session manager for aivcs.local
//!
//! Provides session management primitives for spawning tasks on remote workers
//! with security against path traversal and shell injection.

pub mod remote;
pub mod session;

pub use remote::{RemoteSessionManager, RemoteTarget};
pub use session::{Role, SessionConfig, SessionError};
