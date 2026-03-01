use std::fs;
use std::path::PathBuf;

use rust_resume::adapter::AgentAdapter;
use rust_resume::adapters::*;
use tempfile::TempDir;

// ── Claude ───────────────────────────────────────────────────────────────

#[test]
fn test_claude_find_sessions() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project-abc");
    fs::create_dir_all(&project).unwrap();

    let jsonl = r#"{"type":"user","cwd":"/home/user/project","message":{"content":"Hello from test session"}}
{"type":"assistant","message":{"content":"I can help you with that."}}
{"type":"user","message":{"content":"Thanks for helping"}}
"#;
    fs::write(project.join("test-session-id.jsonl"), jsonl).unwrap();

    let adapter = ClaudeAdapter::new(tmp.path().to_path_buf());
    assert!(adapter.is_available());
    assert_eq!(adapter.name(), "claude");
    assert_eq!(adapter.badge(), "claude");

    let sessions = adapter.find_sessions();
    assert_eq!(sessions.len(), 1);

    let s = &sessions[0];
    assert_eq!(s.id, "test-session-id");
    assert_eq!(s.agent, "claude");
    assert_eq!(s.directory, "/home/user/project");
    assert!(s.title.contains("Hello from test session"));
    assert!(s.message_count >= 2);
}

#[test]
fn test_claude_skips_agent_files() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("project-abc");
    fs::create_dir_all(&project).unwrap();

    // Regular session
    let jsonl = r#"{"type":"user","cwd":"/tmp","message":{"content":"Regular session content here"}}
{"type":"assistant","message":{"content":"Response here."}}
"#;
    fs::write(project.join("regular-session.jsonl"), jsonl).unwrap();

    // Agent subprocess file (should be skipped)
    let agent_jsonl = r#"{"type":"user","message":{"content":"Agent subprocess"}}
"#;
    fs::write(project.join("agent-subprocess-123.jsonl"), agent_jsonl).unwrap();

    let adapter = ClaudeAdapter::new(tmp.path().to_path_buf());
    let sessions = adapter.find_sessions();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, "regular-session");
}

#[test]
fn test_claude_resume_command() {
    let adapter = ClaudeAdapter::new(PathBuf::from("/nonexistent"));
    let session = make_session("ses-123", "claude");
    assert_eq!(
        adapter.get_resume_command(&session, false),
        vec!["claude", "--resume", "ses-123"]
    );
    assert_eq!(
        adapter.get_resume_command(&session, true),
        vec!["claude", "--dangerously-skip-permissions", "--resume", "ses-123"]
    );
}

// ── Codex ────────────────────────────────────────────────────────────────

#[test]
fn test_codex_find_sessions() {
    let tmp = TempDir::new().unwrap();
    let session_dir = tmp.path().join("2025-01-15");
    fs::create_dir_all(&session_dir).unwrap();

    let jsonl = r#"{"type":"session_meta","payload":{"id":"codex-test-id","cwd":"/home/user/project"}}
{"type":"turn_context","payload":{"approval_policy":"always"}}
{"type":"event_msg","payload":{"type":"user_message","message":"Fix the bug in main.rs"}}
{"type":"response_item","payload":{"role":"assistant","content":[{"text":"I'll fix that for you."}]}}
"#;
    fs::write(session_dir.join("session.jsonl"), jsonl).unwrap();

    let adapter = CodexAdapter::new(tmp.path().to_path_buf());
    assert!(adapter.is_available());

    let sessions = adapter.find_sessions();
    assert_eq!(sessions.len(), 1);

    let s = &sessions[0];
    assert_eq!(s.id, "codex-test-id");
    assert_eq!(s.agent, "codex");
    assert_eq!(s.directory, "/home/user/project");
    assert!(!s.yolo);
}

