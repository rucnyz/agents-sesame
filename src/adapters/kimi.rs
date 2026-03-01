use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::NaiveDateTime;
use serde_json::Value;

use crate::adapter::{AgentAdapter, ErrorCallback, SessionCallback};
use crate::session::{RawAdapterStats, Session, truncate_title};

/// Kimi CLI stores sessions in a share directory.
/// The path structure is: <share_dir>/sessions/<path_hash>/<session_id>/
/// Each session dir contains: context.jsonl, wire.jsonl, state.json
fn kimi_sessions_base() -> PathBuf {
    // Kimi uses platformdirs; on Linux it's ~/.local/share/kimi/sessions/
    let home = dirs::home_dir().unwrap_or_default();

    // Check XDG data dir first
    if let Ok(dir) = std::env::var("XDG_DATA_HOME") {
        let p = PathBuf::from(dir).join("kimi/sessions");
        if p.is_dir() {
            return p;
        }
    }

    // Default: ~/.local/share/kimi/sessions/
    let default = home.join(".local/share/kimi/sessions");
    if default.is_dir() {
        return default;
    }

    // Also check ~/.kimi/sessions/ as alternative
    home.join(".kimi/sessions")
}

pub struct KimiAdapter {
    sessions_dir: PathBuf,
}

impl KimiAdapter {
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self { sessions_dir }
    }

    pub fn default_dir() -> PathBuf {
        kimi_sessions_base()
    }

    /// Scan all session directories.
    /// Structure: sessions/<workdir_hash>/<session_id>/context.jsonl
    fn scan_session_files(&self) -> HashMap<String, (PathBuf, f64)> {
        let mut files = HashMap::new();
        if !self.sessions_dir.is_dir() {
            return files;
        }

        let Ok(hash_entries) = fs::read_dir(&self.sessions_dir) else {
            return files;
        };

        for hash_entry in hash_entries.flatten() {
            let hash_dir = hash_entry.path();
            if !hash_dir.is_dir() {
                continue;
            }

            let Ok(session_entries) = fs::read_dir(&hash_dir) else {
                continue;
            };

            for session_entry in session_entries.flatten() {
                let session_dir = session_entry.path();
                if !session_dir.is_dir() {
                    continue;
                }

                let context_file = session_dir.join("context.jsonl");
                if !context_file.exists() {
                    continue;
                }

                let session_id = session_dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();

                if session_id.is_empty() {
                    continue;
                }

                let mtime = fs::metadata(&context_file)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0);

                // Pass the session directory (not context file) as path
                files.insert(session_id, (session_dir, mtime));
            }
        }
        files
    }

    fn parse_session_dir(session_dir: &Path) -> Option<Session> {
        let context_file = session_dir.join("context.jsonl");
        let data = fs::read(&context_file).ok()?;
        let mtime = fs::metadata(&context_file)
            .ok()?
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs_f64();

        let session_id = session_dir
            .file_name()
            .and_then(|n| n.to_str())?
            .to_string();

        // Try to get directory from state.json
        let state_file = session_dir.join("state.json");
        let directory = if state_file.exists() {
            fs::read(&state_file)
                .ok()
                .and_then(|d| serde_json::from_slice::<Value>(&d).ok())
                .and_then(|v| {
                    v.get("work_dir")
                        .or_else(|| v.get("cwd"))
                        .and_then(Value::as_str)
                        .map(String::from)
                })
                .unwrap_or_default()
        } else {
            String::new()
        };

        let mut messages: Vec<String> = Vec::new();
        let mut first_user_text = String::new();
        let mut first_timestamp = String::new();
        let mut turn_count: usize = 0;

        for line in data.split(|&b| b == b'\n') {
            if line.is_empty() {
                continue;
            }
            let Ok(val) = serde_json::from_slice::<Value>(line) else {
                continue;
            };

            let role = val.get("role").and_then(Value::as_str).unwrap_or("");

            // Skip system messages
            if role == "system" {
                continue;
            }

            if first_timestamp.is_empty()
                && let Some(ts) = val.get("timestamp").and_then(Value::as_str)
            {
                first_timestamp = ts.to_string();
            }

            let prefix = if role == "user" { "» " } else { "  " };

            // Content can be string or array of parts
            let content = val.get("content");
            match content {
                Some(Value::String(text)) => {
                    if !text.is_empty() {
                        messages.push(format!("{prefix}{text}"));
                        turn_count += 1;
                        if role == "user" && first_user_text.is_empty() && text.len() > 5 {
                            first_user_text = text.clone();
                        }
                    }
                }
                Some(Value::Array(parts)) => {
                    let mut has_text = false;
                    for part in parts {
                        if let Some(text) = part.get("text").and_then(Value::as_str)
                            && !text.is_empty()
                        {
                            messages.push(format!("{prefix}{text}"));
                            has_text = true;
                            if role == "user" && first_user_text.is_empty() && text.len() > 5 {
                                first_user_text = text.to_string();
                            }
                        }
                    }
                    if has_text {
                        turn_count += 1;
                    }
                }
                _ => {}
            }
        }

        if first_user_text.is_empty() || messages.is_empty() {
            return None;
        }

        let title = truncate_title(&first_user_text, 100);
        let full_content = messages.join("\n\n");

        let timestamp = parse_iso_timestamp(&first_timestamp).or_else(|| {
            chrono::DateTime::from_timestamp(mtime as i64, 0).map(|dt| dt.naive_utc())
        })?;

        Some(Session {
            id: session_id,
            agent: "kimi".to_string(),
            title,
            directory,
            timestamp,
            content: full_content,
            message_count: turn_count,
            mtime,
            yolo: false,
        })
    }
}

