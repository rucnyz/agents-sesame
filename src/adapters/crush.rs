use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use chrono::DateTime;
use serde_json::Value;

use crate::adapter::{AgentAdapter, ErrorCallback, SessionCallback};
use crate::session::{truncate_title, RawAdapterStats, Session};

pub struct CrushAdapter {
    projects_file: PathBuf,
}

impl CrushAdapter {
    pub fn new(projects_file: PathBuf) -> Self {
        Self { projects_file }
    }

    fn load_projects(&self) -> Vec<(String, PathBuf)> {
        let data = match fs::read(&self.projects_file) {
            Ok(d) => d,
            Err(_) => return vec![],
        };
        let val: Value = match serde_json::from_slice(&data) {
            Ok(v) => v,
            Err(_) => return vec![],
        };

        let mut projects = Vec::new();
        if let Some(arr) = val.get("projects").and_then(Value::as_array) {
            for p in arr {
                let path = p.get("path").and_then(Value::as_str).unwrap_or("");
                let data_dir = p.get("data_dir").and_then(Value::as_str).unwrap_or("");
                if !data_dir.is_empty() {
                    let db_path = PathBuf::from(data_dir).join("crush.db");
                    if db_path.exists() {
                        projects.push((path.to_string(), db_path));
                    }
                }
            }
        }
        projects
    }

    fn load_sessions_from_db(db_path: &PathBuf, project_path: &str) -> Vec<Session> {
        let mut sessions = Vec::new();

        let conn = match rusqlite::Connection::open_with_flags(
            db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) {
            Ok(c) => c,
            Err(_) => return sessions,
        };

        // Group by session: fetch session metadata + messages in one query
        let mut stmt = match conn.prepare(
            "SELECT s.id, s.title, s.message_count, s.updated_at, s.created_at,
                    m.role, m.parts, m.created_at as msg_created_at
             FROM sessions s
             LEFT JOIN messages m ON m.session_id = s.id
             WHERE s.message_count > 0
             ORDER BY s.updated_at DESC, m.created_at ASC",
        ) {
            Ok(s) => s,
            Err(_) => return sessions,
        };

        let mut session_data: HashMap<String, (String, i64, i64)> = HashMap::new();
        let mut session_messages: HashMap<String, Vec<(String, String)>> = HashMap::new();
        let mut session_order: Vec<String> = Vec::new();

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,          // id
                row.get::<_, Option<String>>(1)?,   // title
                row.get::<_, Option<i64>>(3)?,      // updated_at
                row.get::<_, Option<i64>>(4)?,      // created_at
                row.get::<_, Option<String>>(5)?,   // role
                row.get::<_, Option<String>>(6)?,   // parts
            ))
        });

        let Ok(rows) = rows else {
            return sessions;
        };

        for row in rows.flatten() {
            let (id, title, updated_at, created_at, role, parts) = row;

            if !session_data.contains_key(&id) {
                session_order.push(id.clone());
                session_data.insert(
                    id.clone(),
                    (
                        title.unwrap_or_default(),
                        updated_at.unwrap_or(0),
                        created_at.unwrap_or(0),
                    ),
                );
            }

            if let (Some(role), Some(parts)) = (role, parts) {
                session_messages
                    .entry(id)
                    .or_default()
                    .push((role, parts));
            }
        }

        // Build sessions
        for id in session_order {
            let Some((title, updated_at, _created_at)) = session_data.get(&id) else {
                continue;
            };
            let msgs = session_messages.get(&id);

            if let Some(session) = Self::build_session(
                &id,
                title,
                *updated_at,
                msgs.map(|v| v.as_slice()).unwrap_or(&[]),
                project_path,
            ) {
                sessions.push(session);
            }
        }

        sessions
    }

    fn build_session(
        id: &str,
        db_title: &str,
        updated_at: i64,
        messages_raw: &[(String, String)],
        project_path: &str,
    ) -> Option<Session> {
        // Detect milliseconds vs seconds
        let ts = if updated_at > 100_000_000_000 {
            updated_at / 1000
        } else {
            updated_at
        };
        let timestamp = DateTime::from_timestamp(ts, 0)?.naive_utc();

        let mut messages: Vec<String> = Vec::new();
        let mut first_user_message = String::new();

        for (role, parts_json) in messages_raw {
            let text = extract_text_from_parts(parts_json);
            if text.is_empty() {
                continue;
            }

            let prefix = if role == "user" { "» " } else { "  " };
            messages.push(format!("{prefix}{text}"));

            if role == "user" && first_user_message.is_empty() && text.len() > 5 {
                first_user_message = text;
            }
        }

        if messages.is_empty() || first_user_message.is_empty() {
            return None;
        }

        let title = if !db_title.is_empty() {
            db_title.to_string()
        } else {
            truncate_title(&first_user_message, 100)
        };

        let mtime = timestamp.and_utc().timestamp() as f64;
        let full_content = messages.join("\n\n");
        let message_count = messages.len();

        Some(Session {
            id: id.to_string(),
            agent: "crush".to_string(),
            title,
            directory: project_path.to_string(),
            timestamp,
            content: full_content,
            message_count,
            mtime,
            yolo: false,
        })
    }
}

