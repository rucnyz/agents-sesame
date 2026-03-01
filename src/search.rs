use std::collections::HashMap;

use crate::adapter::AgentAdapter;
use crate::adapters::{
    ClaudeAdapter, CodexAdapter, CopilotAdapter, CopilotVSCodeAdapter, CrushAdapter, GeminiAdapter,
    KimiAdapter, OpenCodeAdapter, QwenAdapter, VibeAdapter,
};
use crate::config::{self, AppConfig};
use crate::index::TantivyIndex;
use crate::query::{Filter, parse_query};
use crate::session::Session;

pub enum LoadingMsg {
    Sessions(Vec<Session>),
    Done(Box<SessionSearch>),
}

pub struct SessionSearch {
    adapters: Vec<Box<dyn AgentAdapter>>,
    sessions_by_id: HashMap<String, Session>,
    index: TantivyIndex,
}

impl Default for SessionSearch {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionSearch {
    pub fn new() -> Self {
        let cfg = AppConfig::load();
        Self {
            adapters: vec![
                Box::new(ClaudeAdapter::new(
                    cfg.agent_dir("claude", config::claude_dir()),
                )),
                Box::new(CodexAdapter::new(
                    cfg.agent_dir("codex", config::codex_dir()),
                )),
                Box::new(CopilotAdapter::new(
                    cfg.agent_dir("copilot-cli", config::copilot_dir()),
                )),
                Box::new(CopilotVSCodeAdapter::new(
                    cfg.agent_chat_dir("copilot-vscode", CopilotVSCodeAdapter::default_chat_dir()),
                    cfg.agent_workspace_dir(
                        "copilot-vscode",
                        CopilotVSCodeAdapter::default_workspace_dir(),
                    ),
                )),
                Box::new(CrushAdapter::new(
                    cfg.agent_projects_file("crush", config::crush_projects_file()),
                )),
                Box::new(GeminiAdapter::new(
                    cfg.agent_dir("gemini", GeminiAdapter::default_dir()),
                )),
                Box::new(KimiAdapter::new(
                    cfg.agent_dir("kimi", KimiAdapter::default_dir()),
                )),
                Box::new(OpenCodeAdapter::new(
                    cfg.agent_db("opencode", config::opencode_db()),
                    cfg.agent_legacy_dir("opencode", config::opencode_dir().join("storage")),
                )),
                Box::new(QwenAdapter::new(
                    cfg.agent_dir("qwen", QwenAdapter::default_dir()),
                )),
                Box::new(VibeAdapter::new(cfg.agent_dir("vibe", config::vibe_dir()))),
            ],
            sessions_by_id: HashMap::new(),
            index: TantivyIndex::new(),
        }
    }

    /// Get all sessions, using incremental updates.
    pub fn get_all_sessions(&mut self, force_refresh: bool) -> Vec<Session> {
        let known = if force_refresh {
            HashMap::new()
        } else {
            self.index.get_known_sessions()
        };

        if force_refresh {
            self.index.clear();
        }

        let mut all_new: Vec<Session> = Vec::new();
        let mut all_deleted: Vec<String> = Vec::new();

        for adapter in &self.adapters {
            let (new_sessions, deleted) = adapter.find_sessions_incremental(&known, &None, &None);
            all_new.extend(new_sessions);
            all_deleted.extend(deleted);
        }

        // Apply changes
        if !all_deleted.is_empty() {
            self.index.delete_sessions(&all_deleted);
        }
        if !all_new.is_empty() {
            self.index.update_sessions(&all_new);
        }

        self.finalize_sessions()
    }

    /// Progressive loading: send cached sessions first, then update after each adapter.
    pub fn load_progressive(
        &mut self,
        force_refresh: bool,
        tx: &std::sync::mpsc::Sender<LoadingMsg>,
    ) {
        let known = if force_refresh {
            HashMap::new()
        } else {
            self.index.get_known_sessions()
        };

        if force_refresh {
            self.index.clear();
        }

        // Send cached sessions from index immediately (warm start)
        if !known.is_empty() {
            let cached = self.finalize_sessions();
            let _ = tx.send(LoadingMsg::Sessions(cached));
        }

        // Process each adapter and send updates
        let adapter_count = self.adapters.len();
        for i in 0..adapter_count {
            let (new_sessions, deleted) =
                self.adapters[i].find_sessions_incremental(&known, &None, &None);
            let has_changes = !new_sessions.is_empty() || !deleted.is_empty();
            if !deleted.is_empty() {
                self.index.delete_sessions(&deleted);
            }
            if !new_sessions.is_empty() {
                self.index.update_sessions(&new_sessions);
            }
            if has_changes {
                let updated = self.finalize_sessions();
                let _ = tx.send(LoadingMsg::Sessions(updated));
            }
        }
    }

    fn finalize_sessions(&mut self) -> Vec<Session> {
        let mut sessions = self.index.get_all_sessions();
        for s in &sessions {
            self.sessions_by_id.insert(s.id.clone(), s.clone());
        }
        sessions.sort_by(|a, b| {
            b.mtime
                .partial_cmp(&a.mtime)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        sessions
    }

    /// Search sessions with query and filters.
    /// Returns sessions paired with their BM25 relevance scores.
    pub fn search(
        &mut self,
        query: &str,
        agent_filter: Option<&str>,
        directory_filter: Option<&str>,
        limit: usize,
    ) -> Vec<(Session, f64)> {
        let parsed = parse_query(query);

        let effective_agent = if let Some(agent) = agent_filter {
            Some(Filter {
                include: vec![agent.to_string()],
                exclude: vec![],
            })
        } else {
            parsed.agent
        };

        let effective_dir = if let Some(dir) = directory_filter {
            Some(Filter {
                include: vec![dir.to_string()],
                exclude: vec![],
            })
        } else {
            parsed.directory
        };

        let results = self.index.search(
            &parsed.text,
            effective_agent.as_ref(),
            effective_dir.as_ref(),
            parsed.date.as_ref(),
            limit,
        );

        results
            .into_iter()
            .filter_map(|(id, score)| self.sessions_by_id.get(&id).map(|s| (s.clone(), score)))
            .collect()
    }

    /// Look up a session by its ID.
    pub fn get_session_by_id(&self, id: &str) -> Option<&Session> {
        self.sessions_by_id.get(id)
    }

    /// Get the resume command for a session.
    pub fn get_resume_command(&self, session: &Session, yolo: bool) -> Vec<String> {
        for adapter in &self.adapters {
            if adapter.name() == session.agent {
                return adapter.get_resume_command(session, yolo);
            }
        }
        vec![]
    }
}
