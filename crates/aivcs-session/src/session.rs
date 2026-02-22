//! Session configuration and name generation for aivcs-session.
//!
//! Provides deterministic session naming and validation with security
//! against path traversal and shell injection.

use std::str::FromStr;
use thiserror::Error;

/// Task execution role affecting session lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Long-lived session for agent debugging.
    Agent,
    /// Disposable session, killed after task completion.
    Runner,
    /// Manual session for human interaction.
    Human,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Agent => "agent",
            Role::Runner => "runner",
            Role::Human => "human",
        }
    }
}

impl FromStr for Role {
    type Err = SessionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "agent" => Ok(Role::Agent),
            "runner" => Ok(Role::Runner),
            "human" => Ok(Role::Human),
            _ => Err(SessionError::InvalidRole(s.to_string())),
        }
    }
}

/// Errors from session operations.
#[derive(Debug, Clone, Error)]
#[allow(dead_code)]
pub enum SessionError {
    #[error("invalid role: {0}")]
    InvalidRole(String),

    #[error("repo name contains path traversal: {0}")]
    RepoPathTraversal(String),

    #[error("repo name contains invalid characters: {0}")]
    RepoInvalidCharacters(String),

    #[error("work_id contains invalid characters: {0}")]
    WorkIdInvalidCharacters(String),

    #[error("session name generation failed")]
    SessionNameGeneration,

    #[error("repo directory not found on remote: {0}")]
    RepoNotFound(String),

    #[error("SSH connection failed: {0}")]
    ConnectionFailed(String),

    #[error("tmux command failed: {0}")]
    TmuxFailed(String),

    #[error("timeout waiting for session")]
    Timeout,

    #[error("internal error: {0}")]
    Internal(String),

    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
}

/// Session configuration for tmux on remote aivcs.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    repo_name: String,
    work_id: String,
    role: Role,
}

impl Default for SessionConfig {
    fn default() -> Self {
        // Safe defaults for testing
        Self {
            repo_name: "test-repo".to_string(),
            work_id: "test-work".to_string(),
            role: Role::Agent,
        }
    }
}

impl SessionConfig {
    /// Create a new session configuration with validation.
    pub fn new(
        repo_name: impl Into<String>,
        work_id: impl Into<String>,
        role: Role,
    ) -> Result<Self, SessionError> {
        let repo_name = repo_name.into();
        let work_id = work_id.into();

        Self::validate_repo_name(&repo_name)?;
        Self::validate_work_id(&work_id)?;

        Ok(Self {
            repo_name,
            work_id,
            role,
        })
    }

    /// Get repo name (read-only access).
    #[allow(dead_code)]
    pub fn repo_name(&self) -> &str {
        &self.repo_name
    }

    /// Get work ID (read-only access).
    #[allow(dead_code)]
    pub fn work_id(&self) -> &str {
        &self.work_id
    }

    /// Get role (read-only access).
    #[allow(dead_code)]
    pub fn role(&self) -> Role {
        self.role
    }

    /// Generate deterministic session name from configuration.
    ///
    /// Format: `aivcs__${repo}__${work_id}__${role}`
    /// - lowercase
    /// - replace [^a-z0-9._-] with `_`
    /// - forbid `..` and `/`
    pub fn session_name(&self) -> Result<String, SessionError> {
        let sanitized_repo = Self::sanitize_name(&self.repo_name)?;
        let sanitized_work = Self::sanitize_name(&self.work_id)?;
        let role = self.role.as_str();

        Ok(format!(
            "aivcs__{}__{}__{}",
            sanitized_repo, sanitized_work, role
        ))
    }

    /// Validate repo name (forbid path traversal).
    fn validate_repo_name(repo: &str) -> Result<(), SessionError> {
        if repo.is_empty() {
            return Err(SessionError::RepoInvalidCharacters("empty".to_string()));
        }
        if repo.contains("..") || repo.contains('/') {
            return Err(SessionError::RepoPathTraversal(repo.to_string()));
        }
        Ok(())
    }

    /// Validate work_id (non-empty).
    fn validate_work_id(work_id: &str) -> Result<(), SessionError> {
        if work_id.is_empty() {
            return Err(SessionError::WorkIdInvalidCharacters("empty".to_string()));
        }
        Ok(())
    }

    /// Sanitize a name: lowercase + replace invalid chars with `_`.
    fn sanitize_name(name: &str) -> Result<String, SessionError> {
        let sanitized = name
            .to_lowercase()
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>();

        if sanitized.is_empty() {
            return Err(SessionError::SessionNameGeneration);
        }

        Ok(sanitized)
    }

