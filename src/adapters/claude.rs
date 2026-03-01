use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::DateTime;
use serde_json::Value;

use crate::adapter::{AgentAdapter, ErrorCallback, SessionCallback, incremental_scan};
use crate::session::{RawAdapterStats, Session, truncate_title};

pub struct ClaudeAdapter {
    sessions_dir: PathBuf,
}

impl ClaudeAdapter {
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
            let project_dir = entry.path();
            if !project_dir.is_dir() {
                continue;
            }

            let jsonl_entries = match fs::read_dir(&project_dir) {
                Ok(e) => e,
                Err(_) => continue,
            };

            for file_entry in jsonl_entries.flatten() {
                let path = file_entry.path();
                let Some(ext) = path.extension() else {
                    continue;
                };
                if ext != "jsonl" {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                // Skip agent subprocess files
                if stem.starts_with("agent-") {
                    continue;
                }
                let mtime = match file_entry.metadata() {
                    Ok(m) => m
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs_f64())
                        .unwrap_or(0.0),
                    Err(_) => continue,
                };
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

        let stem = path.file_stem()?.to_str()?.to_string();

        let mut first_user_message = String::new();
        let mut directory = String::new();
        let mut messages: Vec<String> = Vec::new();
        let mut turn_count: usize = 0;

        for line in data.split(|&b| b == b'\n') {
            if line.is_empty() {
                continue;
            }
            let Ok(val) = serde_json::from_slice::<Value>(line) else {
                continue;
            };

            let msg_type = val.get("type").and_then(Value::as_str).unwrap_or("");

            match msg_type {
                "user" => {
                    // Extract directory from first user message
                    if directory.is_empty()
                        && let Some(cwd) = val.get("cwd").and_then(Value::as_str)
                    {
                        directory = cwd.to_string();
                    }

                    let content = val.get("message").and_then(|m| m.get("content"));

                    let Some(content) = content else {
                        continue;
                    };

                    match content {
                        Value::String(text) => {
                            // Skip command messages
                            if text.starts_with("<command") || text.starts_with("<local-command") {
                                continue;
                            }
                            if !text.is_empty() {
                                messages.push(format!("» {text}"));
                                if first_user_message.is_empty() && text.chars().count() > 4 {
                                    first_user_message = text.clone();
                                }
                                turn_count += 1;
                            }
                        }
                        Value::Array(parts) => {
                            let mut is_human = false;
                            if let Some(first) = parts.first() {
                                let part_type =
                                    first.get("type").and_then(Value::as_str).unwrap_or("");
                                is_human = part_type == "text" || part_type.is_empty();
                            }

                            for part in parts {
                                let text = match part {
                                    Value::String(s) => Some(s.clone()),
                                    Value::Object(_) => {
                                        part.get("text").and_then(Value::as_str).map(String::from)
                                    }
                                    _ => None,
                                };
                                if let Some(text) = text
                                    && !text.is_empty()
                                {
                                    messages.push(format!("» {text}"));
                                    if first_user_message.is_empty() && text.chars().count() > 4 {
                                        first_user_message = text;
                                    }
                                }
                            }
                            if is_human {
                                turn_count += 1;
                            }
                        }
                        _ => {}
                    }
                }
                "assistant" => {
                    let content = val.get("message").and_then(|m| m.get("content"));

                    let Some(content) = content else {
                        continue;
                    };

                    let mut has_text = false;
                    match content {
                        Value::String(text) => {
                            if !text.is_empty() {
                                messages.push(format!("  {text}"));
                                has_text = true;
                            }
                        }
                        Value::Array(parts) => {
                            for part in parts {
                                let text = match part {
                                    Value::String(s) => Some(s.as_str()),
                                    Value::Object(_) => {
                                        let pt =
                                            part.get("type").and_then(Value::as_str).unwrap_or("");
                                        if pt == "text" {
                                            part.get("text").and_then(Value::as_str)
                                        } else {
                                            None
                                        }
                                    }
                                    _ => None,
                                };
                                if let Some(text) = text
                                    && !text.is_empty()
                                {
                                    messages.push(format!("  {text}"));
                                    has_text = true;
                                }
                            }
                        }
                        _ => {}
                    }
                    if has_text {
                        turn_count += 1;
                    }
                }
                _ => {}
            }
        }

        // Skip sessions with no user message or no content
        if first_user_message.is_empty() || messages.is_empty() {
            return None;
        }

        let title = truncate_title(&first_user_message, 100);
        let full_content = messages.join("\n\n");
        let timestamp = DateTime::from_timestamp(mtime as i64, 0)?.naive_utc();

        Some(Session {
            id: stem,
            agent: "claude".to_string(),
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

impl AgentAdapter for ClaudeAdapter {
    fn name(&self) -> &str {
        "claude"
    }

    fn color(&self) -> &str {
        "#E87B35"
    }

    fn badge(&self) -> &str {
        "claude"
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
        let mut cmd = vec!["claude".to_string()];
        if yolo {
            cmd.push("--dangerously-skip-permissions".to_string());
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
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "jsonl")
                    && !path
                        .file_stem()
                        .is_some_and(|s| s.to_str().is_some_and(|s| s.starts_with("agent-")))
                {
                    file_count += 1;
                    total_bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
                }
            }
        }

        RawAdapterStats {
            agent: "claude".to_string(),
            data_dir: dir.display().to_string(),
            available: dir.is_dir(),
            file_count,
            total_bytes,
        }
    }
}
