//! Remote execution on aivcs.local via SSH and tmux.

use crate::session::{SessionConfig, SessionError};
use std::process::Command;
use tracing::{debug, info};

/// Remote target configuration for SSH/tmux.
#[derive(Debug, Clone)]
pub struct RemoteTarget {
    pub host: String,
    pub user: String,
}

impl Default for RemoteTarget {
    fn default() -> Self {
        Self {
            host: "aivcs.local".to_string(),
            user: "aivcs".to_string(),
        }
    }
}

impl RemoteTarget {
    /// SSH connection string.
    pub fn ssh_target(&self) -> String {
        format!("{}@{}", self.user, self.host)
    }

    /// tmux binary path on remote.
    pub fn tmux_binary(&self) -> &str {
        "/opt/homebrew/bin/tmux"
    }
}

/// Session manager for remote tmux sessions.
pub struct RemoteSessionManager {
    remote: RemoteTarget,
}

impl Default for RemoteSessionManager {
    fn default() -> Self {
        Self::new(RemoteTarget::default())
    }
}

impl RemoteSessionManager {
    pub fn new(remote: RemoteTarget) -> Self {
        Self { remote }
    }

    /// Attach or create a tmux session in the repo directory.
    ///
    /// This is the main entry point for session management.
    pub fn attach_or_create(&self, config: &SessionConfig) -> Result<String, SessionError> {
        let session_name = config.session_name()?;

        // Verify repo directory exists on remote
        self.check_repo_exists(config)?;

        // Create or attach session in repo directory
        self.tmux_new_session(config, &session_name)?;

        info!("Session {} attached/created", session_name);
        Ok(session_name)
    }

    /// Check if repository directory exists on remote.
    fn check_repo_exists(&self, config: &SessionConfig) -> Result<(), SessionError> {
        let repo_path = config.repo_path();
        let escaped_path = shell_escape::unix::escape(repo_path.as_str().into());

        let output = Command::new("ssh")
            .args(["-o", "ConnectTimeout=5"])
            .arg(self.remote.ssh_target())
            .arg(format!("test -d {} && echo ok", escaped_path))
            .output()
            .map_err(|e| SessionError::ConnectionFailed(e.to_string()))?;

        if !output.status.success() {
            return Err(SessionError::RepoNotFound(repo_path));
        }

        debug!("Repo directory verified: {}", repo_path);
        Ok(())
    }

    /// Create or attach tmux session in repo directory.
    fn tmux_new_session(
        &self,
        config: &SessionConfig,
        session_name: &str,
    ) -> Result<(), SessionError> {
        let repo_path = config.repo_path();
        let tmux = self.remote.tmux_binary();

        // Escape session name and path for shell
        let escaped_session = shell_escape::unix::escape(session_name.into());
        let escaped_path = shell_escape::unix::escape(repo_path.into());

        let command = format!(
            "{} new-session -A -s {} -c {}",
            tmux, escaped_session, escaped_path
        );

        let output = Command::new("ssh")
            .args(["-o", "ConnectTimeout=5"])
            .arg(self.remote.ssh_target())
            .arg(command)
            .output()
            .map_err(|e| SessionError::ConnectionFailed(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SessionError::TmuxFailed(stderr.to_string()));
        }

        debug!("tmux session {} created/attached", session_name);
        Ok(())
    }

    /// List all sessions on remote.
    pub fn list_sessions(&self) -> Result<Vec<String>, SessionError> {
        let tmux = self.remote.tmux_binary();
        let command = format!("{} ls -F '#{{session_name}}'", tmux);

        let output = Command::new("ssh")
            .args(["-o", "ConnectTimeout=5"])
            .arg(self.remote.ssh_target())
            .arg(command)
            .output()
            .map_err(|e| SessionError::ConnectionFailed(e.to_string()))?;

        if !output.status.success() {
            return Err(SessionError::TmuxFailed(
                "failed to list sessions".to_string(),
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let sessions = stdout.lines().map(|line| line.to_string()).collect();

        Ok(sessions)
    }

    /// Kill a specific session on remote.
    pub fn kill_session(&self, session_name: &str) -> Result<(), SessionError> {
        let tmux = self.remote.tmux_binary();
        let escaped_session = shell_escape::unix::escape(session_name.into());
        let command = format!("{} kill-session -t {}", tmux, escaped_session);

        let output = Command::new("ssh")
            .args(["-o", "ConnectTimeout=5"])
            .arg(self.remote.ssh_target())
            .arg(command)
            .output()
            .map_err(|e| SessionError::ConnectionFailed(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SessionError::TmuxFailed(stderr.to_string()));
        }

        info!("Session {} killed", session_name);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remote_target_default() {
        let target = RemoteTarget::default();
        assert_eq!(target.host, "aivcs.local");
        assert_eq!(target.user, "aivcs");
    }

    #[test]
    fn test_remote_target_ssh_target() {
        let target = RemoteTarget::default();
        assert_eq!(target.ssh_target(), "aivcs@aivcs.local");
    }

    #[test]
    fn test_remote_target_tmux_binary() {
        let target = RemoteTarget::default();
        assert_eq!(target.tmux_binary(), "/opt/homebrew/bin/tmux");
    }

    #[test]
    fn test_session_manager_creation() {
        let manager = RemoteSessionManager::default();
        assert_eq!(manager.remote.ssh_target(), "aivcs@aivcs.local");
    }

    // Integration tests would require actual remote connection
    // These are placeholder unit tests that verify structure
}