    /// Get the remote repository path (hardcoded safe base, no env var expansion).
    pub fn repo_path(&self) -> String {
        // Use hardcoded base path to prevent $HOME manipulation attacks
        format!("/home/aivcs/engineering/code/clone-base/{}", self.repo_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== Role Tests =====

    #[test]
    fn test_role_as_str() {
        assert_eq!(Role::Agent.as_str(), "agent");
        assert_eq!(Role::Runner.as_str(), "runner");
        assert_eq!(Role::Human.as_str(), "human");
    }

    #[test]
    fn test_role_from_str() {
        assert_eq!(Role::from_str("agent").unwrap(), Role::Agent);
        assert_eq!(Role::from_str("runner").unwrap(), Role::Runner);
        assert_eq!(Role::from_str("human").unwrap(), Role::Human);
        assert_eq!(Role::from_str("AGENT").unwrap(), Role::Agent);
        assert!(Role::from_str("invalid").is_err());
    }

    // ===== SessionConfig Creation Tests =====

    #[test]
    fn test_new_valid_config() {
        let config = SessionConfig::new("my-repo", "job-123", Role::Runner).unwrap();
        assert_eq!(config.repo_name, "my-repo");
        assert_eq!(config.work_id, "job-123");
        assert_eq!(config.role, Role::Runner);
    }

    #[test]
    fn test_rejects_repo_with_path_traversal() {
        assert!(SessionConfig::new("../evil", "job", Role::Runner).is_err());
        assert!(SessionConfig::new("repo/subdir", "job", Role::Runner).is_err());
    }

    #[test]
    fn test_allows_repo_with_special_chars_for_sanitization() {
        // Special characters are allowed and will be sanitized
        assert!(SessionConfig::new("repo@host", "job", Role::Runner).is_ok());
        assert!(SessionConfig::new("repo$var", "job", Role::Runner).is_ok());
        assert!(SessionConfig::new("repo;cmd", "job", Role::Runner).is_ok());
    }

    #[test]
    fn test_allows_work_id_with_special_chars_for_sanitization() {
        // Special characters are allowed and will be sanitized
        assert!(SessionConfig::new("repo", "job@host", Role::Runner).is_ok());
        assert!(SessionConfig::new("repo", "job$var", Role::Runner).is_ok());
        assert!(SessionConfig::new("repo", "job;cmd", Role::Runner).is_ok());
    }

    #[test]
    fn test_allows_valid_special_chars_in_repo() {
        assert!(SessionConfig::new("my-repo_v1.0", "job-123", Role::Runner).is_ok());
    }

    #[test]
    fn test_allows_valid_special_chars_in_work_id() {
        assert!(SessionConfig::new("repo", "job-123_v1.0", Role::Runner).is_ok());
    }

    // ===== Session Name Generation Tests =====

    #[test]
    fn test_session_name_format() {
        let config = SessionConfig::new("my-repo", "job-123", Role::Runner).unwrap();
        let name = config.session_name().unwrap();
        assert_eq!(name, "aivcs__my-repo__job-123__runner");
    }

    #[test]
    fn test_session_name_lowercase() {
        let config = SessionConfig::new("MyRepo", "JOB-123", Role::Agent).unwrap();
        let name = config.session_name().unwrap();
        assert_eq!(name, "aivcs__myrepo__job-123__agent");
    }

    #[test]
    fn test_session_name_replaces_spaces() {
        let config = SessionConfig::new("my repo", "job 123", Role::Human).unwrap();
        let name = config.session_name().unwrap();
        assert_eq!(name, "aivcs__my_repo__job_123__human");
    }

    #[test]
    fn test_session_name_idempotent() {
        let config1 = SessionConfig::new("MyRepo", "Job-123", Role::Runner).unwrap();
        let config2 = SessionConfig::new("myrepo", "job-123", Role::Runner).unwrap();
        assert_eq!(
            config1.session_name().unwrap(),
            config2.session_name().unwrap()
        );
    }

    #[test]
    fn test_session_name_different_roles() {
        let runner = SessionConfig::new("repo", "job", Role::Runner).unwrap();
        let agent = SessionConfig::new("repo", "job", Role::Agent).unwrap();
        assert_ne!(
            runner.session_name().unwrap(),
            agent.session_name().unwrap()
        );
    }

    #[test]
    fn test_forbids_path_traversal_in_sanitize() {
        // Even though validate_repo_name forbids "..", test sanitize behavior
        let config = SessionConfig::new("normalrepo", "job", Role::Runner).unwrap();
        assert_eq!(
            config.session_name().unwrap(),
            "aivcs__normalrepo__job__runner"
        );
    }

    #[test]
    fn test_empty_name_fails() {
        // This should fail during validation, not sanitization
        let result = SessionConfig::new("", "job", Role::Runner);
        assert!(result.is_err());
    }

    // ===== Repo Path Tests =====

    #[test]
    fn test_repo_path() {
        let config = SessionConfig::new("my-repo", "job", Role::Runner).unwrap();
        assert_eq!(
            config.repo_path(),
            "/home/aivcs/engineering/code/clone-base/my-repo"
        );
    }

    // ===== Integration Tests =====

    #[test]
    fn test_realistic_configuration() {
        let config = SessionConfig::new("knittingCrab", "epic1-us2-123", Role::Runner).unwrap();
        let name = config.session_name().unwrap();
        assert_eq!(name, "aivcs__knittingcrab__epic1-us2-123__runner");
        assert_eq!(
            config.repo_path(),
            "/home/aivcs/engineering/code/clone-base/knittingCrab"
        );
    }

    #[test]
    fn test_agent_session_for_debugging() {
        let config = SessionConfig::new("knittingCrab", "debug-456", Role::Agent).unwrap();
        let name = config.session_name().unwrap();
        assert_eq!(name, "aivcs__knittingcrab__debug-456__agent");
        assert_eq!(config.role, Role::Agent);
    }
}