#[test]
fn test_codex_yolo_detection() {
    let tmp = TempDir::new().unwrap();

    let jsonl = r#"{"type":"session_meta","payload":{"id":"yolo-session","cwd":"/tmp"}}
{"type":"turn_context","payload":{"approval_policy":"never"}}
{"type":"event_msg","payload":{"type":"user_message","message":"Do something dangerous here"}}
"#;
    fs::write(tmp.path().join("yolo.jsonl"), jsonl).unwrap();

    let adapter = CodexAdapter::new(tmp.path().to_path_buf());
    let sessions = adapter.find_sessions();
    assert_eq!(sessions.len(), 1);
    assert!(sessions[0].yolo);
}

// ── Copilot CLI ──────────────────────────────────────────────────────────

#[test]
fn test_copilot_find_sessions() {
    let tmp = TempDir::new().unwrap();

    let jsonl = r#"{"type":"session.start","data":{"sessionId":"cop-session-123"}}
{"type":"session.info","data":{"infoType":"folder_trust","message":"Folder /home/user/project is trusted"}}
{"type":"user.message","data":{"content":"Help me refactor this function please"}}
{"type":"assistant.message","data":{"content":"Sure, here's the refactored version."}}
"#;
    fs::write(tmp.path().join("session-state.jsonl"), jsonl).unwrap();

    let adapter = CopilotAdapter::new(tmp.path().to_path_buf());
    assert!(adapter.is_available());

    let sessions = adapter.find_sessions();
    assert_eq!(sessions.len(), 1);

    let s = &sessions[0];
    assert_eq!(s.id, "cop-session-123");
    assert_eq!(s.agent, "copilot-cli");
    assert_eq!(s.directory, "/home/user/project");
    assert_eq!(s.message_count, 2);
}

// ── Copilot VSCode ───────────────────────────────────────────────────────

#[test]
fn test_copilot_vscode_find_sessions() {
    let tmp = TempDir::new().unwrap();
    let chat_dir = tmp.path().join("emptyWindowChatSessions");
    fs::create_dir_all(&chat_dir).unwrap();

    let session_json = r#"{
        "sessionId": "vscode-ses-abc",
        "customTitle": "My Copilot Chat",
        "creationDate": 1700000000000,
        "requests": [
            {
                "message": {"text": "How do I sort a vector in Rust"},
                "response": [{"value": "You can use .sort() method."}]
            }
        ]
    }"#;
    fs::write(chat_dir.join("session.json"), session_json).unwrap();

    let ws_dir = tmp.path().join("workspaceStorage");
    fs::create_dir_all(&ws_dir).unwrap();

    let adapter = CopilotVSCodeAdapter::new(chat_dir, ws_dir);
    assert!(adapter.is_available());

    let sessions = adapter.find_sessions();
    assert_eq!(sessions.len(), 1);

    let s = &sessions[0];
    assert_eq!(s.id, "vscode-ses-abc");
    assert_eq!(s.agent, "copilot-vscode");
    assert_eq!(s.title, "My Copilot Chat");
    assert_eq!(s.message_count, 2);
}

// ── Vibe ─────────────────────────────────────────────────────────────────

#[test]
fn test_vibe_find_sessions() {
    let tmp = TempDir::new().unwrap();
    let session_dir = tmp.path().join("session_test123");
    fs::create_dir_all(&session_dir).unwrap();

    let meta = r#"{
        "session_id": "vibe-ses-001",
        "title": "Vibe Test Session",
        "environment": {"working_directory": "/home/user/vibe-project"},
        "start_time": "2025-01-15T10:30:00Z",
        "config": {"auto_approve": true}
    }"#;
    fs::write(session_dir.join("meta.json"), meta).unwrap();

    let messages = r#"{"role":"user","content":"Build a web server"}
{"role":"assistant","content":"I'll create an HTTP server for you."}
"#;
    fs::write(session_dir.join("messages.jsonl"), messages).unwrap();

    let adapter = VibeAdapter::new(tmp.path().to_path_buf());
    assert!(adapter.is_available());

    let sessions = adapter.find_sessions();
    assert_eq!(sessions.len(), 1);

    let s = &sessions[0];
    assert_eq!(s.id, "vibe-ses-001");
    assert_eq!(s.agent, "vibe");
    assert_eq!(s.title, "Vibe Test Session");
    assert_eq!(s.directory, "/home/user/vibe-project");
    assert!(s.yolo);
}

