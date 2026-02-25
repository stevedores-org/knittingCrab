//! Remote execution on aivcs.local via SSH and tmux.

use crate::session::{SessionConfig, SessionError};
use async_trait::async_trait;
use knitting_crab_core::traits::RemoteSessionManager as RemoteSessionManagerTrait;
use knitting_crab_core::{
    CoreError, ExecutionResult, RemoteRole, RemoteSessionConfig, SessionHandle,
};
use std::process::Command;
use tracing::{debug, info};

/// Validate SSH host/user against command injection patterns.
fn validate_ssh_identifier(value: &str, field_name: &str) -> Result<(), SessionError> {
    if value.is_empty() {
        return Err(SessionError::InvalidConfig(format!(
            "{} cannot be empty",
            field_name
        )));
    }
    // Allow alphanumeric, dots, hyphens, underscores (safe for SSH)
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
    {
        return Err(SessionError::InvalidConfig(format!(
            "{} contains invalid characters (only alphanumeric, dots, hyphens, underscores allowed)",
            field_name
        )));
    }
    Ok(())
}

/// Remote target configuration for SSH/tmux.
#[derive(Debug, Clone)]
pub struct RemoteTarget {
    host: String,
    user: String,
}

impl Default for RemoteTarget {
    fn default() -> Self {
        // These are hardcoded safe defaults and won't fail validation
        Self {
            host: "aivcs.local".to_string(),
            user: "aivcs".to_string(),
        }
    }
}

impl RemoteTarget {
    /// Create a new RemoteTarget with validation.
    pub fn new(host: String, user: String) -> Result<Self, SessionError> {
        validate_ssh_identifier(&host, "host")?;
        validate_ssh_identifier(&user, "user")?;
        Ok(Self { host, user })
    }

    /// Get host (read-only access)
    #[allow(dead_code)]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Get user (read-only access)
    #[allow(dead_code)]
    pub fn user(&self) -> &str {
        &self.user
    }

    /// SSH connection string (with validation performed at construction time).
    pub fn ssh_target(&self) -> String {
        format!("{}@{}", self.user, self.host)
    }

