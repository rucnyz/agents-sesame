use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::NaiveDateTime;
use serde_json::Value;

use crate::adapter::{AgentAdapter, ErrorCallback, SessionCallback};
use crate::session::{RawAdapterStats, Session, truncate_title};

fn qwen_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".qwen/tmp")
}

pub struct QwenAdapter {
    base_dir: PathBuf,
}

impl QwenAdapter {
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    pub fn default_dir() -> PathBuf {
        qwen_dir()
    }

    /// Scan all session JSONL files across all project hash directories.
    fn scan_session_files(&self) -> HashMap<String, (PathBuf, f64)> {
        let mut files = HashMap::new();
        if !self.base_dir.is_dir() {
            return files;
        }

        // Each project has ~/.qwen/tmp/<project_hash>/chats/<session_id>.jsonl
        let Ok(project_entries) = fs::read_dir(&self.base_dir) else {
            return files;
        };

        for project_entry in project_entries.flatten() {
            let chats_dir = project_entry.path().join("chats");
            if !chats_dir.is_dir() {
                continue;
            }
            let Ok(chat_entries) = fs::read_dir(&chats_dir) else {
                continue;
            };
            for entry in chat_entries.flatten() {
                let path = entry.path();
                if path.extension().is_none_or(|e| e != "jsonl") {
                    continue;
                }
                // Session ID is filename stem (UUID)
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                // Validate UUID-like pattern (hex chars and hyphens, 32-36 chars)
                if stem.len() < 32 || !stem.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
                    continue;
                }
                let mtime = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0);
                files.insert(stem.to_string(), (path, mtime));
            }
        }
        files
    }

    fn parse_session_file(path: &Path) -> Option<Session> {
        let data = fs::read(path).ok()?;
        let mtime = fs::metadata(path)
            .ok()?
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs_f64();

        let mut session_id = String::new();
        let mut directory = String::new();
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

            // Extract session ID and cwd from first record
            if session_id.is_empty() {
                if let Some(id) = val.get("sessionId").and_then(Value::as_str) {
                    session_id = id.to_string();
                }
                if let Some(cwd) = val.get("cwd").and_then(Value::as_str) {
                    directory = cwd.to_string();
                }
                if let Some(ts) = val.get("timestamp").and_then(Value::as_str) {
                    first_timestamp = ts.to_string();
                }
            }

            let msg_type = val.get("type").and_then(Value::as_str).unwrap_or("");

            // Skip system messages
            if msg_type == "system" {
                continue;
            }

            // Extract text from message.parts[]
            let parts = val
                .get("message")
                .and_then(|m| m.get("parts"))
                .and_then(Value::as_array);

            if let Some(parts) = parts {
                let prefix = if msg_type == "user" { "» " } else { "  " };
                let mut has_text = false;

                for part in parts {
                    if let Some(text) = part.get("text").and_then(Value::as_str)
                        && !text.is_empty()
                    {
                        messages.push(format!("{prefix}{text}"));
                        has_text = true;
                        if msg_type == "user" && first_user_text.is_empty() {
                            first_user_text = text.to_string();
                        }
                    }
                }
                if has_text {
                    turn_count += 1;
                }
            }
        }

        if first_user_text.is_empty() || messages.is_empty() {
            return None;
        }

        if session_id.is_empty() {
            session_id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
        }

        let title = truncate_title(&first_user_text, 100);
        let full_content = messages.join("\n\n");

        // Parse ISO timestamp or fallback to mtime
        let timestamp = parse_iso_timestamp(&first_timestamp).or_else(|| {
            chrono::DateTime::from_timestamp(mtime as i64, 0).map(|dt| dt.naive_utc())
        })?;

        Some(Session {
            id: session_id,
            agent: "qwen".to_string(),
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

impl AgentAdapter for QwenAdapter {
    fn name(&self) -> &str {
        "qwen"
    }
    fn color(&self) -> &str {
        "#615CED"
    }
    fn badge(&self) -> &str {
        "qwen"
    }
    fn is_available(&self) -> bool {
        self.base_dir.is_dir()
    }

    fn find_sessions(&self) -> Vec<Session> {
        if !self.is_available() {
            return vec![];
        }
        self.scan_session_files()
            .values()
            .filter_map(|(path, _)| Self::parse_session_file(path))
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
            if needs_parse && let Some(mut session) = Self::parse_session_file(path) {
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
            "qwen-code".to_string(),
            "--session-id".to_string(),
            session.id.clone(),
        ]
    }

    fn get_raw_stats(&self) -> RawAdapterStats {
        let files = self.scan_session_files();
        let total_bytes: u64 = files
            .values()
            .filter_map(|(p, _)| fs::metadata(p).ok().map(|m| m.len()))
            .sum();
        RawAdapterStats {
            agent: "qwen".to_string(),
            data_dir: self.base_dir.display().to_string(),
            available: self.is_available(),
            file_count: files.len(),
            total_bytes,
        }
    }
}
