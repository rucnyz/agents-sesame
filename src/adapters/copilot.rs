use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::DateTime;
use regex::Regex;
use serde_json::Value;

use crate::adapter::{AgentAdapter, ErrorCallback, SessionCallback, incremental_scan};
use crate::session::{RawAdapterStats, Session, truncate_title};

pub struct CopilotAdapter {
    sessions_dir: PathBuf,
}

impl CopilotAdapter {
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
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "jsonl") {
                let session_id = Self::get_session_id_from_file(&path);
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
        if let Ok(data) = fs::read(path) {
            for line in data.split(|&b| b == b'\n') {
                if line.is_empty() {
                    continue;
                }
                if let Ok(val) = serde_json::from_slice::<Value>(line)
                    && val.get("type").and_then(Value::as_str) == Some("session.start")
                {
                    if let Some(id) = val
                        .get("data")
                        .and_then(|d| d.get("sessionId"))
                        .and_then(Value::as_str)
                        && !id.is_empty()
                    {
                        return id.to_string();
                    }
                    break;
                }
            }
        }
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

        let mut session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let mut first_user_message = String::new();
        let mut directory = String::new();
        let mut messages: Vec<String> = Vec::new();
        let mut turn_count: usize = 0;

        let folder_re = Regex::new(r"Folder (/[^\s]+)").ok()?;

        for line in data.split(|&b| b == b'\n') {
            if line.is_empty() {
                continue;
            }
            let Ok(val) = serde_json::from_slice::<Value>(line) else {
                continue;
            };

            let msg_type = val.get("type").and_then(Value::as_str).unwrap_or("");
            let data_obj = val.get("data").cloned().unwrap_or(Value::Null);

            match msg_type {
                "session.start" => {
                    if let Some(id) = data_obj.get("sessionId").and_then(Value::as_str) {
                        session_id = id.to_string();
                    }
                }
                "session.info" => {
                    if directory.is_empty() {
                        let info_type = data_obj
                            .get("infoType")
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        if info_type == "folder_trust" {
                            let message = data_obj
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            if let Some(caps) = folder_re.captures(message) {
                                directory = caps[1].to_string();
                            }
                        }
                    }
                }
                "user.message" => {
                    let content = data_obj
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if !content.is_empty() {
                        messages.push(format!("» {content}"));
                        turn_count += 1;
                        if first_user_message.is_empty() && content.len() > 10 {
                            first_user_message = content.to_string();
                        }
                    }
                }
                "assistant.message" => {
                    let content = data_obj
                        .get("content")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if !content.is_empty() {
                        messages.push(format!("  {content}"));
                        turn_count += 1;
                    }
                }
                _ => {}
            }
        }

        if first_user_message.is_empty() || messages.is_empty() {
            return None;
        }

        let title = truncate_title(&first_user_message, 100);
        let full_content = messages.join("\n\n");
        let timestamp = DateTime::from_timestamp(mtime as i64, 0)?.naive_utc();

        Some(Session {
            id: session_id,
            agent: "copilot-cli".to_string(),
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

impl AgentAdapter for CopilotAdapter {
    fn name(&self) -> &str {
        "copilot-cli"
    }
    fn color(&self) -> &str {
        "#9CA3AF"
    }
    fn badge(&self) -> &str {
        "copilot"
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
            .filter_map(|(path, _)| Self::parse_session_file(path))
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
            |path| Self::parse_session_file(path),
            known,
            on_error,
            on_session,
        )
    }

    fn get_resume_command(&self, session: &Session, yolo: bool) -> Vec<String> {
        let mut cmd = vec!["copilot".to_string()];
        if yolo {
            cmd.push("--allow-all-tools".to_string());
            cmd.push("--allow-all-paths".to_string());
        }
        cmd.push("--resume".to_string());
        cmd.push(session.id.clone());
        cmd
    }

    fn get_raw_stats(&self) -> RawAdapterStats {
        let dir = &self.sessions_dir;
        let mut file_count = 0;
        let mut total_bytes = 0;
        if dir.is_dir()
            && let Ok(entries) = fs::read_dir(dir)
        {
            for entry in entries.flatten() {
                if entry.path().extension().is_some_and(|e| e == "jsonl") {
                    file_count += 1;
                    total_bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
                }
            }
        }
        RawAdapterStats {
            agent: "copilot-cli".to_string(),
            data_dir: dir.display().to_string(),
            available: dir.is_dir(),
            file_count,
            total_bytes,
        }
    }
}
