use clap::Parser;

use crate::search::SessionSearch;

#[derive(Parser)]
#[command(
    name = "fr-rs",
    about = "Fast fuzzy finder for coding agent session history"
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
}

pub fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if cli.ids || cli.no_tui || cli.list_only {
        list_sessions(&cli)?;
    } else {
        crate::tui::run_tui(cli.yolo, cli.directory.as_deref())?;
    }

    Ok(())
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
        print_sessions(&results);
    }
    Ok(())
}

fn apply_basic_filters(
    mut sessions: Vec<crate::session::Session>,
    cli: &Cli,
) -> Vec<crate::session::Session> {
    if let Some(ref agent) = cli.agent {
        sessions.retain(|s| s.agent == *agent);
    }
    if let Some(ref dir) = cli.directory {
        let lower = dir.to_lowercase();
        sessions.retain(|s| s.directory.to_lowercase().contains(&lower));
    }
    sessions
}

fn print_sessions(sessions: &[crate::session::Session]) {
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
