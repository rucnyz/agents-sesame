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
    /// Currently scanning this adapter (name, index, total_adapters).
    Scanning(String, usize, usize),
    /// Per-session progress within an adapter (adapter_name, sessions_found).
    Progress(String, usize),
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
    /// Pending changes from list_streaming(), committed later via fork.
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
        verbose: bool,
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

        let total_adapters = adapters_to_scan.len();
        let mut all_new: Vec<Session> = Vec::new();
        let mut all_deleted: Vec<String> = Vec::new();
        let mut adapter_timings: AdapterTimings = Vec::new();
        for (step, i) in adapters_to_scan.into_iter().enumerate() {
            let name = self.adapters[i].name().to_string();
            if verbose {
                use std::io::Write;
                eprint!("\rScanning {name} [{}/{}]\x1b[K", step + 1, total_adapters);
                let _ = std::io::stderr().flush();
            }
            let on_session: crate::adapter::SessionCallback = if verbose {
                use std::io::Write;
                let name_p = name.clone();
                let step_p = step;
                let total_p = total_adapters;
                let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let count_p = count.clone();
                Some(Box::new(move |_s: &Session| {
                    let c = count_p.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    if c == 1 || c.is_multiple_of(50) {
                        eprint!(
                            "\rScanning {name_p} ({c} sessions) [{}/{}]\x1b[K",
                            step_p + 1,
                            total_p
                        );
                        let _ = std::io::stderr().flush();
                    }
                }))
            } else {
                None
            };
            let t = std::time::Instant::now();
            let (new_sessions, deleted) =
                self.adapters[i].find_sessions_incremental(&known, &None, &on_session);
            let elapsed = t.elapsed();
            let count = new_sessions.len();
            adapter_timings.push((name, elapsed, count));
            all_new.extend(new_sessions);
            all_deleted.extend(deleted);
        }

        if verbose {
            use std::io::Write;
            eprint!("\r\x1b[K");
            let _ = std::io::stderr().flush();
        }

        (all_new, all_deleted, adapter_timings)
    }

    /// Get all sessions with immediate Tantivy commit.
    /// Used by TUI and --stats.
    pub fn get_all_sessions(
        &mut self,
        force_refresh: bool,
        agent_hint: Option<&str>,
        verbose: bool,
    ) -> Vec<Session> {
        if !force_refresh && self.index.is_fresh(5) {
            return self.finalize_sessions();
        }

        let (all_new, all_deleted, adapter_timings) =
            self.scan_adapters(force_refresh, agent_hint, verbose);

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

    /// Streaming list for CLI: emit cached sessions first, then scan.
    ///
    /// Tantivy commit is deferred — call `commit_pending()` afterwards.
    /// This lets the caller fork so the parent exits early (giving TV/fzf
    /// an early EOF) while the child commits in the background.
    pub fn list_streaming(
        &mut self,
        force_refresh: bool,
        agent_hint: Option<&str>,
        mut emit: impl FnMut(&[Session]),
    ) {
        if force_refresh {
            let sessions = self.get_all_sessions(true, agent_hint, false);
            emit(&sessions);
            return;
        }

        // Phase 1: emit cached sessions from Tantivy immediately
        let cached = self.finalize_sessions();
        if !cached.is_empty() {
            emit(&cached);
        }

        if self.index.is_fresh(5) {
            return;
        }

        // Phase 2: scan for changes, emit new sessions immediately
        let (all_new, all_deleted, adapter_timings) = self.scan_adapters(false, agent_hint, false);

        if !all_new.is_empty() {
            emit(&all_new);
        }

        // Store pending changes for deferred commit
        self.pending_new = all_new;
        self.pending_deleted = all_deleted;
        self.last_scan_timings = Some(ScanTimings {
            adapters: adapter_timings,
            index_write: std::time::Duration::ZERO,
            new_count: self.pending_new.len(),
            deleted_count: self.pending_deleted.len(),
        });
    }

    /// Whether there are pending changes to commit.
    pub fn has_pending(&self) -> bool {
        !self.pending_new.is_empty() || !self.pending_deleted.is_empty()
    }

    /// Commit pending changes to Tantivy index (called from forked child).
    pub fn commit_pending(&mut self) {
        if !self.has_pending() {
            return;
        }
        self.index
            .batch_update(&self.pending_deleted, &self.pending_new);
        self.index.touch_scan_marker();
        self.pending_new.clear();
        self.pending_deleted.clear();
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
            let name = self.adapters[i].name().to_string();
            let _ = tx.send(LoadingMsg::Scanning(name.clone(), i, adapter_count));

            // Per-session progress callback (throttled: every 50 sessions)
            let tx_p = tx.clone();
            let name_p = name.clone();
            let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let count_p = count.clone();
            let on_session: crate::adapter::SessionCallback =
                Some(Box::new(move |_s: &Session| {
                    let c = count_p.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    if c == 1 || c.is_multiple_of(50) {
                        let _ = tx_p.send(LoadingMsg::Progress(name_p.clone(), c));
                    }
                }));

            let (new_sessions, deleted) =
                self.adapters[i].find_sessions_incremental(&known, &None, &on_session);
            // Send final count if not already sent
            let final_count = count.load(std::sync::atomic::Ordering::Relaxed);
            if final_count > 0 && !final_count.is_multiple_of(50) {
                let _ = tx.send(LoadingMsg::Progress(name, final_count));
            }
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

    /// Load content for a session if not already loaded.
    /// Returns true if content was loaded (or already present).
    pub fn ensure_session_content(&mut self, id: &str) -> bool {
        if let Some(session) = self.sessions_by_id.get(id)
            && !session.content.is_empty()
        {
            return true;
        }
        if let Some(content) = self.index.get_session_content(id)
            && let Some(session) = self.sessions_by_id.get_mut(id)
        {
            session.content = content;
            return true;
        }
        false
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
