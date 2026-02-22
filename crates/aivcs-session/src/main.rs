mod remote;
mod session;

use clap::{Parser, Subcommand};
use remote::{RemoteSessionManager, RemoteTarget};
use session::SessionConfig;
use tracing_subscriber::fmt;

/// aivcs-session: Deterministic tmux session manager for aivcs.local
#[derive(Parser)]
#[command(name = "aivcs-session")]
#[command(about = "Manage tmux sessions on aivcs.local for task execution")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Enable debug logging
    #[arg(global = true, short, long)]
    debug: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Attach or create a session
    Attach {
        /// Repository name (lowercase, alphanumeric + hyphen/underscore/dot)
        #[arg(long)]
        repo: String,

        /// Work ID (alphanumeric + hyphen/underscore/dot)
        #[arg(long)]
        work_id: String,

        /// Session role: agent, runner, human
        #[arg(long)]
        role: String,

        /// Remote host (default: aivcs.local)
        #[arg(long, default_value = "aivcs.local")]
        host: String,

        /// Remote user (default: aivcs)
        #[arg(long, default_value = "aivcs")]
        user: String,
    },

    /// List sessions on remote
    List {
        /// Remote host (default: aivcs.local)
        #[arg(long, default_value = "aivcs.local")]
        host: String,

        /// Remote user (default: aivcs)
        #[arg(long, default_value = "aivcs")]
        user: String,
    },

    /// Kill a session on remote
    Kill {
        /// Session name to kill
        #[arg(long)]
        session: String,

        /// Remote host (default: aivcs.local)
        #[arg(long, default_value = "aivcs.local")]
        host: String,

        /// Remote user (default: aivcs)
        #[arg(long, default_value = "aivcs")]
        user: String,
    },
}

fn init_logging(_debug: bool) {
    // Logging can be controlled via RUST_LOG environment variable
    fmt().with_writer(std::io::stderr).init();
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    init_logging(cli.debug);

    match cli.command {
        Commands::Attach {
            repo,
            work_id,
            role,
            host,
            user,
        } => {
            let role = role.parse()?;
            let config = SessionConfig::new(repo, work_id, role)?;
            let remote = RemoteTarget { host, user };
            let manager = RemoteSessionManager::new(remote);

            let session_name = manager.attach_or_create(&config)?;
            println!("{}", session_name);
            Ok(())
        }

        Commands::List { host, user } => {
            let remote = RemoteTarget { host, user };
            let manager = RemoteSessionManager::new(remote);

            let sessions = manager.list_sessions()?;
            for session in sessions {
                println!("{}", session);
            }
            Ok(())
        }

        Commands::Kill {
            session,
            host,
            user,
        } => {
            let remote = RemoteTarget { host, user };
            let manager = RemoteSessionManager::new(remote);

            manager.kill_session(&session)?;
            println!("Killed session: {}", session);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cli_attach_parsing() {
        let args = vec![
            "aivcs-session",
            "attach",
            "--repo",
            "myrepo",
            "--work-id",
            "job123",
            "--role",
            "runner",
        ];
        let cli = Cli::try_parse_from(&args);
        assert!(cli.is_ok());
    }

    #[test]
    fn test_cli_list_parsing() {
        let args = vec!["aivcs-session", "list"];
        let cli = Cli::try_parse_from(&args);
        assert!(cli.is_ok());
    }

    #[test]
    fn test_cli_kill_parsing() {
        let args = vec![
            "aivcs-session",
            "kill",
            "--session",
            "aivcs__myrepo__job__runner",
        ];
        let cli = Cli::try_parse_from(&args);
        assert!(cli.is_ok());
    }
}
