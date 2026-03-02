use clap::{Parser, Subcommand};

use crate::search::SessionSearch;
use crate::session::Session;
use crate::tui::utils::format_time_ago;

#[derive(Parser)]
#[command(
    name = "ase",
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

    /// Print session content to stdout (for fzf/television preview)
    #[arg(long)]
    pub preview: Option<String>,

    /// Resume a session by ID directly (for fzf/television integration)
    #[arg(long)]
    pub resume: Option<String>,

    /// Output format for --list: table, tsv, json
    #[arg(long, default_value = "table")]
    pub format: String,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Set up shell integration (keybinding + completions)
    Init {
        /// Shell to configure (fish, bash, zsh). Auto-detected if omitted.
        shell: Option<String>,
    },
    /// Update ase to the latest version
    Update,
    /// Uninstall ase (remove binary, shell integration, and cache)
    Uninstall,
}

pub fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if let Some(ref cmd) = cli.command {
        return match cmd {
            Command::Update => crate::update::self_update(),
            Command::Uninstall => uninstall(),
            Command::Init { shell } => print_init(shell.as_deref().unwrap_or("")),
        };
    }

    if let Some(ref id) = cli.preview {
        return preview_session(id);
    }

    if let Some(ref id) = cli.resume {
        return resume_session_by_id(id, cli.yolo);
    }

    if cli.stats {
        return print_stats(&cli);
    }

    if cli.ids || cli.no_tui || cli.list_only {
        list_sessions(&cli)?;
    } else {
        crate::tui::run_tui(cli.yolo, cli.directory.as_deref())?;
    }

    Ok(())
}

fn uninstall() -> anyhow::Result<()> {
    use crate::config;

    println!("Uninstalling ase...\n");
    let mut any_removed = false;

    // Remove cache dir (~/.cache/agents-sesame)
    let cache = config::cache_dir();
    if cache.exists() {
        println!("Removing cache: {}", cache.display());
        std::fs::remove_dir_all(&cache)?;
        any_removed = true;
    }

    // Remove config dir (~/.config/agents-sesame)
    let config_file = config::config_file();
    let config_dir = config_file.parent().unwrap();
    if config_dir.exists() {
        println!("Removing config: {}", config_dir.display());
        std::fs::remove_dir_all(config_dir)?;
        any_removed = true;
    }

    // Remove source lines from shell config files
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot determine home dir"))?;
    let shell_configs = [
        home.join(".config/fish/config.fish"),
        home.join(".bashrc"),
        home.join(".zshrc"),
    ];
    for path in &shell_configs {
        if path.exists() {
            let content = std::fs::read_to_string(path)?;
            if content.contains("ase") {
                println!("Cleaning shell config: {}", path.display());
                let cleaned: Vec<&str> = content
                    .lines()
                    .filter(|line| !line.contains("ase"))
                    .collect();
                std::fs::write(path, cleaned.join("\n") + "\n")?;
                any_removed = true;
            }
        }
    }

    // Remove binary (try ~/.local/bin/ase, then current exe)
    let binary = std::env::current_exe().unwrap_or_default();
    let local_bin = home.join(".local/bin/ase");

    let symlink = home.join(".local/bin/agents-sesame");
    if local_bin.exists() || local_bin.symlink_metadata().is_ok() {
        println!("Removing binary: {}", local_bin.display());
        std::fs::remove_file(&local_bin)?;
        any_removed = true;
    }
    if symlink.exists() || symlink.symlink_metadata().is_ok() {
        println!("Removing symlink: {}", symlink.display());
        std::fs::remove_file(&symlink)?;
        any_removed = true;
    }
    if !any_removed && binary.exists() {
        println!("Binary at {} — remove manually", binary.display());
    }

    if any_removed {
        println!("\nDone. Restart your shell to complete uninstall.");
    } else {
        println!("Nothing to remove.");
    }
    Ok(())
}

const SUPPORTED_SHELLS: &[&str] = &["fish", "bash", "zsh", "elvish", "nushell", "powershell"];