fn extract_text_from_parts(parts_json: &str) -> String {
    let Ok(parts) = serde_json::from_str::<Value>(parts_json) else {
        return String::new();
    };
    let Some(arr) = parts.as_array() else {
        return String::new();
    };

    let mut text_parts = Vec::new();
    for part in arr {
        let part_type = part.get("type").and_then(Value::as_str).unwrap_or("");
        let data = part.get("data");

        match part_type {
            "text" => {
                if let Some(text) = data.and_then(|d| d.get("text")).and_then(Value::as_str)
                    && !text.is_empty() {
                        text_parts.push(text.to_string());
                    }
            }
            "tool_call" => {
                if let Some(name) = data.and_then(|d| d.get("name")).and_then(Value::as_str)
                    && !name.is_empty() {
                        text_parts.push(format!("[calling {name}]"));
                    }
            }
            "tool_result" => {
                if let Some(d) = data {
                    let content = d.get("content").and_then(Value::as_str).unwrap_or("");
                    let name = d.get("name").and_then(Value::as_str).unwrap_or("tool");
                    if !content.is_empty() && content.len() < 500 {
                        let truncated: String = content.chars().take(200).collect();
                        text_parts.push(format!("[{name}]: {truncated}"));
                    }
                }
            }
            _ => {}
        }
    }

    text_parts.join(" ")
}

impl AgentAdapter for CrushAdapter {
    fn name(&self) -> &str {
        "crush"
    }
    fn color(&self) -> &str {
        "#6B51FF"
    }
    fn badge(&self) -> &str {
        "crush"
    }

    fn is_available(&self) -> bool {
        self.projects_file.exists()
    }

    fn find_sessions(&self) -> Vec<Session> {
        if !self.is_available() {
            return vec![];
        }
        let mut all = Vec::new();
        for (project_path, db_path) in self.load_projects() {
            all.extend(Self::load_sessions_from_db(&db_path, &project_path));
        }
        all
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

        let mut new_or_modified = Vec::new();
        let mut all_current_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        for (project_path, db_path) in self.load_projects() {
            let project_sessions = Self::load_sessions_from_db(&db_path, &project_path);

            for mut session in project_sessions {
                all_current_ids.insert(session.id.clone());
                let session_mtime = session.timestamp.and_utc().timestamp() as f64;
                let known_entry = known.get(&session.id);
                if known_entry.is_none()
                    || session_mtime > known_entry.unwrap().0 + 0.001
                {
                    session.mtime = session_mtime;
                    if let Some(cb) = on_session {
                        cb(&session);
                    }
                    new_or_modified.push(session);
                }
            }
        }

        let deleted: Vec<String> = known
            .iter()
            .filter(|(_, (_, agent))| agent == self.name())
            .filter(|(id, _)| !all_current_ids.contains(*id))
            .map(|(id, _)| id.clone())
            .collect();

        (new_or_modified, deleted)
    }

    fn get_resume_command(&self, _session: &Session, _yolo: bool) -> Vec<String> {
        vec!["crush".to_string()]
    }

    fn get_raw_stats(&self) -> RawAdapterStats {
        let mut file_count = 0;
        let mut total_bytes = 0;
        for (_, db_path) in self.load_projects() {
            if let Ok(meta) = fs::metadata(&db_path) {
                file_count += 1;
                total_bytes += meta.len();
            }
        }
        RawAdapterStats {
            agent: "crush".to_string(),
            data_dir: self.projects_file.parent().map(|p| p.display().to_string()).unwrap_or_default(),
            available: self.is_available(),
            file_count,
            total_bytes,
        }
    }
}
