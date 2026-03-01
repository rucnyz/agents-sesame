use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDateTime};
use serde_json::Value;

use crate::adapter::{incremental_scan, AgentAdapter, ErrorCallback, SessionCallback};
use crate::session::{truncate_title, RawAdapterStats, Session};

pub struct VibeAdapter {
    sessions_dir: PathBuf,
}

impl VibeAdapter {
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self { sessions_dir }
    }

    fn scan_session_files(&self) -> HashMap<String, (PathBuf, f64)> {
        let mut files = HashMap::new();
        let dir = &self.sessions_dir;
        if !dir.is_dir() {
            return files;
        }

        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return files,
        };

        for entry in entries.flatten() {
            let session_dir = entry.path();
            if !session_dir.is_dir() {
                continue;
            }
            let name = session_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if !name.starts_with("session_") {
                continue;
            }

            let meta_file = session_dir.join("meta.json");
            if !meta_file.exists() {
                continue;
            }

            // Read session_id from meta.json
            let session_id = if let Ok(data) = fs::read(&meta_file) {
                serde_json::from_slice::<Value>(&data)
                    .ok()
                    .and_then(|v| v.get("session_id").and_then(Value::as_str).map(String::from))
                    .unwrap_or_else(|| name.to_string())
            } else {
                name.to_string()
            };

            let mtime = fs::metadata(&meta_file)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0);

            files.insert(session_id, (session_dir, mtime));
        }
        files
    }

    fn parse_session_dir(session_dir: &Path) -> Option<Session> {
        let meta_file = session_dir.join("meta.json");
        let messages_file = session_dir.join("messages.jsonl");

        let meta_data = fs::read(&meta_file).ok()?;
        let metadata: Value = serde_json::from_slice(&meta_data).ok()?;

        let mtime = fs::metadata(&meta_file)
            .ok()?
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs_f64();

        let session_id = metadata
            .get("session_id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                session_dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
            })
            .to_string();

        let directory = metadata
            .get("environment")
            .and_then(|e| e.get("working_directory"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        // Check yolo: config.auto_approve or root auto_approve
        let yolo = metadata
            .get("config")
            .and_then(|c| c.get("auto_approve"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || metadata
                .get("auto_approve")
                .and_then(Value::as_bool)
                .unwrap_or(false);

        // Parse timestamp from start_time (ISO format)
        let timestamp = metadata
            .get("start_time")
            .and_then(Value::as_str)
            .and_then(|s| {
                // Try parsing ISO 8601
                DateTime::parse_from_rfc3339(s)
                    .ok()
                    .map(|dt| dt.naive_utc())
                    .or_else(|| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f").ok())
                    .or_else(|| NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").ok())
            })
            .or_else(|| DateTime::from_timestamp(mtime as i64, 0).map(|dt| dt.naive_utc()))?;

        let meta_title = metadata
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        // Read messages
        let mut messages: Vec<String> = Vec::new();
        let mut first_user_content = String::new();

        if messages_file.exists()
            && let Ok(data) = fs::read(&messages_file) {
                for line in data.split(|&b| b == b'\n') {
                    if line.is_empty() {
                        continue;
                    }
                    let Ok(msg) = serde_json::from_slice::<Value>(line) else {
                        continue;
                    };

                    let role = msg.get("role").and_then(Value::as_str).unwrap_or("");
                    if role == "system" {
                        continue;
                    }

                    let prefix = if role == "user" { "» " } else { "  " };
                    let content = msg.get("content");

                    match content {
                        Some(Value::String(text)) => {
                            if !text.is_empty() {
                                messages.push(format!("{prefix}{text}"));
                                if role == "user" && first_user_content.is_empty() {
                                    first_user_content = text.clone();
                                }
                            }
                        }
                        Some(Value::Array(parts)) => {
                            for part in parts {
                                if let Some(text) = part.get("text").and_then(Value::as_str)
                                    && !text.is_empty() {
                                        messages.push(format!("{prefix}{text}"));
                                        if role == "user" && first_user_content.is_empty() {
                                            first_user_content = text.to_string();
                                        }
                                    }
                            }
                        }
                        _ => {}
                    }
                }
            }

        let title = if !meta_title.is_empty() {
            meta_title
        } else if !first_user_content.is_empty() {
            truncate_title(&first_user_content, 80)
        } else {
            "Vibe session".to_string()
        };

        let full_content = messages.join("\n\n");
        let message_count = messages.len();

        Some(Session {
            id: session_id,
            agent: "vibe".to_string(),
            title,
            directory,
            timestamp,
            content: full_content,
            message_count,
            mtime,
            yolo,
        })
    }
}

impl AgentAdapter for VibeAdapter {
    fn name(&self) -> &str {
        "vibe"
    }
    fn color(&self) -> &str {
        "#FF6B35"
    }
    fn badge(&self) -> &str {
        "vibe"
    }
    fn supports_yolo(&self) -> bool {
        true
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
        on_error: &ErrorCallback,
        on_session: &SessionCallback,
    ) -> (Vec<Session>, Vec<String>) {
        incremental_scan(
            self.name(),
            self.is_available(),
            || self.scan_session_files(),
            |path| Self::parse_session_dir(path),
            known,
            on_error,
            on_session,
        )
    }

    fn get_resume_command(&self, session: &Session, yolo: bool) -> Vec<String> {
        let mut cmd = vec!["vibe".to_string()];
        if yolo {
            cmd.push("--agent".to_string());
            cmd.push("auto-approve".to_string());
        }
        cmd.push("--resume".to_string());
        cmd.push(session.id.clone());
        cmd
    }

    fn get_raw_stats(&self) -> RawAdapterStats {
        let dir = &self.sessions_dir;
        let mut file_count = 0;
        let mut total_bytes = 0;
        if dir.is_dir() {
            for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
                if entry.file_type().is_file() {
                    file_count += 1;
                    total_bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
                }
            }
        }
        RawAdapterStats {
            agent: "vibe".to_string(),
            data_dir: dir.display().to_string(),
            available: dir.is_dir(),
            file_count,
            total_bytes,
        }
    }
}
