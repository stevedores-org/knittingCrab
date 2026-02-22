# aivcs-session CLI Tool Specification

**Purpose**: Remote session manager for `aivcs.local` (Apple Silicon studio) providing deterministic, idempotent tmux session lifecycle.

**Status**: Design phase (not yet implemented)

**Estimated Effort**: 2–3 days (validation, sanitization, SSH/tmux orchestration)

---

## User-Facing Interface

### Command: `aivcs-session attach`

Create or attach to a tmux session on `aivcs.local`.

```bash
aivcs-session attach \
  --repo knittingCrab \
  --work task-12345 \
  --role runner
```

**Outputs**:
- Success: Exits 0, attached to interactive tmux session
- Failure: Exits non-zero with error message

**Error Cases**:
```
aivcs-session attach --repo "bad/name" --work "task-1" --role runner
❌ Error: Invalid repo name "bad/name" (forbid /)

aivcs-session attach --repo "knittingCrab" --work "task-1" --role invalid_role
❌ Error: Invalid role "invalid_role" (must be: agent, runner, human)

aivcs-session attach --repo "missing-repo" --work "task-1" --role runner
❌ Error: Repository directory not found on aivcs.local
   (checked: ~/engineering/code/clone-base/missing-repo)
```

### Command: `aivcs-session kill`

Terminate a specific session.

```bash
aivcs-session kill --session aivcs__knittingcrab__task-12345__runner
```

**Outputs**:
- Success: Exits 0
- Failure: Exits non-zero with error

### Command: `aivcs-session list`

List all active aivcs sessions on `aivcs.local`.

```bash
$ aivcs-session list
aivcs__knittingcrab__task-12345__runner   (created 2m ago)
aivcs__knittingcrab__task-12346__runner   (created 30s ago)
aivcs__dotfiles__task-1__agent            (created 3h ago)
```

### Command: `aivcs-session prune`

Remove sessions older than N hours.

```bash
aivcs-session prune --older-than 2 --role runner
```

Prunes all `runner` role sessions created > 2 hours ago.

---

## Input Validation & Sanitization

### Repository Name (`--repo`)

**Rules**:
1. Lowercase
2. Only `[a-z0-9._-]` (alphanumeric, dot, underscore, hyphen)
3. Forbid `..` (path traversal)
4. Forbid `/` (path separator)
5. Max length 100 chars
6. Min length 1 char

**Examples**:
```
✅ knittingCrab          → knittingcrab
✅ my-repo_v2.1          → my-repo_v2.1
❌ ../../../secret       → Error: forbidden (..)
❌ repo/subdir           → Error: forbidden (/)
❌ My-Repo               → myre-repo (sanitized to lowercase)
❌ ""                    → Error: empty repo name
```

**Implementation**:
```rust
fn validate_and_sanitize_repo(s: &str) -> Result<String, String> {
    if s.is_empty() {
        return Err("repo name cannot be empty".to_string());
    }

    if s.len() > 100 {
        return Err("repo name too long (max 100)".to_string());
    }

    let lower = s.to_lowercase();

    if lower.contains("..") {
        return Err("repo name forbids '..'".to_string());
    }

    if lower.contains('/') {
        return Err("repo name forbids '/'".to_string());
    }

    let sanitized: String = lower
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();

    Ok(sanitized)
}
```

### Work ID (`--work`)

**Rules**:
1. Lowercase
2. Only `[a-z0-9._-]` + hyphen (for task IDs like `task-12345`)
3. Max length 100 chars
4. Min length 1 char

**Examples**:
```
✅ task-12345       → task-12345
✅ Job_ABC-001      → job_abc-001
❌ ../inject        → Error: forbidden
```

### Role (`--role`)

**Rules**:
1. Must be one of: `agent`, `runner`, `human`
2. Case-insensitive

**Examples**:
```
✅ runner           → runner
✅ AGENT            → agent
❌ invalid_role     → Error: must be agent/runner/human
```

---

## Session Naming

**Format**:
```
aivcs__${REPO}__${WORK}__${ROLE}
```

