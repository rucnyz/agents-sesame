use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::NaiveDateTime;
use serde_json::Value;

use crate::adapter::{AgentAdapter, ErrorCallback, SessionCallback};
use crate::session::{RawAdapterStats, Session, truncate_title};

fn gemini_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_default().join(".gemini/tmp")
}

pub struct GeminiAdapter {
    base_dir: PathBuf,
}

impl GeminiAdapter {
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    pub fn default_dir() -> PathBuf {
        gemini_dir()
    }

    /// Scan all session JSON files across all project hash directories.
    fn scan_session_files(&self) -> HashMap<String, (PathBuf, f64)> {
        let mut files = HashMap::new();
        if !self.base_dir.is_dir() {
            return files;
        }

        // ~/.gemini/tmp/<project_id>/chats/session-<ts>-<id8>.json
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
                if path.extension().is_none_or(|e| e != "json") {
                    continue;
                }
                let Some(fname) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if !fname.starts_with("session-") {
                    continue;
                }

                // Extract session ID from file content
                let session_id = Self::get_session_id_from_file(&path);
                if session_id.is_empty() {
                    continue;
                }

                let mtime = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0);

                files.insert(session_id, (path, mtime));
            }
        }
        files
    }

    fn get_session_id_from_file(path: &Path) -> String {
        if let Ok(data) = fs::read(path)
            && let Ok(val) = serde_json::from_slice::<Value>(&data)
            && let Some(id) = val.get("sessionId").and_then(Value::as_str)
        {
            return id.to_string();
        }
        // Fallback: extract from filename (session-YYYY-MM-DDTHH-mm-<id8>.json)
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string()
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

        let val: Value = serde_json::from_slice(&data).ok()?;

        let session_id = val.get("sessionId").and_then(Value::as_str)?.to_string();

        // Skip subagent sessions
        let kind = val.get("kind").and_then(Value::as_str).unwrap_or("main");
        if kind == "subagent" {
            return None;
        }

        let start_time = val.get("startTime").and_then(Value::as_str).unwrap_or("");
        let last_updated = val.get("lastUpdated").and_then(Value::as_str).unwrap_or("");

        // Get directory from directories[] array or project context
        let directory = val
            .get("directories")
            .and_then(Value::as_array)
            .and_then(|dirs| dirs.first())
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        let messages_arr = val.get("messages").and_then(Value::as_array)?;

        let mut messages: Vec<String> = Vec::new();
        let mut first_user_text = String::new();
        let mut turn_count: usize = 0;

        for msg in messages_arr {
            let msg_type = msg.get("type").and_then(Value::as_str).unwrap_or("");

            // Skip info/error/warning messages
            if msg_type != "user" && msg_type != "gemini" {
                continue;
            }

            let prefix = if msg_type == "user" { "» " } else { "  " };

            // Extract text from content parts
            let content = msg.get("content").or_else(|| msg.get("displayContent"));
            let mut has_text = false;

            if let Some(parts) = content.and_then(Value::as_array) {
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
            } else if let Some(text) = content.and_then(Value::as_str)
                && !text.is_empty()
            {
                messages.push(format!("{prefix}{text}"));
                has_text = true;
                if msg_type == "user" && first_user_text.is_empty() {
                    first_user_text = text.to_string();
                }
            }

            if has_text {
                turn_count += 1;
            }
        }

        if first_user_text.is_empty() || messages.is_empty() {
            return None;
        }

        let title = truncate_title(&first_user_text, 100);
        let full_content = messages.join("\n\n");

        // Parse timestamp: prefer lastUpdated, then startTime, then mtime
        let ts_str = if !last_updated.is_empty() {
            last_updated
        } else {
            start_time
        };
        let timestamp = parse_iso_timestamp(ts_str).or_else(|| {
            chrono::DateTime::from_timestamp(mtime as i64, 0).map(|dt| dt.naive_utc())
        })?;

        Some(Session {
            id: session_id,
            agent: "gemini".to_string(),
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

impl AgentAdapter for GeminiAdapter {
    fn name(&self) -> &str {
        "gemini"
    }
    fn color(&self) -> &str {
        "#4285F4"
    }
    fn badge(&self) -> &str {
        "gemini"
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
            "gemini".to_string(),
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
            agent: "gemini".to_string(),
            data_dir: self.base_dir.display().to_string(),
            available: self.is_available(),
            file_count: files.len(),
            total_bytes,
        }
    }
}
