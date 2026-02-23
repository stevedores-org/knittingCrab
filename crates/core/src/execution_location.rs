//! Execution location specifies where a task runs: locally or on a remote session.

use serde::{Deserialize, Serialize};

/// Where a task should be executed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ExecutionLocation {
    /// Execute locally on the worker machine.
    #[default]
    Local,
    /// Execute on a remote tmux session via SSH.
    RemoteSession(RemoteSessionTarget),
}

/// Target for remote session execution via SSH + tmux.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteSessionTarget {
    /// SSH host (alphanumeric, dots, hyphens, underscores only).
    pub host: String,
    /// SSH user (alphanumeric, dots, hyphens, underscores only).
    pub user: String,
    /// Repository name on remote (no .. or / for traversal protection).
    pub repo_name: String,
}

impl RemoteSessionTarget {
    /// Create a new RemoteSessionTarget with validation.
    pub fn new(host: String, user: String, repo_name: String) -> Result<Self, String> {
        // Validate host
        if host.is_empty() {
            return Err("host cannot be empty".to_string());
        }
        if !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
        {
            return Err("host contains invalid characters".to_string());
        }

        // Validate user
        if user.is_empty() {
            return Err("user cannot be empty".to_string());
        }
        if !user
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
        {
            return Err("user contains invalid characters".to_string());
        }

        // Validate repo_name
        if repo_name.is_empty() {
            return Err("repo_name cannot be empty".to_string());
        }
        if repo_name.contains("..") {
            return Err("repo_name contains path traversal (..)".to_string());
        }
        if repo_name.contains('/') {
            return Err("repo_name contains path separator".to_string());
        }

        Ok(Self {
            host,
            user,
            repo_name,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_to_local() {
        let location = ExecutionLocation::default();
        assert_eq!(location, ExecutionLocation::Local);
    }

    #[test]
    fn serde_round_trip_local() {
        let location = ExecutionLocation::Local;
        let json = serde_json::to_string(&location).unwrap();
        let deserialized: ExecutionLocation = serde_json::from_str(&json).unwrap();
        assert_eq!(location, deserialized);
    }

    #[test]
    fn serde_round_trip_remote() {
        let target = RemoteSessionTarget {
            host: "aivcs.local".to_string(),
            user: "aivcs".to_string(),
            repo_name: "my-repo".to_string(),
        };
        let location = ExecutionLocation::RemoteSession(target);
        let json = serde_json::to_string(&location).unwrap();
        let deserialized: ExecutionLocation = serde_json::from_str(&json).unwrap();
        assert_eq!(location, deserialized);
    }

    #[test]
    fn remote_target_validation_rejects_traversal() {
        let result = RemoteSessionTarget::new(
            "aivcs.local".to_string(),
            "aivcs".to_string(),
            "../etc".to_string(),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("traversal"));
    }

    #[test]
    fn remote_target_validation_rejects_slash() {
        let result = RemoteSessionTarget::new(
            "aivcs.local".to_string(),
            "aivcs".to_string(),
            "repo/name".to_string(),
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("separator"));
    }

    #[test]
    fn remote_target_validation_accepts_valid_names() {
        let result = RemoteSessionTarget::new(
            "aivcs.local".to_string(),
            "aivcs".to_string(),
            "my-repo_123".to_string(),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn remote_target_validation_rejects_invalid_host() {
        let result = RemoteSessionTarget::new(
            "aivcs@evil".to_string(),
            "aivcs".to_string(),
            "repo".to_string(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn remote_target_validation_rejects_invalid_user() {
        let result = RemoteSessionTarget::new(
            "aivcs.local".to_string(),
            "user;whoami".to_string(),
            "repo".to_string(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn remote_target_validation_rejects_empty_repo_name() {
        let result = RemoteSessionTarget::new(
            "aivcs.local".to_string(),
            "aivcs".to_string(),
            "".to_string(),
        );
        assert!(result.is_err());
    }
}
