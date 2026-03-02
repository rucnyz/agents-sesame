use std::collections::{HashMap, HashSet};

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

const SEARCH_CACHE_CAPACITY: usize = 64;

/// Per-adapter timing: (name, duration, new_count).
pub type AdapterTimings = Vec<(String, std::time::Duration, usize)>;

pub struct ScanTimings {
    pub adapters: AdapterTimings,
    pub index_write: std::time::Duration,
    pub new_count: usize,
    pub deleted_count: usize,
}

pub struct SessionSearch {
    adapters: Vec<Box<dyn AgentAdapter>>,
    sessions_by_id: HashMap<String, Session>,
    index: TantivyIndex,
    /// Cache: query key → Tantivy results (id, score). Cleared on index update.
    search_cache: HashMap<String, Vec<(String, f64)>>,
    /// Timing data from the last scan (for --stats diagnostics).
    pub last_scan_timings: Option<ScanTimings>,
    /// Pending changes from deferred commit.
    pending_new: Vec<Session>,
    pending_deleted: Vec<String>,
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
            search_cache: HashMap::new(),
            last_scan_timings: None,
            pending_new: Vec::new(),
            pending_deleted: Vec::new(),
        }
    }

    /// Scan adapters and collect changes. Returns (all_new, all_deleted, adapter_timings).
    fn scan_adapters(
        &mut self,
        force_refresh: bool,
        agent_hint: Option<&str>,
    ) -> (Vec<Session>, Vec<String>, AdapterTimings) {
        let known = if force_refresh {
            HashMap::new()
        } else {
            self.index.get_known_sessions()
        };

        if force_refresh {
            self.index.clear();
        }

        let adapters_to_scan: Vec<usize> = match agent_hint {
            Some(agent) => self
                .adapters
                .iter()
                .enumerate()
                .filter(|(_, a)| a.name() == agent)
                .map(|(i, _)| i)
                .collect(),
            None => (0..self.adapters.len()).collect(),
        };

        let mut all_new: Vec<Session> = Vec::new();
        let mut all_deleted: Vec<String> = Vec::new();
        let mut adapter_timings: AdapterTimings = Vec::new();
        for i in adapters_to_scan {
            let t = std::time::Instant::now();
            let (new_sessions, deleted) =
                self.adapters[i].find_sessions_incremental(&known, &None, &None);
            let elapsed = t.elapsed();
            let count = new_sessions.len();
            adapter_timings.push((self.adapters[i].name().to_string(), elapsed, count));
            all_new.extend(new_sessions);
            all_deleted.extend(deleted);
        }

        (all_new, all_deleted, adapter_timings)
    }

    /// Get all sessions with immediate Tantivy commit.
    /// Used by TUI and --stats.
    pub fn get_all_sessions(
        &mut self,
        force_refresh: bool,
        agent_hint: Option<&str>,
    ) -> Vec<Session> {
        if !force_refresh && self.index.is_fresh(5) {
            return self.finalize_sessions();
        }

        let (all_new, all_deleted, adapter_timings) = self.scan_adapters(force_refresh, agent_hint);

        let index_start = std::time::Instant::now();
        self.index.batch_update(&all_deleted, &all_new);
        let index_elapsed = index_start.elapsed();

        self.index.touch_scan_marker();
        self.last_scan_timings = Some(ScanTimings {
            adapters: adapter_timings,
            index_write: index_elapsed,
            new_count: all_new.len(),
            deleted_count: all_deleted.len(),
        });
        self.finalize_sessions()
    }

    /// Get all sessions with deferred Tantivy commit.
    /// Returns sessions immediately via in-memory overlay (old index + new/deleted).
    /// Call `commit_pending()` afterwards to persist to Tantivy.
    pub fn get_all_sessions_deferred(
        &mut self,
        force_refresh: bool,
        agent_hint: Option<&str>,
    ) -> Vec<Session> {
        // For rebuild, commit synchronously (index was wiped, must persist)
        if force_refresh {
            return self.get_all_sessions(true, agent_hint);
        }

        if self.index.is_fresh(5) {
            return self.finalize_sessions();
        }

        let (all_new, all_deleted, adapter_timings) = self.scan_adapters(false, agent_hint);

        self.last_scan_timings = Some(ScanTimings {
            adapters: adapter_timings,
            index_write: std::time::Duration::ZERO,
            new_count: all_new.len(),
            deleted_count: all_deleted.len(),
        });

        // No changes → fast path: just read from Tantivy, no overlay or commit needed
        if all_new.is_empty() && all_deleted.is_empty() {
            self.index.touch_scan_marker();
            return self.finalize_sessions();
        }

        // Has changes → in-memory overlay, stash for deferred commit
        let sessions = self.finalize_with_overlay(&all_new, &all_deleted);
        self.pending_new = all_new;
        self.pending_deleted = all_deleted;
        sessions
    }

    /// Whether there are pending changes to commit.
    pub fn has_pending(&self) -> bool {
        !self.pending_new.is_empty() || !self.pending_deleted.is_empty()
    }

    /// Commit any pending changes from `get_all_sessions_deferred()` to Tantivy.
    pub fn commit_pending(&mut self) {
        if self.pending_new.is_empty() && self.pending_deleted.is_empty() {
            return;
        }
        let new = std::mem::take(&mut self.pending_new);
        let deleted = std::mem::take(&mut self.pending_deleted);
        self.index.batch_update(&deleted, &new);
        self.index.touch_scan_marker();
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

    /// Read from Tantivy index and populate sessions_by_id.
    fn finalize_sessions(&mut self) -> Vec<Session> {
        self.search_cache.clear();
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

    /// Read from Tantivy index, apply in-memory overlay (new + deleted), populate sessions_by_id.
    fn finalize_with_overlay(&mut self, new: &[Session], deleted: &[String]) -> Vec<Session> {
        self.search_cache.clear();
        let mut sessions = self.index.get_all_sessions();

        // Build lookup sets
        let deleted_set: HashSet<&str> = deleted.iter().map(|s| s.as_str()).collect();
        let new_ids: HashSet<&str> = new.iter().map(|s| s.id.as_str()).collect();

        // Remove deleted and sessions that will be replaced
        sessions
            .retain(|s| !deleted_set.contains(s.id.as_str()) && !new_ids.contains(s.id.as_str()));

        // Add new/updated sessions
        sessions.extend(new.iter().cloned());

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

    /// Build a cache key from search parameters.
    fn cache_key(
        query: &str,
        agent_filter: Option<&str>,
        directory_filter: Option<&str>,
    ) -> String {
        format!(
            "{}\0{}\0{}",
            query,
            agent_filter.unwrap_or(""),
            directory_filter.unwrap_or("")
        )
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
        let key = Self::cache_key(query, agent_filter, directory_filter);

        // Check cache
        if let Some(cached) = self.search_cache.get(&key) {
            return cached
                .iter()
                .filter_map(|(id, score)| self.sessions_by_id.get(id).map(|s| (s.clone(), *score)))
                .collect();
        }

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

        // Store in cache (evict all if full)
        if self.search_cache.len() >= SEARCH_CACHE_CAPACITY {
            self.search_cache.clear();
        }
        self.search_cache.insert(key, results.clone());

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