**Example**:
- Inputs: `repo=knittingCrab`, `work=task-12345`, `role=runner`
- Session name: `aivcs__knittingcrab__task-12345__runner`

**Invariants**:
- Deterministic: Same inputs always produce same name
- Unique: Different work IDs → different sessions
- Safe: No shell metacharacters

---

## Implementation (Pseudocode)

### Rust Using `tokio` + `ssh2` Crate

```rust
// src/main.rs

use clap::{Parser, Subcommand};
use std::process;

mod ssh;
mod session;
mod validate;

use validate::{validate_repo, validate_work, validate_role};
use session::SessionName;
use ssh::SSH;

#[derive(Parser)]
#[command(name = "aivcs-session")]
#[command(about = "Manage tmux sessions on aivcs.local")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Attach to or create a tmux session
    Attach {
        #[arg(long)]
        repo: String,
        #[arg(long)]
        work: String,
        #[arg(long)]
        role: String,
    },
    /// Kill a session
    Kill {
        #[arg(long)]
        session: String,
    },
    /// List all aivcs sessions
    List,
    /// Prune old sessions
    Prune {
        #[arg(long)]
        older_than: u64,  // hours
        #[arg(long)]
        role: Option<String>,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    match cli.command {
        Commands::Attach { repo, work, role } => {
            if let Err(e) = attach(&repo, &work, &role).await {
                eprintln!("❌ Error: {}", e);
                process::exit(1);
            }
        }
        Commands::Kill { session } => {
            if let Err(e) = kill(&session).await {
                eprintln!("❌ Error: {}", e);
                process::exit(1);
            }
        }
        Commands::List => {
            if let Err(e) = list().await {
                eprintln!("❌ Error: {}", e);
                process::exit(1);
            }
        }
        Commands::Prune { older_than, role } => {
            if let Err(e) = prune(older_than, role.as_deref()).await {
                eprintln!("❌ Error: {}", e);
                process::exit(1);
            }
        }
    }
}

async fn attach(repo: &str, work: &str, role: &str) -> Result<(), String> {
    // 1. Validate inputs
    let repo = validate_repo(repo)?;
    let work = validate_work(work)?;
    let role = validate_role(role)?;

    // 2. Generate session name
    let session_name = SessionName::new(&repo, &work, &role);

    // 3. Connect to aivcs.local
    let mut ssh = SSH::connect("aivcs", "aivcs.local").await?;

    // 4. Check repo exists
    let repo_dir = format!("~/engineering/code/clone-base/{}", repo);
    ssh.check_dir_exists(&repo_dir).await?;

    // 5. Attach/create tmux session
    ssh.tmux_attach_or_create(
        &session_name.to_string(),
        &repo_dir,
    ).await?;

    Ok(())
}

async fn kill(session: &str) -> Result<(), String> {
    let mut ssh = SSH::connect("aivcs", "aivcs.local").await?;
    ssh.tmux_kill_session(session).await?;
    Ok(())
}

async fn list() -> Result<(), String> {
    let mut ssh = SSH::connect("aivcs", "aivcs.local").await?;
    let sessions = ssh.tmux_list_sessions().await?;
    for (name, created) in sessions {
        if name.starts_with("aivcs__") {
            let age = chrono::Utc::now()
                .signed_duration_since(created)
                .num_minutes();
            println!("{}   (created {}m ago)", name, age);
        }
    }
    Ok(())
}

async fn prune(older_than: u64, role: Option<&str>) -> Result<(), String> {
    let mut ssh = SSH::connect("aivcs", "aivcs.local").await?;
    let sessions = ssh.tmux_list_sessions().await?;
    let cutoff = chrono::Utc::now() - chrono::Duration::hours(older_than as i64);

    for (name, created) in sessions {
        if !name.starts_with("aivcs__") {
            continue;
        }
        if created < cutoff {
            // Optional: check role
            if let Some(r) = role {
                if !name.ends_with(&format!("__{}", r)) {
                    continue;
                }
            }
            ssh.tmux_kill_session(&name).await.ok();  // ignore errors
        }
    }
    Ok(())
}
```