/// Detect current shell from $SHELL env var.
fn detect_shell() -> anyhow::Result<String> {
    let shell_path = std::env::var("SHELL").unwrap_or_default();
    let shell_name = std::path::Path::new(&shell_path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    match shell_name {
        "fish" | "bash" | "zsh" | "elvish" | "nushell" => Ok(shell_name.into()),
        "nu" => Ok("nushell".into()),
        "pwsh" | "powershell" => Ok("powershell".into()),
        _ => anyhow::bail!(
            "Cannot detect shell from $SHELL={shell_path}. Specify explicitly: ase init <{}>",
            SUPPORTED_SHELLS.join("|")
        ),
    }
}

/// Generate clap completion script for the given shell.
fn generate_completions(shell: &str) -> String {
    use clap::CommandFactory;
    let mut cmd = Cli::command();
    let mut buf = Vec::new();
    match shell {
        "nushell" | "nu" => {
            clap_complete::generate(clap_complete_nushell::Nushell, &mut cmd, "ase", &mut buf);
        }
        _ => {
            let clap_shell = match shell {
                "fish" => clap_complete::Shell::Fish,
                "bash" => clap_complete::Shell::Bash,
                "zsh" => clap_complete::Shell::Zsh,
                "elvish" => clap_complete::Shell::Elvish,
                "powershell" => clap_complete::Shell::PowerShell,
                _ => return String::new(),
            };
            clap_complete::generate(clap_shell, &mut cmd, "ase", &mut buf);
        }
    }
    String::from_utf8(buf).unwrap_or_default()
}

/// Generate full shell integration code (completions + keybinding) as a String.
fn shell_init_code(shell: &str) -> String {
    let mut code = generate_completions(shell);
    match shell {
        "fish" => code.push_str(
            r#"
# ase value completions
complete -c ase -l agent -f -a 'claude codex copilot copilot-vscode crush gemini kimi opencode qwen vibe'
complete -c ase -l format -f -a 'table tsv json'
complete -c ase -n '__fish_seen_subcommand_from init' -f -a 'fish bash zsh elvish nushell powershell'
complete -c ase -l preview -f -a '(ase --list --format=tsv 2>/dev/null | while read -d\t id agent title rest; printf "%s\t%s: %s\n" $id $agent $title; end)'
complete -c ase -l resume -f -a '(ase --list --format=tsv 2>/dev/null | while read -d\t id agent title rest; printf "%s\t%s: %s\n" $id $agent $title; end)'

# ase keybinding (Alt+G)
function __ase_widget
    ase -d (pwd)
    commandline -f repaint
end
bind \eg __ase_widget
"#,
        ),
        "bash" => code.push_str(
            r#"
# ase value completions
_fr_rs_complete() {
    local cur prev
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"
    case "$prev" in
        --agent) COMPREPLY=($(compgen -W "claude codex copilot copilot-vscode crush gemini kimi opencode qwen vibe" -- "$cur")) ;;
        --format) COMPREPLY=($(compgen -W "table tsv json" -- "$cur")) ;;
        init) COMPREPLY=($(compgen -W "fish bash zsh" -- "$cur")) ;;
        --preview|--resume) COMPREPLY=($(compgen -W "$(ase --ids --list 2>/dev/null)" -- "$cur")) ;;
    esac
}
complete -F _fr_rs_complete ase

# ase keybinding (Alt+G)
__ase_widget() {
    ase -d "$PWD"
}
bind -x '"\eg":"__ase_widget"'
"#,
        ),
        "zsh" => code.push_str(
            r#"
# ase value completions
_fr_rs() {
    local -a agents=(claude codex copilot copilot-vscode crush gemini kimi opencode qwen vibe)
    local -a formats=(table tsv json)
    local -a shells=(fish bash zsh)
    case "$words[CURRENT-1]" in
        --agent) compadd -a agents ;;
        --format) compadd -a formats ;;
        init) compadd -a shells ;;
        --preview|--resume) compadd -- $(ase --ids --list 2>/dev/null) ;;
        *) _arguments '1:command:(init update uninstall)' '--agent[Filter by agent]:agent:($agents)' '--list[List sessions]' '--format[Output format]:format:($formats)' '--preview[Preview session]:id:' '--resume[Resume session]:id:' '--stats[Stats]' '--rebuild[Rebuild index]' '--yolo[Auto-approve]' ;;
    esac
}
compdef _fr_rs ase

# ase keybinding (Alt+G)
__ase_widget() {
    ase -d "$PWD"
    zle reset-prompt
}
zle -N __ase_widget
bindkey '\eg' __ase_widget
"#,
        ),
        // elvish / powershell: completions only (from clap_complete above)
        _ => {}
    }
    code
}

