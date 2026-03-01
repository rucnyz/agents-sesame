use std::collections::HashMap;
use std::path::PathBuf;

use crate::session::{ParseError, RawAdapterStats, Session};

pub type ErrorCallback = Option<Box<dyn Fn(&ParseError) + Send + Sync>>;
pub type SessionCallback = Option<Box<dyn Fn(&Session) + Send + Sync>>;

/// Trait for all agent adapters.
#[allow(dead_code)]
pub trait AgentAdapter: Send + Sync {
    fn name(&self) -> &str;
    fn color(&self) -> &str;
    fn badge(&self) -> &str;
    fn supports_yolo(&self) -> bool {
        false
    }

    fn is_available(&self) -> bool;
    fn find_sessions(&self) -> Vec<Session>;
    fn find_sessions_incremental(
        &self,
        known: &HashMap<String, (f64, String)>,
        on_error: &ErrorCallback,
        on_session: &SessionCallback,
    ) -> (Vec<Session>, Vec<String>);
    fn get_resume_command(&self, session: &Session, yolo: bool) -> Vec<String>;
    fn get_raw_stats(&self) -> RawAdapterStats;
}

/// mtime tolerance for comparison (1ms)
const MTIME_TOLERANCE: f64 = 0.001;

/// Helper that implements the incremental scan template method.
///
/// Adapters provide closures for scanning files and parsing sessions.
pub fn incremental_scan(
    adapter_name: &str,
    is_available: bool,
    scan_files: impl Fn() -> HashMap<String, (PathBuf, f64)>,
    parse_file: impl Fn(&PathBuf) -> Option<Session>,
    known: &HashMap<String, (f64, String)>,
    on_error: &ErrorCallback,
    on_session: &SessionCallback,
) -> (Vec<Session>, Vec<String>) {
    let _ = on_error; // available for future use

    // If adapter not available, report all known sessions for this agent as deleted
    if !is_available {
        let deleted_ids: Vec<String> = known
            .iter()
            .filter(|(_, (_, agent))| agent == adapter_name)
            .map(|(id, _)| id.clone())
            .collect();
        return (vec![], deleted_ids);
    }

    let current_files = scan_files();
    let mut new_or_modified = Vec::new();

    for (session_id, (path, mtime)) in &current_files {
        let needs_parse = match known.get(session_id) {
            Some((known_mtime, _)) => *mtime > *known_mtime + MTIME_TOLERANCE,
            None => true,
        };

        if needs_parse && let Some(mut session) = parse_file(path) {
            session.mtime = *mtime;
            if let Some(cb) = on_session {
                cb(&session);
            }
            new_or_modified.push(session);
        }
    }

    // Find deleted sessions
    let deleted_ids: Vec<String> = known
        .iter()
        .filter(|(_, (_, agent))| agent == adapter_name)
        .filter(|(id, _)| !current_files.contains_key(*id))
        .map(|(id, _)| id.clone())
        .collect();

    (new_or_modified, deleted_ids)
}