### SSH Module

```rust
// src/ssh.rs

use tokio::process::Command;
use std::time::Duration;

pub struct SSH {
    user: String,
    host: String,
}

impl SSH {
    pub async fn connect(user: &str, host: &str) -> Result<Self, String> {
        // Test connection with a simple ping
        let output = Command::new("autossh")
            .arg("-M")
            .arg("0")
            .arg("-o")
            .arg("StrictHostKeyChecking=no")
            .arg("-o")
            .arg("ConnectTimeout=5")
            .arg(format!("{}@{}", user, host))
            .arg("echo ok")
            .output()
            .await
            .map_err(|e| format!("SSH connection failed: {}", e))?;

        if !output.status.success() {
            return Err("SSH connection test failed".to_string());
        }

        Ok(SSH {
            user: user.to_string(),
            host: host.to_string(),
        })
    }

    async fn exec(&self, cmd: &str) -> Result<String, String> {
        let full_cmd = format!(
            r#"autossh -M 0 -t {}@{} "{}""#,
            self.user, self.host, cmd
        );

        let output = Command::new("sh")
            .arg("-c")
            .arg(&full_cmd)
            .output()
            .await
            .map_err(|e| format!("exec failed: {}", e))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(stderr.to_string());
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    pub async fn check_dir_exists(&mut self, dir: &str) -> Result<(), String> {
        self.exec(&format!("[ -d {} ] || exit 1", dir)).await?;
        Ok(())
    }

    pub async fn tmux_attach_or_create(
        &mut self,
        session_name: &str,
        start_dir: &str,
    ) -> Result<(), String> {
        let cmd = format!(
            r#"/opt/homebrew/bin/tmux new-session -A -s "{}" -c "{}""#,
            session_name, start_dir
        );
        self.exec(&cmd).await?;
        Ok(())
    }

    pub async fn tmux_kill_session(&mut self, session_name: &str) -> Result<(), String> {
        let cmd = format!(
            r#"/opt/homebrew/bin/tmux kill-session -t "{}""#,
            session_name
        );
        self.exec(&cmd).await?;
        Ok(())
    }

    pub async fn tmux_list_sessions(&mut self) -> Result<Vec<(String, chrono::DateTime<chrono::Utc>)>, String> {
        let cmd = r#"/opt/homebrew/bin/tmux list-sessions -F '#{session_name}|#{session_created}'"#;
        let output = self.exec(cmd).await?;

        let mut sessions = Vec::new();
        for line in output.lines() {
            let parts: Vec<&str> = line.split('|').collect();
            if parts.len() == 2 {
                let name = parts[0].to_string();
                // Parse timestamp (could be unix epoch, needs investigation)
                sessions.push((name, chrono::Utc::now()));
            }
        }

        Ok(sessions)
    }
}
```

### Validation Module

```rust
// src/validate.rs

pub fn validate_repo(s: &str) -> Result<String, String> {
    if s.is_empty() {
        return Err("repo name cannot be empty".to_string());
    }
    if s.len() > 100 {
        return Err("repo name too long (max 100)".to_string());
    }

    let lower = s.to_lowercase();

    if lower.contains("..") {
        return Err("repo name forbids '..'".to_string());
    }
    if lower.contains('/') {
        return Err("repo name forbids '/'".to_string());
    }

    let sanitized: String = lower
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();

    Ok(sanitized)
}

pub fn validate_work(s: &str) -> Result<String, String> {
    if s.is_empty() {
        return Err("work id cannot be empty".to_string());
    }
    if s.len() > 100 {
        return Err("work id too long (max 100)".to_string());
    }

    let lower = s.to_lowercase();

    if lower.contains("..") || lower.contains('/') {
        return Err("work id contains forbidden characters".to_string());
    }

    let sanitized: String = lower
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._-".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();

    Ok(sanitized)
}

pub fn validate_role(s: &str) -> Result<String, String> {
    let lower = s.to_lowercase();
    match lower.as_str() {
        "agent" | "runner" | "human" => Ok(lower),
        _ => Err(format!(
            "invalid role '{}' (must be: agent, runner, human)",
            s
        )),
    }
}
```