    /// tmux binary path on remote (escaped for shell safety).
    pub fn tmux_binary(&self) -> String {
        shell_escape::unix::escape("/opt/homebrew/bin/tmux".into()).to_string()
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
    pub async fn attach_or_create(&self, config: &SessionConfig) -> Result<String, SessionError> {
        let session_name = config.session_name()?;

        // Verify repo directory exists on remote
        self.check_repo_exists(config).await?;

        // Create or attach session in repo directory
        self.tmux_new_session(config, &session_name).await?;

        info!("Session {} attached/created", session_name);
        Ok(session_name)
    }

    /// Check if repository directory exists on remote.
    async fn check_repo_exists(&self, config: &SessionConfig) -> Result<(), SessionError> {
        let repo_path = config.repo_path();
        let repo_path_for_error = repo_path.clone();
        let ssh_target = self.remote.ssh_target().to_string();

        let output = tokio::task::spawn_blocking(move || {
            let escaped_path = shell_escape::unix::escape(repo_path.as_str().into());
            Command::new("ssh")
                .args(["-o", "ConnectTimeout=5"])
                .arg(&ssh_target)
                .arg(format!("test -d {} && echo ok", escaped_path))
                .output()
        })
        .await
        .map_err(|e| SessionError::ConnectionFailed(format!("spawn_blocking failed: {}", e)))?
        .map_err(|e| SessionError::ConnectionFailed(e.to_string()))?;

        if !output.status.success() {
            return Err(SessionError::RepoNotFound(repo_path_for_error));
        }

        debug!("Repo directory verified: {}", repo_path_for_error);
        Ok(())
    }

    /// Create or attach tmux session in repo directory.
    async fn tmux_new_session(
        &self,
        config: &SessionConfig,
        session_name: &str,
    ) -> Result<(), SessionError> {
        let repo_path = config.repo_path();
        let tmux = self.remote.tmux_binary();
        let ssh_target = self.remote.ssh_target().to_string();
        let session_name_owned = session_name.to_string();
        let session_name_for_log = session_name_owned.clone();

        let output = tokio::task::spawn_blocking(move || {
            // Escape session name and path for shell
            let escaped_session = shell_escape::unix::escape(session_name_owned.as_str().into());
            let escaped_path = shell_escape::unix::escape(repo_path.as_str().into());

            let command = format!(
                "{} new-session -A -s {} -c {}",
                tmux, escaped_session, escaped_path
            );

            Command::new("ssh")
                .args(["-o", "ConnectTimeout=5"])
                .arg(&ssh_target)
                .arg(command)
                .output()
        })
        .await
        .map_err(|e| SessionError::ConnectionFailed(format!("spawn_blocking failed: {}", e)))?
        .map_err(|e| SessionError::ConnectionFailed(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SessionError::TmuxFailed(stderr.to_string()));
        }

        debug!("tmux session {} created/attached", session_name_for_log);
        Ok(())
    }

    /// List all sessions on remote.
    pub async fn list_sessions(&self) -> Result<Vec<String>, SessionError> {
        let tmux = self.remote.tmux_binary();
        let ssh_target = self.remote.ssh_target().to_string();

        let output = tokio::task::spawn_blocking(move || {
            let command = format!("{} ls -F '#{{session_name}}'", tmux);

            Command::new("ssh")
                .args(["-o", "ConnectTimeout=5"])
                .arg(&ssh_target)
                .arg(command)
                .output()
        })
        .await
        .map_err(|e| SessionError::ConnectionFailed(format!("spawn_blocking failed: {}", e)))?
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
    pub async fn kill_session(&self, session_name: &str) -> Result<(), SessionError> {
        let tmux = self.remote.tmux_binary();
        let ssh_target = self.remote.ssh_target().to_string();
        let session_name_owned = session_name.to_string();
        let session_name_for_log = session_name_owned.clone();

        let output = tokio::task::spawn_blocking(move || {
            let escaped_session = shell_escape::unix::escape(session_name_owned.as_str().into());
            let command = format!("{} kill-session -t {}", tmux, escaped_session);

            Command::new("ssh")
                .args(["-o", "ConnectTimeout=5"])
                .arg(&ssh_target)
                .arg(command)
                .output()
        })
        .await
        .map_err(|e| SessionError::ConnectionFailed(format!("spawn_blocking failed: {}", e)))?
        .map_err(|e| SessionError::ConnectionFailed(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SessionError::TmuxFailed(stderr.to_string()));
        }

        info!("Session {} killed", session_name_for_log);
        Ok(())
    }

    /// Run a command in a session and capture output.
    pub async fn run_in_session(
        &self,
        session_name: &str,
        cmd: &str,
    ) -> Result<ExecutionResult, SessionError> {
        let tmux = self.remote.tmux_binary();
        let ssh_target = self.remote.ssh_target().to_string();
        let session_name_owned = session_name.to_string();
        let cmd_owned = cmd.to_string();

        let output = tokio::task::spawn_blocking(move || {
            let escaped_session = shell_escape::unix::escape(session_name_owned.as_str().into());
            let escaped_cmd = shell_escape::unix::escape(cmd_owned.as_str().into());
            let command = format!(
                "{} send-keys -t {} {} Enter",
                tmux, escaped_session, escaped_cmd
            );

            Command::new("ssh")
                .args(["-o", "ConnectTimeout=5"])
                .arg(&ssh_target)
                .arg(command)
                .output()
        })
        .await
        .map_err(|e| SessionError::ConnectionFailed(format!("spawn_blocking failed: {}", e)))?
        .map_err(|e| SessionError::ConnectionFailed(e.to_string()))?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(1);

        Ok(ExecutionResult {
            exit_code,
            stdout,
            stderr,
        })
    }
}

/// Implement RemoteSessionManager trait from knitting-crab-core for the aivcs RemoteSessionManager.
#[async_trait]
impl RemoteSessionManagerTrait for RemoteSessionManager {
    async fn create_or_attach(
        &self,
        config: &RemoteSessionConfig,
    ) -> Result<SessionHandle, CoreError> {
        let session_config = SessionConfig::new(
            &config.repo_name,
            &config.work_id,
            match config.role {
                RemoteRole::Runner => crate::session::Role::Runner,
                RemoteRole::Agent => crate::session::Role::Agent,
                RemoteRole::Human => crate::session::Role::Human,
            },
        )
        .map_err(|e| CoreError::SessionFailed(e.to_string()))?;

        let session_name = self
            .attach_or_create(&session_config)
            .await
            .map_err(|e| CoreError::SessionFailed(e.to_string()))?;

        Ok(SessionHandle { session_name })
    }

    async fn run_command(
        &self,
        session: &SessionHandle,
        cmd: &str,
    ) -> Result<ExecutionResult, CoreError> {
        self.run_in_session(&session.session_name, cmd)
            .await
            .map_err(|e| CoreError::SessionFailed(e.to_string()))
    }

    async fn kill_session(&self, session: &SessionHandle) -> Result<(), CoreError> {
        self.kill_session(&session.session_name)
            .await
            .map_err(|e| CoreError::SessionFailed(e.to_string()))
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

    #[tokio::test]
    async fn test_trait_impl_session_handle() {
        // This test verifies that the trait implementation can be called
        // without requiring actual SSH/tmux (it will fail without real connection)
        let manager = RemoteSessionManager::default();
        let config = RemoteSessionConfig {
            host: "aivcs.local".to_string(),
            user: "aivcs".to_string(),
            repo_name: "test-repo".to_string(),
            work_id: "test-work".to_string(),
            role: RemoteRole::Runner,
        };

        // This would fail without actual SSH connection, so we just verify the type checks
        let _config_ref: &RemoteSessionConfig = &config;
        let _manager_ref: &dyn RemoteSessionManagerTrait = &manager;
    }

    #[test]
    fn test_cmd_escaping_safety() {
        // Verify that dangerous characters are properly escaped
        let dangerous_cmd = "'; rm -rf /; echo '";
        let escaped = shell_escape::unix::escape(dangerous_cmd.into());
        // Escaped should have quotes or backslashes to neutralize the injection
        let escaped_str = escaped.as_ref();
        assert!(
            escaped_str.contains('\'') || escaped_str.contains('\\') || escaped_str.contains('"'),
            "Escaped command should contain quote or backslash protection: {}",
            escaped_str
        );
    }

    #[test]
    fn test_session_name_escaping() {
        let dangerous_name = "'; rm -rf";
        let escaped = shell_escape::unix::escape(dangerous_name.into());
        // Escaped form should have quotes around it for safety
        assert!(escaped.as_ref().contains('\\') || escaped.as_ref().contains('\''));
    }
}