fn print_init(shell_arg: &str) -> anyhow::Result<()> {
    let shell = if shell_arg.is_empty() {
        detect_shell()?
    } else {
        let s = match shell_arg.to_lowercase().as_str() {
            "nu" => "nushell".into(),
            "pwsh" => "powershell".into(),
            other => other.to_string(),
        };
        if SUPPORTED_SHELLS.contains(&s.as_str()) {
            s
        } else {
            anyhow::bail!(
                "Unsupported shell: {shell_arg}. Supported: {}",
                SUPPORTED_SHELLS.join(", ")
            );
        }
    };

    // If piped (stdout is not a terminal), emit shell code directly
    if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        print!("{}", shell_init_code(&shell));
        return Ok(());
    }

    // Write init file to ~/.config/agents-sesame/init.{shell}
    let init_dir = crate::config::config_file().parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&init_dir)?;
    let init_file = init_dir.join(format!("init.{shell}"));
    std::fs::write(&init_file, shell_init_code(&shell))?;
    println!("Generated {}", init_file.display());

    // For fish/bash/zsh: auto-inject source line into shell config
    let home =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
    let shell_config = match shell.as_str() {
        "fish" => Some(home.join(".config/fish/config.fish")),
        "bash" => Some(home.join(".bashrc")),
        "zsh" => Some(home.join(".zshrc")),
        _ => None,
    };

    if let Some(config_path) = shell_config {
        let source_line = format!("source {}", init_file.display());
        let already_installed = config_path
            .exists()
            .then(|| std::fs::read_to_string(&config_path).unwrap_or_default())
            .is_some_and(|content| content.contains(&source_line));

        if already_installed {
            println!("Already installed in {}", config_path.display());
        } else {
            if let Some(parent) = config_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let snippet = format!("\n# ase shell integration\n{source_line}\n");
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&config_path)?;
            std::io::Write::write_all(&mut file, snippet.as_bytes())?;
            println!("Added source line to {}", config_path.display());
        }
        println!(
            "Restart your shell or run: source {}",
            config_path.display()
        );
    } else {
        println!(
            "Add to your shell config: eval (ase init {} | slurp)",
            shell
        );
    }

    // Offer to install television cable channel
    install_tv_channel()?;

    Ok(())
}

/// Detect television and offer to install the ase cable channel.
fn install_tv_channel() -> anyhow::Result<()> {
    // Check if television is installed
    if which::which("tv").is_err() {
        return Ok(());
    }

    let cable_dir = dirs::config_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap().join(".config"))
        .join("television/cable");
    let channel_file = cable_dir.join("ase.toml");

    if channel_file.exists() {
        println!(
            "Television channel already installed at {}",
            channel_file.display()
        );
        return Ok(());
    }

    // Ask user
    eprint!("Television detected. Install ase channel for `tv ase`? [Y/n] ");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let answer = input.trim().to_lowercase();
    if !answer.is_empty() && answer != "y" && answer != "yes" {
        println!("Skipped television channel installation.");
        return Ok(());
    }

    std::fs::create_dir_all(&cable_dir)?;
    std::fs::write(
        &channel_file,
        include_str!("../docs/television-channel.toml"),
    )?;
    println!("Installed television channel at {}", channel_file.display());
    println!("Try: tv ase");
    Ok(())
}

fn preview_session(id: &str) -> anyhow::Result<()> {
    let mut engine = SessionSearch::new();
    engine.get_all_sessions(false, None);
    let session = engine
        .get_session_by_id(id)
        .ok_or_else(|| anyhow::anyhow!("Session not found: {id}"))?;
    print!("{}", session.content);
    Ok(())
}