### Session Name Type

```rust
// src/session.rs

#[derive(Debug, Clone)]
pub struct SessionName {
    repo: String,
    work: String,
    role: String,
}

impl SessionName {
    pub fn new(repo: &str, work: &str, role: &str) -> Self {
        SessionName {
            repo: repo.to_string(),
            work: work.to_string(),
            role: role.to_string(),
        }
    }
}

impl std::fmt::Display for SessionName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "aivcs__{}__{}__{}", self.repo, self.work, self.role)
    }
}
```

---

## Cargo.toml

```toml
[package]
name = "aivcs-session"
version = "0.1.0"
edition = "2021"

[dependencies]
tokio = { version = "1", features = ["process", "macros", "rt-multi-thread"] }
clap = { version = "4", features = ["derive"] }
chrono = { version = "0.4", features = ["serde"] }
thiserror = "1"

[[bin]]
name = "aivcs-session"
path = "src/main.rs"
```

---

## Testing Strategy

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_repo_valid() {
        assert_eq!(
            validate_repo("knittingCrab").unwrap(),
            "knittingcrab"
        );
    }

    #[test]
    fn test_validate_repo_forbid_dot_dot() {
        assert!(validate_repo("../secret").is_err());
    }

    #[test]
    fn test_validate_repo_forbid_slash() {
        assert!(validate_repo("repo/subdir").is_err());
    }

    #[test]
    fn test_validate_role_valid() {
        assert_eq!(validate_role("RUNNER").unwrap(), "runner");
    }

    #[test]
    fn test_validate_role_invalid() {
        assert!(validate_role("invalid").is_err());
    }

    #[test]
    fn test_session_name_format() {
        let session = SessionName::new("knittingcrab", "task-001", "runner");
        assert_eq!(
            session.to_string(),
            "aivcs__knittingcrab__task-001__runner"
        );
    }
}
```

### Integration Tests (require `aivcs.local` available)

```rust
#[tokio::test]
#[ignore]  // requires aivcs.local access
async fn test_attach_creates_session() {
    let repo = "knittingCrab";
    let work = "test-task-001";
    let role = "human";

    // Attach
    attach(repo, work, role).await.unwrap();

    // List and verify
    // ...

    // Cleanup
    kill(&format!("aivcs__knittingcrab__test-task-001__human"))
        .await
        .unwrap();
}
```

Run with:
```bash
cargo test -- --ignored --test-threads=1
```

---

## Deployment

### Installation

```bash
# Build
cargo build --release

# Install to ~/.cargo/bin/ (or system path)
cp target/release/aivcs-session ~/.cargo/bin/
chmod +x ~/.cargo/bin/aivcs-session

# Verify
aivcs-session --help
```

### Scheduler Integration

**In CoordinatorServer**:
```rust
async fn dispatch_to_remote_worker(
    repo_name: &str,
    task_id: &TaskId,
    worker_id: &WorkerId,
) -> Result<(), String> {
    let session_name = format!(
        "aivcs__{}__{}__{}",
        repo_name, task_id, "runner"
    );

    // Shell out to aivcs-session
    let cmd = format!(
        "aivcs-session attach --repo {} --work {} --role runner",
        repo_name, task_id
    );

    tokio::process::Command::new("sh")
        .arg("-c")
        .arg(&cmd)
        .output()
        .await?;

    // Worker should now be connected; normal dispatch proceeds
    Ok(())
}
```

---

## Future Enhancements

1. **Connection pooling**: Reuse SSH connections across multiple session operations
2. **Metrics**: Track session creation time, cleanup frequency
3. **Config file**: Support custom `aivcs.local` hostname, base directory
4. **Logging**: Structured logs for debugging SSH/tmux issues
5. **Graceful termination**: Allow sessions to finish current work before killing
