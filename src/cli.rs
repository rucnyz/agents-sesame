use clap::Parser;

use crate::search::SessionSearch;
use crate::session::Session;
use crate::tui::utils::format_time_ago;

#[derive(Parser)]
#[command(
    name = "fr-rs",
    about = "Fast fuzzy finder for coding agent session history",
    version
)]
pub struct Cli {
    /// Search query
    pub query: Option<String>,

    /// Filter by agent
    #[arg(short, long)]
    pub agent: Option<String>,

    /// Filter by directory (substring match)
    #[arg(short, long)]
    pub directory: Option<String>,

    /// Output list to stdout instead of TUI
    #[arg(long)]
    pub no_tui: bool,

    /// Just list sessions, don't resume
    #[arg(long = "list")]
    pub list_only: bool,

    /// Force rebuild the session index
    #[arg(long)]
    pub rebuild: bool,

    /// Show index statistics
    #[arg(long)]
    pub stats: bool,

    /// Resume with auto-approve/skip-permissions
    #[arg(long)]
    pub yolo: bool,

    /// Output only session IDs (for testing/scripting)
    #[arg(long, hide = true)]
    pub ids: bool,

    /// Update fr-rs to the latest version
    #[arg(long)]
    pub update: bool,

    /// Print session content to stdout (for fzf/television preview)
    #[arg(long)]
    pub preview: Option<String>,

    /// Resume a session by ID directly (for fzf/television integration)
    #[arg(long)]
    pub resume: Option<String>,

    /// Output format for --list: table, tsv, json
    #[arg(long, default_value = "table")]
    pub format: String,
}

pub fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if cli.update {
        return crate::update::self_update();
    }

    if let Some(ref id) = cli.preview {
        return preview_session(id);
    }

    if let Some(ref id) = cli.resume {
        return resume_session_by_id(id, cli.yolo);
    }

    if cli.ids || cli.no_tui || cli.list_only {
        list_sessions(&cli)?;
    } else {
        crate::tui::run_tui(cli.yolo, cli.directory.as_deref())?;
    }

    Ok(())
}

fn preview_session(id: &str) -> anyhow::Result<()> {
    let mut engine = SessionSearch::new();
    engine.get_all_sessions(false);
    let session = engine
        .get_session_by_id(id)
        .ok_or_else(|| anyhow::anyhow!("Session not found: {id}"))?;
    print!("{}", session.content);
    Ok(())
}

fn resume_session_by_id(id: &str, yolo: bool) -> anyhow::Result<()> {
    let mut engine = SessionSearch::new();
    engine.get_all_sessions(false);
    let session = engine
        .get_session_by_id(id)
        .ok_or_else(|| anyhow::anyhow!("Session not found: {id}"))?
        .clone();
    let cmd = engine.get_resume_command(&session, yolo);
    if cmd.is_empty() {
        anyhow::bail!("No resume command for session: {id}");
    }
    let mut command = std::process::Command::new(&cmd[0]);
    command.args(&cmd[1..]);
    if !session.directory.is_empty() {
        let dir = std::path::Path::new(&session.directory);
        if dir.is_dir() {
            command.current_dir(dir);
        }
    }
    let status = command.status()?;
    std::process::exit(status.code().unwrap_or(1));
}

fn list_sessions(cli: &Cli) -> anyhow::Result<()> {
    let mut engine = SessionSearch::new();

    // Index all sessions (incremental)
    let sessions = engine.get_all_sessions(cli.rebuild);

    // If there's a query, use full-text search
    let results = if let Some(ref query) = cli.query {
        if !query.is_empty() {
            engine.search(
                query,
                cli.agent.as_deref(),
                cli.directory.as_deref(),
                100,
                false,
            )
        } else {
            apply_basic_filters(sessions, cli)
        }
    } else {
        apply_basic_filters(sessions, cli)
    };

    if cli.ids {
        for s in &results {
            println!("{}", s.id);
        }
    } else {
        match cli.format.as_str() {
            "tsv" => print_sessions_tsv(&results),
            "json" => print_sessions_json(&results),
            _ => print_sessions(&results),
        }
    }
    Ok(())
}

fn apply_basic_filters(mut sessions: Vec<Session>, cli: &Cli) -> Vec<Session> {
    if let Some(ref agent) = cli.agent {
        sessions.retain(|s| s.agent == *agent);
    }
    if let Some(ref dir) = cli.directory {
        let lower = dir.to_lowercase();
        sessions.retain(|s| s.directory.to_lowercase().contains(&lower));
    }
    sessions
}

fn print_sessions(sessions: &[Session]) {
    let home = dirs::home_dir().unwrap_or_default();
    let home_str = home.to_string_lossy();
    let total = sessions.len();
    let display_count = total.min(50);

    println!("{:<10} {:<52} {:<37} ID", "Agent", "Title", "Directory");
    println!("{}", "-".repeat(120));

    for session in sessions.iter().take(display_count) {
        let title = if session.title.chars().count() > 50 {
            let truncated: String = session.title.chars().take(50).collect();
            format!("{truncated}...")
        } else {
            session.title.clone()
        };

        let dir = session.directory.replace(&*home_str, "~");
        let dir = if dir.chars().count() > 35 {
            let last32: String = dir
                .chars()
                .rev()
                .take(32)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            format!("...{last32}")
        } else {
            dir
        };

        println!(
            "{:<10} {:<52} {:<37} {}",
            session.agent, title, dir, session.id
        );
    }

    println!("\nShowing {display_count} of {total} sessions");
}

fn print_sessions_tsv(sessions: &[Session]) {
    let home = dirs::home_dir().unwrap_or_default();
    let home_str = home.to_string_lossy();
    for s in sessions {
        let dir = s.directory.replace(&*home_str, "~");
        let date = format_time_ago(s.timestamp);
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}",
            s.id, s.agent, s.title, dir, s.message_count, date
        );
    }
}

fn print_sessions_json(sessions: &[Session]) {
    for s in sessions {
        let obj = serde_json::json!({
            "id": s.id,
            "agent": s.agent,
            "title": s.title,
            "directory": s.directory,
            "turns": s.message_count,
            "timestamp": s.timestamp.to_string(),
        });
        println!("{}", serde_json::to_string(&obj).unwrap());
    }
}