// ── Crush ────────────────────────────────────────────────────────────────

#[test]
fn test_crush_find_sessions() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("crush.db");

    // Create SQLite DB
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE sessions (id TEXT PRIMARY KEY, title TEXT, message_count INTEGER, updated_at INTEGER, created_at INTEGER);
         CREATE TABLE messages (id TEXT, session_id TEXT, role TEXT, parts TEXT, created_at INTEGER);
         INSERT INTO sessions VALUES ('crush-ses-1', 'Test Crush Session', 2, 1700000000, 1700000000);
         INSERT INTO messages VALUES ('msg-1', 'crush-ses-1', 'user', '[{\"type\":\"text\",\"data\":{\"text\":\"Explain this Rust code to me please\"}}]', 1700000000);
         INSERT INTO messages VALUES ('msg-2', 'crush-ses-1', 'assistant', '[{\"type\":\"text\",\"data\":{\"text\":\"This code does...\"}}]', 1700000001);",
    ).unwrap();
    drop(conn);

    let projects_json = serde_json::json!({
        "projects": [{
            "path": "/home/user/crush-project",
            "data_dir": tmp.path().to_str().unwrap()
        }]
    });
    let projects_file = tmp.path().join("projects.json");
    fs::write(&projects_file, projects_json.to_string()).unwrap();

    let adapter = CrushAdapter::new(projects_file);
    assert!(adapter.is_available());

    let sessions = adapter.find_sessions();
    assert_eq!(sessions.len(), 1);

    let s = &sessions[0];
    assert_eq!(s.id, "crush-ses-1");
    assert_eq!(s.agent, "crush");
    assert_eq!(s.title, "Test Crush Session");
    assert_eq!(s.directory, "/home/user/crush-project");
}

// ── OpenCode ─────────────────────────────────────────────────────────────

#[test]
fn test_opencode_find_sessions() {
    let tmp = TempDir::new().unwrap();
    let db_path = tmp.path().join("opencode.db");

    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE session (id TEXT PRIMARY KEY, title TEXT, directory TEXT, time_created INTEGER, time_updated INTEGER);
         CREATE TABLE message (id TEXT, session_id TEXT, data TEXT, time_created INTEGER);
         CREATE TABLE part (id TEXT, message_id TEXT, session_id TEXT, data TEXT, time_created INTEGER);
         INSERT INTO session VALUES ('oc-ses-1', 'OpenCode Session', '/home/user/oc-project', 1700000000000, 1700000000000);
         INSERT INTO message VALUES ('msg-1', 'oc-ses-1', '{\"role\":\"user\"}', 1700000000000);
         INSERT INTO part VALUES ('part-1', 'msg-1', 'oc-ses-1', '{\"type\":\"text\",\"text\":\"Help me debug this code issue\"}', 1700000000000);
         INSERT INTO message VALUES ('msg-2', 'oc-ses-1', '{\"role\":\"assistant\"}', 1700000001000);
         INSERT INTO part VALUES ('part-2', 'msg-2', 'oc-ses-1', '{\"type\":\"text\",\"text\":\"The issue is in your loop.\"}', 1700000001000);",
    ).unwrap();
    drop(conn);

    let legacy_dir = tmp.path().join("legacy");
    fs::create_dir_all(&legacy_dir).unwrap();

    let adapter = OpenCodeAdapter::new(db_path, legacy_dir);
    assert!(adapter.is_available());

    let sessions = adapter.find_sessions();
    assert_eq!(sessions.len(), 1);

    let s = &sessions[0];
    assert_eq!(s.id, "oc-ses-1");
    assert_eq!(s.agent, "opencode");
    assert_eq!(s.title, "OpenCode Session");
    assert_eq!(s.directory, "/home/user/oc-project");
}

