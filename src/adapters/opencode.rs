use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use chrono::DateTime;
use serde_json::Value;

use crate::adapter::{AgentAdapter, ErrorCallback, SessionCallback};
use crate::session::{RawAdapterStats, Session};

pub struct OpenCodeAdapter {
    db_path: PathBuf,
    legacy_dir: PathBuf,
}

impl OpenCodeAdapter {
    pub fn new(db_path: PathBuf, legacy_dir: PathBuf) -> Self {
        Self {
            db_path,
            legacy_dir,
        }
    }

    fn has_sqlite(&self) -> bool {
        self.db_path.exists()
    }

    fn has_legacy(&self) -> bool {
        self.legacy_dir.exists() && self.legacy_dir.join("session").exists()
    }

    fn load_sessions_from_db(&self) -> Vec<Session> {
        let mut sessions = Vec::new();

        let conn = match rusqlite::Connection::open_with_flags(
            &self.db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) {
            Ok(c) => c,
            Err(_) => return sessions,
        };

        // Fetch sessions
        let mut stmt = match conn.prepare(
            "SELECT id, title, directory, time_created, time_updated FROM session ORDER BY time_updated DESC",
        ) {
            Ok(s) => s,
            Err(_) => return sessions,
        };

        let session_rows: Vec<(String, String, String, i64, i64)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                    row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                ))
            })
            .ok()
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default();

        if session_rows.is_empty() {
            return sessions;
        }

        let session_ids: Vec<String> = session_rows.iter().map(|(id, ..)| id.clone()).collect();

        // Fetch messages (role via json_extract)
        let mut messages_by_session: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for chunk in session_ids.chunks(900) {
            let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT m.id, m.session_id, json_extract(m.data, '$.role')
                 FROM message m
                 WHERE m.session_id IN ({placeholders})
                 ORDER BY m.time_created ASC"
            );
            let mut stmt = match conn.prepare(&sql) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let params: Vec<&dyn rusqlite::types::ToSql> = chunk
                .iter()
                .map(|s| s as &dyn rusqlite::types::ToSql)
                .collect();
            if let Ok(rows) = stmt.query_map(params.as_slice(), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                ))
            }) {
                for row in rows.flatten() {
                    messages_by_session
                        .entry(row.1)
                        .or_default()
                        .push((row.0, row.2));
                }
            }
        }

        // Fetch text parts
        let mut parts_by_message: HashMap<String, Vec<String>> = HashMap::new();
        for chunk in session_ids.chunks(900) {
            let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT p.message_id, json_extract(p.data, '$.text')
                 FROM part p
                 WHERE p.session_id IN ({placeholders})
                   AND json_extract(p.data, '$.type') = 'text'
                 ORDER BY p.time_created ASC"
            );
            let mut stmt = match conn.prepare(&sql) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let params: Vec<&dyn rusqlite::types::ToSql> = chunk
                .iter()
                .map(|s| s as &dyn rusqlite::types::ToSql)
                .collect();
            if let Ok(rows) = stmt.query_map(params.as_slice(), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            }) {
                for row in rows.flatten() {
                    if let Some(text) = row.1
                        && !text.is_empty()
                    {
                        parts_by_message.entry(row.0).or_default().push(text);
                    }
                }
            }
        }

        // Build sessions
        for (id, title, directory, time_created, time_updated) in session_rows {
            let time_ms = time_created.max(time_updated);
            let timestamp = if time_ms > 0 {
                DateTime::from_timestamp(time_ms / 1000, 0).map(|dt| dt.naive_utc())
            } else {
                None
            };
            let Some(timestamp) = timestamp else {
                continue;
            };

            let title = if title.is_empty() {
                "Untitled session".to_string()
            } else {
                title
            };

            // Skip empty ACP sessions (auto-created with no content)
            if title.starts_with("ACP Session") {
                continue;
            }

            let mut messages: Vec<String> = Vec::new();
            let session_msgs = messages_by_session.get(&id).cloned().unwrap_or_default();
            for (msg_id, role) in &session_msgs {
                let prefix = if role == "user" { "» " } else { "  " };
                if let Some(parts) = parts_by_message.get(msg_id) {
                    for text in parts {
                        messages.push(format!("{prefix}{text}"));
                    }
                }
            }

            let full_content = messages.join("\n\n");
            let mtime = (time_ms as f64) / 1000.0;

            sessions.push(Session {
                id,
                agent: "opencode".to_string(),
                title,
                directory,
                timestamp,
                content: full_content,
                message_count: session_msgs.len(),
                mtime,
                yolo: false,
            });
        }

        sessions
    }

    /// Incremental loading from SQLite: only fetch messages/parts for changed sessions.
    fn load_sessions_incremental_db(
        &self,
        known: &HashMap<String, (f64, String)>,
        on_session: &SessionCallback,
    ) -> (Vec<Session>, Vec<String>) {
        let conn = match rusqlite::Connection::open_with_flags(
            &self.db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ) {
            Ok(c) => c,
            Err(_) => {
                let deleted: Vec<String> = known
                    .iter()
                    .filter(|(_, (_, a))| a == self.name())
                    .map(|(id, _)| id.clone())
                    .collect();
                return (vec![], deleted);
            }
        };

        // Step 1: lightweight query — only session metadata
        let mut stmt = match conn
            .prepare("SELECT id, title, directory, time_created, time_updated FROM session")
        {
            Ok(s) => s,
            Err(_) => return (vec![], vec![]),
        };

        let rows: Vec<(String, String, String, i64, i64)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                    row.get::<_, Option<i64>>(4)?.unwrap_or(0),
                ))
            })
            .ok()
            .map(|r| r.flatten().collect())
            .unwrap_or_default();

        // Step 2: diff against known — find which sessions changed
        let mut all_current_ids = std::collections::HashSet::new();
        let mut to_fetch: Vec<(String, String, String, i64, i64, f64)> = Vec::new();

        for (id, title, directory, time_created, time_updated) in &rows {
            all_current_ids.insert(id.clone());
            let time_ms = (*time_created).max(*time_updated);
            let mtime = (time_ms as f64) / 1000.0;

            let needs_fetch = match known.get(id.as_str()) {
                Some((known_mtime, _)) => mtime > *known_mtime + 0.001,
                None => true,
            };
            if needs_fetch {
                to_fetch.push((
                    id.clone(),
                    title.clone(),
                    directory.clone(),
                    *time_created,
                    *time_updated,
                    mtime,
                ));
            }
        }

        let deleted: Vec<String> = known
            .iter()
            .filter(|(_, (_, a))| a == self.name())
            .filter(|(id, _)| !all_current_ids.contains(*id))
            .map(|(id, _)| id.clone())
            .collect();

        if to_fetch.is_empty() {
            return (vec![], deleted);
        }

        // Step 3: only fetch messages/parts for changed sessions
        let fetch_ids: Vec<String> = to_fetch.iter().map(|(id, ..)| id.clone()).collect();

        let mut messages_by_session: HashMap<String, Vec<(String, String)>> = HashMap::new();
        for chunk in fetch_ids.chunks(900) {
            let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT m.id, m.session_id, json_extract(m.data, '$.role')
                 FROM message m
                 WHERE m.session_id IN ({placeholders})
                 ORDER BY m.time_created ASC"
            );
            let mut stmt = match conn.prepare(&sql) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let params: Vec<&dyn rusqlite::types::ToSql> = chunk
                .iter()
                .map(|s| s as &dyn rusqlite::types::ToSql)
                .collect();
            if let Ok(rows) = stmt.query_map(params.as_slice(), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                ))
            }) {
                for row in rows.flatten() {
                    messages_by_session
                        .entry(row.1)
                        .or_default()
                        .push((row.0, row.2));
                }
            }
        }

        let mut parts_by_message: HashMap<String, Vec<String>> = HashMap::new();
        for chunk in fetch_ids.chunks(900) {
            let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT p.message_id, json_extract(p.data, '$.text')
                 FROM part p
                 WHERE p.session_id IN ({placeholders})
                   AND json_extract(p.data, '$.type') = 'text'
                 ORDER BY p.time_created ASC"
            );
            let mut stmt = match conn.prepare(&sql) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let params: Vec<&dyn rusqlite::types::ToSql> = chunk
                .iter()
                .map(|s| s as &dyn rusqlite::types::ToSql)
                .collect();
            if let Ok(rows) = stmt.query_map(params.as_slice(), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            }) {
                for row in rows.flatten() {
                    if let Some(text) = row.1
                        && !text.is_empty()
                    {
                        parts_by_message.entry(row.0).or_default().push(text);
                    }
                }
            }
        }

        // Step 4: build sessions
        let mut new_or_modified = Vec::new();
        for (id, title, directory, time_created, time_updated, mtime) in to_fetch {
            let time_ms = time_created.max(time_updated);
            let timestamp = if time_ms > 0 {
                DateTime::from_timestamp(time_ms / 1000, 0).map(|dt| dt.naive_utc())
            } else {
                None
            };
            let Some(timestamp) = timestamp else {
                continue;
            };

            let title = if title.is_empty() {
                "Untitled session".to_string()
            } else {
                title
            };

            // Skip empty ACP sessions (auto-created with no content)
            if title.starts_with("ACP Session") {
                continue;
            }

            let mut messages: Vec<String> = Vec::new();
            let session_msgs = messages_by_session.get(&id).cloned().unwrap_or_default();
            for (msg_id, role) in &session_msgs {
                let prefix = if role == "user" { "» " } else { "  " };
                if let Some(parts) = parts_by_message.get(msg_id) {
                    for text in parts {
                        messages.push(format!("{prefix}{text}"));
                    }
                }
            }

            let full_content = messages.join("\n\n");

            let session = Session {
                id,
                agent: "opencode".to_string(),
                title,
                directory,
                timestamp,
                content: full_content,
                message_count: session_msgs.len(),
                mtime,
                yolo: false,
            };
            if let Some(cb) = on_session {
                cb(&session);
            }
            new_or_modified.push(session);
        }

        (new_or_modified, deleted)
    }

    fn load_sessions_legacy(&self) -> Vec<Session> {
        let session_dir = self.legacy_dir.join("session");
        let message_dir = self.legacy_dir.join("message");
        let part_dir = self.legacy_dir.join("part");

        if !session_dir.exists() {
            return vec![];
        }

        // Pre-index messages by session_id
        let mut messages_by_session: HashMap<String, Vec<(String, String)>> = HashMap::new();
        if message_dir.exists() {
            for entry in walkdir::WalkDir::new(&message_dir).into_iter().flatten() {
                let path = entry.path();
                if path.extension().is_none_or(|e| e != "json") {
                    continue;
                }
                let Some(fname) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if !fname.starts_with("msg_") {
                    continue;
                }
                if let Ok(data) = fs::read(path)
                    && let Ok(val) = serde_json::from_slice::<Value>(&data)
                {
                    let session_id = path
                        .parent()
                        .and_then(|p| p.file_name())
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .to_string();
                    let msg_id = val
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let role = val
                        .get("role")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if !msg_id.is_empty() {
                        messages_by_session
                            .entry(session_id)
                            .or_default()
                            .push((msg_id, role));
                    }
                }
            }
        }

        // Pre-index parts by message_id
        let mut parts_by_message: HashMap<String, Vec<String>> = HashMap::new();
        if part_dir.exists() {
            for entry in walkdir::WalkDir::new(&part_dir).into_iter().flatten() {
                let path = entry.path();
                if path.extension().is_none_or(|e| e != "json") {
                    continue;
                }
                if let Ok(data) = fs::read(path)
                    && let Ok(val) = serde_json::from_slice::<Value>(&data)
                    && val.get("type").and_then(Value::as_str) == Some("text")
                {
                    let msg_id = path
                        .parent()
                        .and_then(|p| p.file_name())
                        .and_then(|n| n.to_str())
                        .unwrap_or("")
                        .to_string();
                    let text = val
                        .get("text")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if !text.is_empty() {
                        parts_by_message.entry(msg_id).or_default().push(text);
                    }
                }
            }
        }

        let mut sessions = Vec::new();
        if let Ok(project_entries) = fs::read_dir(&session_dir) {
            for project_entry in project_entries.flatten() {
                if !project_entry.path().is_dir() {
                    continue;
                }
                if let Ok(ses_entries) = fs::read_dir(project_entry.path()) {
                    for ses_entry in ses_entries.flatten() {
                        let path = ses_entry.path();
                        let Some(fname) = path.file_name().and_then(|n| n.to_str()) else {
                            continue;
                        };
                        if !fname.starts_with("ses_") || !fname.ends_with(".json") {
                            continue;
                        }
                        if let Some(session) = Self::parse_legacy_session(
                            &path,
                            &messages_by_session,
                            &parts_by_message,
                        ) {
                            sessions.push(session);
                        }
                    }
                }
            }
        }

        sessions
    }

    fn parse_legacy_session(
        path: &std::path::Path,
        messages_by_session: &HashMap<String, Vec<(String, String)>>,
        parts_by_message: &HashMap<String, Vec<String>>,
    ) -> Option<Session> {
        let data = fs::read(path).ok()?;
        let val: Value = serde_json::from_slice(&data).ok()?;

        let session_id = val.get("id").and_then(Value::as_str)?.to_string();
        let title = val
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Untitled session")
            .to_string();

        // Skip empty ACP sessions (auto-created with no content)
        if title.starts_with("ACP Session") {
            return None;
        }

        let directory = val
            .get("directory")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        let time_data = val.get("time");
        let created = time_data
            .and_then(|t| t.get("created"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let updated = time_data
            .and_then(|t| t.get("updated"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let time_ms = created.max(updated);

        let timestamp = if time_ms > 0 {
            DateTime::from_timestamp(time_ms / 1000, 0)?.naive_utc()
        } else {
            let mtime = fs::metadata(path)
                .ok()?
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_secs_f64();
            DateTime::from_timestamp(mtime as i64, 0)?.naive_utc()
        };

        let mut messages: Vec<String> = Vec::new();
        let session_msgs = messages_by_session
            .get(&session_id)
            .cloned()
            .unwrap_or_default();
        for (msg_id, role) in &session_msgs {
            let prefix = if role == "user" { "» " } else { "  " };
            if let Some(parts) = parts_by_message.get(msg_id) {
                for text in parts {
                    messages.push(format!("{prefix}{text}"));
                }
            }
        }

        let full_content = messages.join("\n\n");
        let mtime = (time_ms as f64) / 1000.0;

        Some(Session {
            id: session_id,
            agent: "opencode".to_string(),
            title,
            directory,
            timestamp,
            content: full_content,
            message_count: session_msgs.len(),
            mtime,
            yolo: false,
        })
    }
}

impl AgentAdapter for OpenCodeAdapter {
    fn name(&self) -> &str {
        "opencode"
    }
    fn color(&self) -> &str {
        "#CFCECD"
    }
    fn badge(&self) -> &str {
        "opencode"
    }

    fn is_available(&self) -> bool {
        self.has_sqlite() || self.has_legacy()
    }

    fn find_sessions(&self) -> Vec<Session> {
        if !self.is_available() {
            return vec![];
        }
        if self.has_sqlite() {
            return self.load_sessions_from_db();
        }
        self.load_sessions_legacy()
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

        if self.has_sqlite() {
            return self.load_sessions_incremental_db(known, on_session);
        }

        // Legacy: reload all and diff
        let all_sessions = self.load_sessions_legacy();
        let mut all_current_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        let mut new_or_modified = Vec::new();

        for session in all_sessions {
            all_current_ids.insert(session.id.clone());
            let known_entry = known.get(&session.id);
            if known_entry.is_none() || session.mtime > known_entry.unwrap().0 + 0.001 {
                if let Some(cb) = on_session {
                    cb(&session);
                }
                new_or_modified.push(session);
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

    fn get_resume_command(&self, session: &Session, _yolo: bool) -> Vec<String> {
        vec![
            "opencode".to_string(),
            session.directory.clone(),
            "--session".to_string(),
            session.id.clone(),
        ]
    }

    fn get_raw_stats(&self) -> RawAdapterStats {
        let mut file_count = 0;
        let mut total_bytes = 0;
        if self.has_sqlite()
            && let Ok(meta) = fs::metadata(&self.db_path)
        {
            file_count += 1;
            total_bytes += meta.len();
        }
        RawAdapterStats {
            agent: "opencode".to_string(),
            data_dir: self
                .db_path
                .parent()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            available: self.is_available(),
            file_count,
            total_bytes,
        }
    }
}
