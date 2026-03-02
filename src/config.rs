use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

/// Serde helper: accepts either a single string or array of strings in TOML.
#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum KeyOrKeys {
    Single(String),
    Multiple(Vec<String>),
}

pub struct AgentConfig {
    pub color: &'static str,
    pub badge: &'static str,
}

pub const AGENTS: &[(&str, AgentConfig)] = &[
    (
        "claude",
        AgentConfig {
            color: "#E87B35",
            badge: "claude",
        },
    ),
    (
        "codex",
        AgentConfig {
            color: "#00A67E",
            badge: "codex",
        },
    ),
    (
        "opencode",
        AgentConfig {
            color: "#CFCECD",
            badge: "opencode",
        },
    ),
    (
        "vibe",
        AgentConfig {
            color: "#FF6B35",
            badge: "vibe",
        },
    ),
    (
        "crush",
        AgentConfig {
            color: "#6B51FF",
            badge: "crush",
        },
    ),
    (
        "copilot-cli",
        AgentConfig {
            color: "#9CA3AF",
            badge: "copilot",
        },
    ),
    (
        "copilot-vscode",
        AgentConfig {
            color: "#007ACC",
            badge: "vscode",
        },
    ),
    (
        "qwen",
        AgentConfig {
            color: "#615CED",
            badge: "qwen",
        },
    ),
    (
        "gemini",
        AgentConfig {
            color: "#4285F4",
            badge: "gemini",
        },
    ),
    (
        "kimi",
        AgentConfig {
            color: "#1A73E8",
            badge: "kimi",
        },
    ),
];

#[allow(dead_code)]
pub const SCHEMA_VERSION: u32 = 20;

fn home_dir() -> PathBuf {
    dirs::home_dir().expect("could not determine home directory")
}

pub fn claude_dir() -> PathBuf {
    home_dir().join(".claude").join("projects")
}

pub fn codex_dir() -> PathBuf {
    home_dir().join(".codex").join("sessions")
}

pub fn copilot_dir() -> PathBuf {
    home_dir().join(".copilot").join("session-state")
}

pub fn opencode_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| home_dir().join(".local/share"))
        .join("opencode")
}

pub fn opencode_db() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| home_dir().join(".local/share"))
        .join("opencode/opencode.db")
}

pub fn vibe_dir() -> PathBuf {
    home_dir().join(".vibe/logs/session")
}

pub fn crush_projects_file() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| home_dir().join(".local/share"))
        .join("crush/projects.json")
}

pub fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| home_dir().join(".cache"))
        .join("agents-sesame")
}

pub fn config_file() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| home_dir().join(".config"))
        .join("agents-sesame/config.toml")
}

pub fn index_dir() -> PathBuf {
    cache_dir().join("tantivy_index_rs")
}

#[allow(dead_code)]
pub fn log_file() -> PathBuf {
    cache_dir().join("parse-errors.log")
}

/// Default search result limit (max results returned by Tantivy per query).
pub const DEFAULT_SEARCH_LIMIT: usize = 1000;

/// Application configuration loaded from TOML.
#[derive(Debug, Deserialize, Default)]
pub struct AppConfig {
    #[serde(default)]
    pub agents: HashMap<String, AgentPathConfig>,
    #[serde(default)]
    pub keybindings: HashMap<String, KeyOrKeys>,
    #[serde(default)]
    pub theme: Option<crate::tui::theme::ThemeConfig>,
    /// Max search results returned per query. Default: 1000.
    pub search_limit: Option<usize>,
}

/// Per-agent path configuration. All fields optional.
#[derive(Debug, Deserialize, Default, Clone)]
pub struct AgentPathConfig {
    pub dir: Option<PathBuf>,
    pub db: Option<PathBuf>,
    pub projects_file: Option<PathBuf>,
    pub chat_dir: Option<PathBuf>,
    pub workspace_dir: Option<PathBuf>,
    pub legacy_dir: Option<PathBuf>,
}

impl AppConfig {
    /// Load config from ~/.config/agents-sesame/config.toml, returning defaults if missing.
    pub fn load() -> Self {
        let path = config_file();
        match fs::read_to_string(path) {
            Ok(s) => toml::from_str(&s).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Get a dir path for an agent, falling back to the provided default.
    pub fn agent_dir(&self, agent: &str, default: PathBuf) -> PathBuf {
        self.agents
            .get(agent)
            .and_then(|c| c.dir.clone())
            .map(expand_tilde)
            .unwrap_or(default)
    }

    /// Get a db path for an agent, falling back to the provided default.
    pub fn agent_db(&self, agent: &str, default: PathBuf) -> PathBuf {
        self.agents
            .get(agent)
            .and_then(|c| c.db.clone())
            .map(expand_tilde)
            .unwrap_or(default)
    }

    /// Get a projects_file path for an agent, falling back to the provided default.
    pub fn agent_projects_file(&self, agent: &str, default: PathBuf) -> PathBuf {
        self.agents
            .get(agent)
            .and_then(|c| c.projects_file.clone())
            .map(expand_tilde)
            .unwrap_or(default)
    }

    /// Get chat_dir for an agent (copilot-vscode), falling back to default.
    pub fn agent_chat_dir(&self, agent: &str, default: PathBuf) -> PathBuf {
        self.agents
            .get(agent)
            .and_then(|c| c.chat_dir.clone())
            .map(expand_tilde)
            .unwrap_or(default)
    }

    /// Get workspace_dir for an agent (copilot-vscode), falling back to default.
    pub fn agent_workspace_dir(&self, agent: &str, default: PathBuf) -> PathBuf {
        self.agents
            .get(agent)
            .and_then(|c| c.workspace_dir.clone())
            .map(expand_tilde)
            .unwrap_or(default)
    }

    /// Get legacy_dir for an agent (opencode), falling back to default.
    pub fn agent_legacy_dir(&self, agent: &str, default: PathBuf) -> PathBuf {
        self.agents
            .get(agent)
            .and_then(|c| c.legacy_dir.clone())
            .map(expand_tilde)
            .unwrap_or(default)
    }
}

/// Expand ~ to home directory in a path.
fn expand_tilde(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        home_dir().join(rest)
    } else if s == "~" {
        home_dir()
    } else {
        path
    }
}

pub fn get_agent_config(name: &str) -> Option<&'static AgentConfig> {
    AGENTS.iter().find(|(n, _)| *n == name).map(|(_, c)| c)
}