// ── Qwen ─────────────────────────────────────────────────────────────────

#[test]
fn test_qwen_find_sessions() {
    let tmp = TempDir::new().unwrap();
    let chats_dir = tmp.path().join("abc123hash").join("chats");
    fs::create_dir_all(&chats_dir).unwrap();

    let jsonl = r#"{"sessionId":"qwen-ses-1","cwd":"/home/user/qwen-project","timestamp":"2025-01-15T10:00:00Z","type":"user","message":{"parts":[{"text":"Write a hello world in Rust please"}]}}
{"type":"assistant","message":{"parts":[{"text":"Here's a simple hello world program."}]}}
"#;
    // UUID-like filename (32+ hex chars with hyphens)
    fs::write(
        chats_dir.join("a1b2c3d4-e5f6-7890-abcd-ef1234567890.jsonl"),
        jsonl,
    )
    .unwrap();

    let adapter = QwenAdapter::new(tmp.path().to_path_buf());
    assert!(adapter.is_available());

    let sessions = adapter.find_sessions();
    assert_eq!(sessions.len(), 1);

    let s = &sessions[0];
    assert_eq!(s.agent, "qwen");
    assert_eq!(s.directory, "/home/user/qwen-project");
    assert!(s.title.contains("hello world"));
}

// ── Gemini ───────────────────────────────────────────────────────────────

#[test]
fn test_gemini_find_sessions() {
    let tmp = TempDir::new().unwrap();
    let chats_dir = tmp.path().join("project-hash-123").join("chats");
    fs::create_dir_all(&chats_dir).unwrap();

    let session_json = r#"{
        "sessionId": "gemini-ses-1",
        "kind": "main",
        "startTime": "2025-01-15T10:00:00Z",
        "lastUpdated": "2025-01-15T11:00:00Z",
        "directories": ["/home/user/gemini-project"],
        "messages": [
            {"type": "user", "content": [{"text": "Explain async/await in Rust for me"}]},
            {"type": "gemini", "content": [{"text": "Async/await in Rust is a way to write concurrent code."}]}
        ]
    }"#;
    fs::write(
        chats_dir.join("session-2025-01-15T10-00-abc12345.json"),
        session_json,
    )
    .unwrap();

    let adapter = GeminiAdapter::new(tmp.path().to_path_buf());
    assert!(adapter.is_available());

    let sessions = adapter.find_sessions();
    assert_eq!(sessions.len(), 1);

    let s = &sessions[0];
    assert_eq!(s.id, "gemini-ses-1");
    assert_eq!(s.agent, "gemini");
    assert_eq!(s.directory, "/home/user/gemini-project");
    assert!(s.title.contains("async/await"));
}

#[test]
fn test_gemini_skips_subagent() {
    let tmp = TempDir::new().unwrap();
    let chats_dir = tmp.path().join("project-1").join("chats");
    fs::create_dir_all(&chats_dir).unwrap();

    let session_json = r#"{
        "sessionId": "sub-agent-1",
        "kind": "subagent",
        "messages": [
            {"type": "user", "content": [{"text": "subagent task content here"}]}
        ]
    }"#;
    fs::write(
        chats_dir.join("session-2025-01-15T10-00-sub12345.json"),
        session_json,
    )
    .unwrap();

    let adapter = GeminiAdapter::new(tmp.path().to_path_buf());
    let sessions = adapter.find_sessions();
    assert_eq!(sessions.len(), 0);
}

// ── Kimi ─────────────────────────────────────────────────────────────────