fn resume_session_by_id(id: &str, yolo: bool) -> anyhow::Result<()> {
    let mut engine = SessionSearch::new();
    engine.get_all_sessions(false, None);
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

fn print_stats(cli: &Cli) -> anyhow::Result<()> {
    let total_start = std::time::Instant::now();

    let init_start = std::time::Instant::now();
    let mut engine = SessionSearch::new();
    let init_elapsed = init_start.elapsed();

    let scan_start = std::time::Instant::now();
    let sessions = engine.get_all_sessions(cli.rebuild, cli.agent.as_deref());
    let scan_elapsed = scan_start.elapsed();

    let total_elapsed = total_start.elapsed();

    println!("=== Scan Diagnostics ===\n");
    println!(
        "Total: {:.1}ms  (init: {:.1}ms, scan+index: {:.1}ms)",
        total_elapsed.as_secs_f64() * 1000.0,
        init_elapsed.as_secs_f64() * 1000.0,
        scan_elapsed.as_secs_f64() * 1000.0,
    );
    println!("Sessions: {} total\n", sessions.len());

    if let Some(ref timings) = engine.last_scan_timings {
        println!("{:<18} {:>8}  {:>6}", "Adapter", "Time", "New");
        println!("{}", "-".repeat(36));
        let mut sorted = timings.adapters.clone();
        sorted.sort_by(|a, b| b.1.cmp(&a.1));
        for (name, duration, count) in &sorted {
            let ms = duration.as_secs_f64() * 1000.0;
            if *count > 0 {
                println!("{:<18} {:>7.1}ms  {:>5}", name, ms, count);
            } else {
                println!("{:<18} {:>7.1}ms      -", name, ms);
            }
        }
        println!(
            "\nIndex write: {:.1}ms ({} new, {} deleted)",
            timings.index_write.as_secs_f64() * 1000.0,
            timings.new_count,
            timings.deleted_count,
        );
    } else {
        println!("(skipped scan — index fresh)");
    }

    Ok(())
}

fn list_sessions(cli: &Cli) -> anyhow::Result<()> {
    use std::io::Write;

    let start = std::time::Instant::now();
    let mut engine = SessionSearch::new();
    let has_query = cli.query.as_ref().is_some_and(|q| !q.is_empty());

    if has_query || cli.ids || cli.format != "tsv" {
        // Non-streaming path: need all sessions for search/table/json
        let sessions = engine.get_all_sessions(cli.rebuild, cli.agent.as_deref());
        let results = if let Some(ref query) = cli.query {
            if !query.is_empty() {
                engine
                    .search(
                        query,
                        cli.agent.as_deref(),
                        cli.directory.as_deref(),
                        sessions.len().max(1),
                    )
                    .into_iter()
                    .map(|(s, _)| s)
                    .collect()
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
                "json" => print_sessions_json(&results),
                _ => print_sessions(&results, start.elapsed()),
            }
        }
    } else {
        // Streaming TSV path: emit data first, then fork to commit Tantivy
        // in the background. Parent exits early → TV/fzf get early EOF.
        let agent = cli.agent.clone();
        let directory = cli.directory.clone();
        engine.list_streaming(cli.rebuild, cli.agent.as_deref(), |sessions| {
            let filtered =
                apply_basic_filters_ref(sessions, agent.as_deref(), directory.as_deref());
            print_sessions_tsv(&filtered);
            let _ = std::io::stdout().flush();
        });
        fork_commit(engine);
        return Ok(());
    }

    Ok(())
}

/// Fork so parent exits early (EOF for TV/fzf), child commits Tantivy.
fn fork_commit(mut engine: SessionSearch) {
    if !engine.has_pending() {
        return;
    }
    unsafe {
        let pid = libc::fork();
        if pid < 0 {
            // fork failed — commit synchronously
            engine.commit_pending();
        } else if pid == 0 {
            // child: close stdout, commit in background, then exit
            libc::close(1);
            engine.commit_pending();
            std::process::exit(0);
        }
        // parent: return immediately (process exits, TV gets EOF)
    }
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

fn apply_basic_filters_ref(
    sessions: &[Session],
    agent: Option<&str>,
    directory: Option<&str>,
) -> Vec<Session> {
    let dir_lower = directory.map(|d| d.to_lowercase());
    sessions
        .iter()
        .filter(|s| agent.is_none_or(|a| s.agent == a))
        .filter(|s| {
            dir_lower
                .as_ref()
                .is_none_or(|d| s.directory.to_lowercase().contains(d.as_str()))
        })
        .cloned()
        .collect()
}

fn print_sessions(sessions: &[Session], elapsed: std::time::Duration) {
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

    let ms = elapsed.as_secs_f64() * 1000.0;
    println!("\nShowing {display_count} of {total} sessions ({ms:.0}ms)");
}

fn print_sessions_tsv(sessions: &[Session]) {
    let home = dirs::home_dir().unwrap_or_default();
    let home_str = home.to_string_lossy();
    for s in sessions {
        let dir = s.directory.replace(&*home_str, "~");
        let date = format_time_ago(s.timestamp, chrono::Local::now().naive_local());
        // Replace newlines in title to keep one TSV record per line
        let title = s.title.replace('\n', " ").replace('\r', "");
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}",
            s.id, s.agent, title, dir, s.message_count, date
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
