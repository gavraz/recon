use clap::{Parser, Subcommand};

/// Monitor and manage Claude Code sessions running in tmux
#[derive(Parser)]
#[command(name = "recon", version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Open the visual (tamagotchi) dashboard
    View,
    /// Interactive form to create a new tmux session
    New,
    /// Create a new claude session (background by default)
    Launch {
        /// Custom session name (defaults to directory name)
        #[arg(long)]
        name: Option<String>,
        /// Working directory (defaults to current directory)
        #[arg(long)]
        cwd: Option<String>,
        /// Custom command to run instead of claude (e.g. "claude --model sonnet")
        #[arg(long)]
        command: Option<String>,
        /// Attach to the session after creating it
        #[arg(long)]
        attach: bool,
        /// Tag the session (key:value, repeatable)
        #[arg(long)]
        tag: Vec<String>,
    },
    /// Jump directly to the next agent waiting for input
    Next,
    /// Resume a past session (interactive picker, or by ID)
    Resume {
        /// Session ID to resume directly (skips the picker)
        #[arg(long)]
        id: Option<String>,
        /// Custom tmux session name
        #[arg(long)]
        name: Option<String>,
        /// Don't attach to the session after resuming
        #[arg(long)]
        no_attach: bool,
    },
    /// Print all session state as JSON
    Json {
        /// Filter sessions by tag (key:value, repeatable, must match all)
        #[arg(long)]
        tag: Vec<String>,
    },
    /// Inspect recon configuration
    Config {
        /// List the columns available for the table
        #[arg(long)]
        available_columns: bool,
    },
    /// Save all live sessions to disk for restoring later
    Park,
    /// Restore previously parked sessions
    Unpark,
}