#[test]
fn test_kimi_find_sessions() {
    let tmp = TempDir::new().unwrap();
    let session_dir = tmp.path().join("workhash123").join("session-abc");
    fs::create_dir_all(&session_dir).unwrap();

    let context = r#"{"role":"user","content":"Help me write a parser in Rust","timestamp":"2025-01-15T10:00:00Z"}
{"role":"assistant","content":"I'll help you build a parser."}
"#;
    fs::write(session_dir.join("context.jsonl"), context).unwrap();

    let state = r#"{"work_dir": "/home/user/kimi-project"}"#;
    fs::write(session_dir.join("state.json"), state).unwrap();

    let adapter = KimiAdapter::new(tmp.path().to_path_buf());
    assert!(adapter.is_available());

    let sessions = adapter.find_sessions();
    assert_eq!(sessions.len(), 1);

    let s = &sessions[0];
    assert_eq!(s.id, "session-abc");
    assert_eq!(s.agent, "kimi");
    assert_eq!(s.directory, "/home/user/kimi-project");
    assert!(s.title.contains("parser"));
}

// ── Unavailable adapters ─────────────────────────────────────────────────

#[test]
fn test_unavailable_returns_empty() {
    let nonexistent = PathBuf::from("/nonexistent/path/that/does/not/exist");

    let claude = ClaudeAdapter::new(nonexistent.clone());
    assert!(!claude.is_available());
    assert_eq!(claude.find_sessions().len(), 0);

    let codex = CodexAdapter::new(nonexistent.clone());
    assert!(!codex.is_available());
    assert_eq!(codex.find_sessions().len(), 0);

    let copilot = CopilotAdapter::new(nonexistent.clone());
    assert!(!copilot.is_available());
    assert_eq!(copilot.find_sessions().len(), 0);

    let vibe = VibeAdapter::new(nonexistent.clone());
    assert!(!vibe.is_available());
    assert_eq!(vibe.find_sessions().len(), 0);

    let qwen = QwenAdapter::new(nonexistent.clone());
    assert!(!qwen.is_available());
    assert_eq!(qwen.find_sessions().len(), 0);

    let gemini = GeminiAdapter::new(nonexistent.clone());
    assert!(!gemini.is_available());
    assert_eq!(gemini.find_sessions().len(), 0);

    let kimi = KimiAdapter::new(nonexistent.clone());
    assert!(!kimi.is_available());
    assert_eq!(kimi.find_sessions().len(), 0);
}

// ── Incremental scan ─────────────────────────────────────────────────────

#[test]
fn test_incremental_detects_new_and_deleted() {
    let tmp = TempDir::new().unwrap();
    let project = tmp.path().join("proj");
    fs::create_dir_all(&project).unwrap();

    let jsonl = r#"{"type":"user","cwd":"/tmp","message":{"content":"First session test content"}}
{"type":"assistant","message":{"content":"Response to first session."}}
"#;
    fs::write(project.join("ses-1.jsonl"), jsonl).unwrap();

    let adapter = ClaudeAdapter::new(tmp.path().to_path_buf());

    // First scan: everything is new
    let (new_sessions, deleted) =
        adapter.find_sessions_incremental(&std::collections::HashMap::new(), &None, &None);
    assert_eq!(new_sessions.len(), 1);
    assert!(deleted.is_empty());

    // Build known map
    let mut known = std::collections::HashMap::new();
    for s in &new_sessions {
        known.insert(s.id.clone(), (s.mtime, s.agent.clone()));
    }

    // Second scan with same state: nothing new
    let (new_sessions, deleted) = adapter.find_sessions_incremental(&known, &None, &None);
    assert!(new_sessions.is_empty());
    assert!(deleted.is_empty());

    // Delete the file: should detect deletion
    fs::remove_file(project.join("ses-1.jsonl")).unwrap();
    let (new_sessions, deleted) = adapter.find_sessions_incremental(&known, &None, &None);
    assert!(new_sessions.is_empty());
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0], "ses-1");
}

// ── Helper ───────────────────────────────────────────────────────────────

fn make_session(id: &str, agent: &str) -> rust_resume::session::Session {
    rust_resume::session::Session {
        id: id.to_string(),
        agent: agent.to_string(),
        title: "Test".to_string(),
        directory: "/tmp".to_string(),
        timestamp: chrono::NaiveDateTime::default(),
        content: String::new(),
        message_count: 0,
        mtime: 0.0,
        yolo: false,
    }
}