fn parse_iso_timestamp(s: &str) -> Option<NaiveDateTime> {
    if s.is_empty() {
        return None;
    }
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.naive_utc())
        .or_else(|| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f").ok())
        .or_else(|| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok())
}

impl AgentAdapter for KimiAdapter {
    fn name(&self) -> &str {
        "kimi"
    }
    fn color(&self) -> &str {
        "#1A73E8"
    }
    fn badge(&self) -> &str {
        "kimi"
    }
    fn is_available(&self) -> bool {
        self.sessions_dir.is_dir()
    }

    fn find_sessions(&self) -> Vec<Session> {
        if !self.is_available() {
            return vec![];
        }
        self.scan_session_files()
            .values()
            .filter_map(|(path, _)| Self::parse_session_dir(path))
            .collect()
    }

    fn find_sessions_incremental(
        &self,
        known: &HashMap<String, (f64, String)>,
        _on_error: &ErrorCallback,
        on_session: &SessionCallback,
    ) -> (Vec<Session>, Vec<String>) {
        if !self.is_available() {
            let deleted: Vec<String> = known
                .iter()
                .filter(|(_, (_, a))| a == self.name())
                .map(|(id, _)| id.clone())
                .collect();
            return (vec![], deleted);
        }

        let current = self.scan_session_files();
        let mut new_or_modified = Vec::new();

        for (session_id, (path, mtime)) in &current {
            let needs_parse = match known.get(session_id) {
                Some((known_mtime, _)) => *mtime > *known_mtime + 0.001,
                None => true,
            };
            if needs_parse && let Some(mut session) = Self::parse_session_dir(path) {
                session.mtime = *mtime;
                if let Some(cb) = on_session {
                    cb(&session);
                }
                new_or_modified.push(session);
            }
        }

        let deleted: Vec<String> = known
            .iter()
            .filter(|(_, (_, a))| a == self.name())
            .filter(|(id, _)| !current.contains_key(*id))
            .map(|(id, _)| id.clone())
            .collect();

        (new_or_modified, deleted)
    }

    fn get_resume_command(&self, session: &Session, _yolo: bool) -> Vec<String> {
        vec![
            "kimi".to_string(),
            "--resume".to_string(),
            session.id.clone(),
        ]
    }

    fn get_raw_stats(&self) -> RawAdapterStats {
        let files = self.scan_session_files();
        let total_bytes: u64 = files
            .values()
            .filter_map(|(p, _)| {
                let ctx = p.join("context.jsonl");
                fs::metadata(ctx).ok().map(|m| m.len())
            })
            .sum();
        RawAdapterStats {
            agent: "kimi".to_string(),
            data_dir: self.sessions_dir.display().to_string(),
            available: self.is_available(),
            file_count: files.len(),
            total_bytes,
        }
    }
}
