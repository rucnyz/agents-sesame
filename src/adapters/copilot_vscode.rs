use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::DateTime;
use serde_json::Value;

use crate::adapter::{AgentAdapter, ErrorCallback, SessionCallback};
use crate::session::{RawAdapterStats, Session, truncate_title};

fn vscode_storage_dir() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_default();
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Code")
    } else if cfg!(target_os = "windows") {
        home.join("AppData/Roaming/Code")
    } else {
        home.join(".config/Code")
    }
}

pub struct CopilotVSCodeAdapter {
    chat_sessions_dir: PathBuf,
    workspace_storage_dir: PathBuf,
}

impl CopilotVSCodeAdapter {
    pub fn new(chat_sessions_dir: PathBuf, workspace_storage_dir: PathBuf) -> Self {
        Self {
            chat_sessions_dir,
            workspace_storage_dir,
        }
    }

    pub fn default_chat_dir() -> PathBuf {
        vscode_storage_dir().join("User/globalStorage/emptyWindowChatSessions")
    }

    pub fn default_workspace_dir() -> PathBuf {
        vscode_storage_dir().join("User/workspaceStorage")
    }

    fn get_all_session_files(&self) -> Vec<(PathBuf, String)> {
        let mut session_files: Vec<(PathBuf, String)> = Vec::new();

        // Empty window sessions
        if self.chat_sessions_dir.is_dir()
            && let Ok(entries) = fs::read_dir(&self.chat_sessions_dir)
        {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "json") {
                    session_files.push((path, String::new()));
                }
            }
        }

        // Workspace-specific sessions
        if self.workspace_storage_dir.is_dir()
            && let Ok(ws_entries) = fs::read_dir(&self.workspace_storage_dir)
        {
            for ws_entry in ws_entries.flatten() {
                let ws_dir = ws_entry.path();
                if !ws_dir.is_dir() {
                    continue;
                }
                let chat_dir = ws_dir.join("chatSessions");
                if !chat_dir.is_dir() {
                    continue;
                }
                let ws_directory = Self::get_workspace_directory(&ws_dir);
                if let Ok(entries) = fs::read_dir(&chat_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().is_some_and(|e| e == "json") {
                            session_files.push((path, ws_directory.clone()));
                        }
                    }
                }
            }
        }

        session_files
    }

    fn get_workspace_directory(workspace_dir: &Path) -> String {
        let workspace_json = workspace_dir.join("workspace.json");
        if let Ok(data) = fs::read(&workspace_json)
            && let Ok(val) = serde_json::from_slice::<Value>(&data)
            && let Some(folder) = val.get("folder").and_then(Value::as_str)
            && let Some(path) = folder.strip_prefix("file://")
        {
            // URL-decode the path
            return urldecode(path);
        }
        String::new()
    }

    fn get_session_id_from_file(path: &Path) -> Option<String> {
        let data = fs::read(path).ok()?;
        let val: Value = serde_json::from_slice(&data).ok()?;
        let id = val
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| path.file_stem().and_then(|s| s.to_str()).unwrap_or(""));
        Some(id.to_string())
    }

    fn parse_session(path: &Path, workspace_directory: &str) -> Option<Session> {
        let data = fs::read(path).ok()?;
        let mtime = fs::metadata(path)
            .ok()?
            .modified()
            .ok()?
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs_f64();

        let val: Value = serde_json::from_slice(&data).ok()?;

        let session_id = val
            .get("sessionId")
            .and_then(Value::as_str)
            .unwrap_or_else(|| path.file_stem().and_then(|s| s.to_str()).unwrap_or(""))
            .to_string();

        let custom_title = val
            .get("customTitle")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        let requests = val.get("requests").and_then(Value::as_array)?;
        if requests.is_empty() {
            return None;
        }

        let mut messages: Vec<String> = Vec::new();
        let mut directory = workspace_directory.to_string();
        let mut turn_count: usize = 0;

        for req in requests {
            // User message
            let user_text = req
                .get("message")
                .and_then(|m| m.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if !user_text.is_empty() {
                messages.push(format!("» {user_text}"));
                turn_count += 1;
            }

            // Extract directory from content references
            if directory.is_empty()
                && let Some(refs) = req.get("contentReferences").and_then(Value::as_array)
            {
                for r in refs {
                    let fs_path = r
                        .get("reference")
                        .and_then(|rf| rf.get("uri"))
                        .and_then(|u| u.get("fsPath"))
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if !fs_path.is_empty()
                        && let Some(parent) = Path::new(fs_path).parent()
                    {
                        directory = parent.display().to_string();
                        break;
                    }
                }
            }

            // Assistant response
            if let Some(response) = req.get("response").and_then(Value::as_array) {
                let mut has_response = false;
                for part in response {
                    let value = part.get("value").and_then(Value::as_str).unwrap_or("");
                    if !value.is_empty() {
                        messages.push(format!("  {value}"));
                        has_response = true;
                    }
                }
                if has_response {
                    turn_count += 1;
                }
            }
        }

        if messages.is_empty() {
            return None;
        }

        let title = if !custom_title.is_empty() {
            custom_title
        } else {
            let first = messages[0].trim_start_matches("» ").trim();
            truncate_title(first, 100)
        };

        // Timestamp: prefer lastMessageDate, then creationDate, then file mtime
        let ts_ms = val
            .get("lastMessageDate")
            .or_else(|| val.get("creationDate"))
            .and_then(Value::as_f64);

        let timestamp = if let Some(ms) = ts_ms {
            DateTime::from_timestamp((ms / 1000.0) as i64, 0)?.naive_utc()
        } else {
            DateTime::from_timestamp(mtime as i64, 0)?.naive_utc()
        };

        let full_content = messages.join("\n\n");

        Some(Session {
            id: session_id,
            agent: "copilot-vscode".to_string(),
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

impl AgentAdapter for CopilotVSCodeAdapter {
    fn name(&self) -> &str {
        "copilot-vscode"
    }
    fn color(&self) -> &str {
        "#007ACC"
    }
    fn badge(&self) -> &str {
        "vscode"
    }

    fn is_available(&self) -> bool {
        if self.chat_sessions_dir.is_dir()
            && let Ok(mut entries) = fs::read_dir(&self.chat_sessions_dir)
            && entries.any(|e| {
                e.ok()
                    .is_some_and(|e| e.path().extension().is_some_and(|x| x == "json"))
            })
        {
            return true;
        }
        if self.workspace_storage_dir.is_dir()
            && let Ok(ws_entries) = fs::read_dir(&self.workspace_storage_dir)
        {
            for ws_entry in ws_entries.flatten() {
                let chat_dir = ws_entry.path().join("chatSessions");
                if chat_dir.is_dir()
                    && let Ok(mut entries) = fs::read_dir(&chat_dir)
                    && entries.any(|e| {
                        e.ok()
                            .is_some_and(|e| e.path().extension().is_some_and(|x| x == "json"))
                    })
                {
                    return true;
                }
            }
        }
        false
    }

    fn find_sessions(&self) -> Vec<Session> {
        if !self.is_available() {
            return vec![];
        }
        self.get_all_session_files()
            .iter()
            .filter_map(|(path, ws_dir)| Self::parse_session(path, ws_dir))
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
                .filter(|(_, (_, agent))| agent == self.name())
                .map(|(id, _)| id.clone())
                .collect();
            return (vec![], deleted);
        }

        let mut current_files: HashMap<String, (PathBuf, f64, String)> = HashMap::new();

        for (path, ws_dir) in self.get_all_session_files() {
            if let Some(session_id) = Self::get_session_id_from_file(&path) {
                let mtime = fs::metadata(&path)
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs_f64())
                    .unwrap_or(0.0);
                current_files.insert(session_id, (path, mtime, ws_dir));
            }
        }

        let mut new_or_modified = Vec::new();
        for (session_id, (path, mtime, ws_dir)) in &current_files {
            let needs_parse = match known.get(session_id) {
                Some((known_mtime, _)) => *mtime > *known_mtime + 0.001,
                None => true,
            };
            if needs_parse && let Some(session) = Self::parse_session(path, ws_dir) {
                if let Some(cb) = on_session {
                    cb(&session);
                }
                new_or_modified.push(session);
            }
        }

        let deleted: Vec<String> = known
            .iter()
            .filter(|(_, (_, agent))| agent == self.name())
            .filter(|(id, _)| !current_files.contains_key(*id))
            .map(|(id, _)| id.clone())
            .collect();

        (new_or_modified, deleted)
    }

    fn get_resume_command(&self, session: &Session, _yolo: bool) -> Vec<String> {
        if !session.directory.is_empty() {
            vec!["code".to_string(), session.directory.clone()]
        } else {
            vec!["code".to_string()]
        }
    }

    fn get_raw_stats(&self) -> RawAdapterStats {
        let files = self.get_all_session_files();
        let total_bytes: u64 = files
            .iter()
            .filter_map(|(p, _)| fs::metadata(p).ok().map(|m| m.len()))
            .sum();
        RawAdapterStats {
            agent: "copilot-vscode".to_string(),
            data_dir: self.chat_sessions_dir.display().to_string(),
            available: self.is_available(),
            file_count: files.len(),
            total_bytes,
        }
    }
}

/// Simple URL-decode for file:// paths
fn urldecode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.bytes();
    while let Some(b) = chars.next() {
        if b == b'%' {
            let h1 = chars.next().unwrap_or(b'0');
            let h2 = chars.next().unwrap_or(b'0');
            let hex = [h1, h2];
            if let Ok(decoded) = u8::from_str_radix(std::str::from_utf8(&hex).unwrap_or("00"), 16) {
                result.push(decoded as char);
            }
        } else {
            result.push(b as char);
        }
    }
    result
}
