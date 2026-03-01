use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;

/// Manages agent icon images for terminal rendering.
pub struct IconManager {
    picker: Picker,
    icons: HashMap<String, image::DynamicImage>,
    /// Pre-created protocols for each agent, keyed by "agent:WxH".
    protocols: HashMap<String, StatefulProtocol>,
}

impl IconManager {
    /// Create a new IconManager. Must be called BEFORE entering alternate screen.
    pub fn new(assets_dir: &Path) -> Option<Self> {
        let picker = Picker::from_query_stdio().ok()?;

        let mut icons = HashMap::new();
        let agents = [
            "claude",
            "codex",
            "copilot-cli",
            "copilot-vscode",
            "crush",
            "gemini",
            "kimi",
            "opencode",
            "qwen",
            "vibe",
        ];

        for agent in agents {
            let icon_path = assets_dir.join(format!("{agent}.png"));
            if icon_path.exists()
                && let Ok(img) = image::open(&icon_path)
            {
                icons.insert(agent.to_string(), img);
            }
        }

        Some(Self {
            picker,
            icons,
            protocols: HashMap::new(),
        })
    }

    /// Get a StatefulProtocol for the given agent, sized for the given area.
    /// Creates and caches the protocol on first use per size.
    pub fn get_protocol(
        &mut self,
        agent: &str,
        area: ratatui::layout::Rect,
    ) -> Option<&mut StatefulProtocol> {
        let key = format!("{agent}:{}x{}", area.width, area.height);

        if !self.protocols.contains_key(&key) {
            let img = self.icons.get(agent)?;
            let protocol = self.picker.new_resize_protocol(img.clone());
            self.protocols.insert(key.clone(), protocol);
        }

        self.protocols.get_mut(&key)
    }

    pub fn has_icon(&self, agent: &str) -> bool {
        self.icons.contains_key(agent)
    }
}

/// Get the assets directory path.
pub fn assets_dir() -> PathBuf {
    // Try relative to executable first, then fallback to compile-time path
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()));

    if let Some(dir) = exe_dir {
        let assets = dir.join("assets");
        if assets.is_dir() {
            return assets;
        }
        // Try ../assets (if binary is in target/release/)
        let assets = dir.join("../../assets");
        if assets.is_dir() {
            return assets;
        }
    }

    // Fallback: hardcoded path relative to project root
    let home = dirs::home_dir().unwrap_or_default();
    home.join("src/rust-resume/assets")
}
