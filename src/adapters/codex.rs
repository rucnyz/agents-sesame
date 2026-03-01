use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::DateTime;
use serde_json::Value;

use crate::adapter::{incremental_scan, AgentAdapter, ErrorCallback, SessionCallback};
use crate::session::{truncate_title, RawAdapterStats, Session};

pub struct CodexAdapter {
    sessions_dir: PathBuf,
}

impl CodexAdapter {
    pub fn new(sessions_dir: PathBuf) -> Self {
        Self { sessions_dir }
    }

    fn scan_session_files(&self) -> HashMap<String, (PathBuf, f64)> {
        let mut files = HashMap::new();
        let dir = &self.sessions_dir;
        if !dir.is_dir() {
            return files;
        }

        for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
            let path = entry.path().to_path_buf();
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
        // Try to extract session_id from session_meta in file
        if let Ok(data) = fs::read(path) {
            for line in data.split(|&b| b == b'\n') {
                if line.is_empty() {
                    continue;
                }
                if let Ok(val) = serde_json::from_slice::<Value>(line)
                    && val.get("type").and_then(Value::as_str) == Some("session_meta") {
                        if let Some(id) = val
                            .get("payload")
                            .and_then(|p| p.get("id"))
                            .and_then(Value::as_str)
                            && !id.is_empty() {
                                return id.to_string();
                            }
                        break;
                    }
            }
        }
        // Fallback: filename stem
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

        let mut session_id = String::new();
        let mut directory = String::new();
        let mut messages: Vec<String> = Vec::new();
        let mut user_prompts: Vec<String> = Vec::new();
        let mut turn_count: usize = 0;
        let mut yolo = false;

        for line in data.split(|&b| b == b'\n') {
            if line.is_empty() {
                continue;
            }
            let Ok(val) = serde_json::from_slice::<Value>(line) else {
                continue;
            };

            let msg_type = val.get("type").and_then(Value::as_str).unwrap_or("");
            let payload = val.get("payload").cloned().unwrap_or(Value::Null);

            match msg_type {
                "session_meta" => {
                    if let Some(id) = payload.get("id").and_then(Value::as_str) {
                        session_id = id.to_string();
                    }
                    if let Some(cwd) = payload.get("cwd").and_then(Value::as_str) {
                        directory = cwd.to_string();
                    }
                }
                "turn_context" => {
                    let approval = payload
                        .get("approval_policy")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let sandbox_mode = payload
                        .get("sandbox_policy")
                        .and_then(|sp| sp.get("mode"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if approval == "never" || sandbox_mode == "danger-full-access" {
                        yolo = true;
                    }
                }
                "response_item" => {
                    let role = payload.get("role").and_then(Value::as_str).unwrap_or("");
                    if role == "user" || role == "assistant" {
                        let prefix = if role == "user" { "» " } else { "  " };
                        let content = payload.get("content").and_then(Value::as_array);
                        let mut has_text = false;
                        if let Some(parts) = content {
                            for part in parts {
                                let text = part
                                    .get("text")
                                    .or_else(|| part.get("input_text"))
                                    .and_then(Value::as_str)
                                    .unwrap_or("");
                                if !text.is_empty()
                                    && !text.trim_start().starts_with("<environment_context>")
                                {
                                    messages.push(format!("{prefix}{text}"));
                                    has_text = true;
                                }
                            }
                        }
                        if has_text {
                            turn_count += 1;
                        }
                    }
                }
                "event_msg" => {
                    let event_type = payload.get("type").and_then(Value::as_str).unwrap_or("");
                    if event_type == "user_message" {
                        if let Some(msg) = payload.get("message").and_then(Value::as_str)
                            && !msg.is_empty() {
                                messages.push(format!("» {msg}"));
                                user_prompts.push(msg.to_string());
                            }
                    } else if event_type == "agent_reasoning"
                        && let Some(text) = payload.get("text").and_then(Value::as_str)
                            && !text.is_empty() {
                                messages.push(format!("  {text}"));
                            }
                }
                _ => {}
            }
        }

        if session_id.is_empty() {
            session_id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();
        }

        if user_prompts.is_empty() {
            return None;
        }

        let title = truncate_title(&user_prompts[0], 80);
        let full_content = messages.join("\n\n");
        let timestamp = DateTime::from_timestamp(mtime as i64, 0)?.naive_utc();

        Some(Session {
            id: session_id,
            agent: "codex".to_string(),
            title,
            directory,
            timestamp,
            content: full_content,
            message_count: turn_count,
            mtime,
            yolo,
        })
    }
}

impl AgentAdapter for CodexAdapter {
    fn name(&self) -> &str {
        "codex"
    }
    fn color(&self) -> &str {
        "#00A67E"
    }
    fn badge(&self) -> &str {
        "codex"
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
        let mut cmd = vec!["codex".to_string()];
        if yolo {
            cmd.push("--dangerously-bypass-approvals-and-sandbox".to_string());
        }
        cmd.push("resume".to_string());
        cmd.push(session.id.clone());
        cmd
    }

    fn get_raw_stats(&self) -> RawAdapterStats {
        let dir = &self.sessions_dir;
        let mut file_count = 0;
        let mut total_bytes = 0;
        if dir.is_dir() {
            for entry in walkdir::WalkDir::new(dir).into_iter().flatten() {
                if entry.path().extension().is_some_and(|e| e == "jsonl") {
                    file_count += 1;
                    total_bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
                }
            }
        }
        RawAdapterStats {
            agent: "codex".to_string(),
            data_dir: dir.display().to_string(),
            available: dir.is_dir(),
            file_count,
            total_bytes,
        }
    }
}
