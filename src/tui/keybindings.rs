use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::KeyOrKeys;

/// Every bindable action in the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    // Global (always active regardless of pane focus)
    Quit,
    ResumeSession,
    TogglePreview,
    TogglePreviewLayout,
    ToggleSort,
    DeleteWordBackward,
    ClearSearch,
    ToggleMouseCapture,
    TogglePaneFocus,
    CycleDirectoryScope,
    CycleAgentFilterForward,
    CycleAgentFilterBackward,
    RefreshSessions,

    // Results-focused (only fire when results pane focused)
    NavigateDown,
    NavigateUp,
    PageDown,
    PageUp,
    CursorHome,
    CursorEnd,
    CursorLeft,
    CursorRight,
    CursorWordLeft,
    CursorWordRight,
    DeleteCharBackward,
    SwitchToPreview,

    // Preview-focused (only fire when preview pane focused)
    ScrollPreviewDown,
    ScrollPreviewUp,
    PagePreviewDown,
    PagePreviewUp,
    ScrollPreviewToTop,
    ScrollPreviewToBottom,
    CopySessionContent,
    SwitchToResults,

    // Relocate mode
    RelocateSession,

    // Cross-pane shift navigation (behavior inverts based on focus)
    ShiftDown,
    ShiftUp,
    ShiftPageDown,
    ShiftPageUp,
}

/// A normalized key combination for lookup.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyCombo {
    pub fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { code, modifiers }
    }

    /// Create from a crossterm KeyEvent, keeping only CONTROL and SHIFT bits.
    /// Normalizes BackTab by stripping SHIFT (BackTab already implies Shift+Tab).
    pub fn from_key_event(key: &KeyEvent) -> Self {
        let mut mods = key.modifiers & (KeyModifiers::CONTROL | KeyModifiers::SHIFT);
        // BackTab already implies Shift+Tab; strip SHIFT to match the parsed form
        if key.code == KeyCode::BackTab {
            mods &= !KeyModifiers::SHIFT;
        }
        Self {
            code: key.code,
            modifiers: mods,
        }
    }

    /// Parse a string like "ctrl+s", "shift+up", "enter", "backtick".
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_lowercase();
        let parts: Vec<&str> = s.split('+').collect();
        if parts.is_empty() {
            return None;
        }

        let mut modifiers = KeyModifiers::empty();
        for &part in &parts[..parts.len() - 1] {
            match part {
                "ctrl" | "control" => modifiers |= KeyModifiers::CONTROL,
                "shift" => modifiers |= KeyModifiers::SHIFT,
                _ => return None, // unknown modifier
            }
        }

        let key_name = parts[parts.len() - 1];
        let code = parse_key_name(key_name)?;

        // "shift+tab" → BackTab (crossterm convention)
        if code == KeyCode::Tab && modifiers.contains(KeyModifiers::SHIFT) {
            return Some(Self {
                code: KeyCode::BackTab,
                modifiers: modifiers & !KeyModifiers::SHIFT,
            });
        }

        Some(Self { code, modifiers })
    }
}

fn parse_key_name(name: &str) -> Option<KeyCode> {
    match name {
        "esc" | "escape" => Some(KeyCode::Esc),
        "enter" | "return" => Some(KeyCode::Enter),
        "tab" => Some(KeyCode::Tab),
        "backtab" => Some(KeyCode::BackTab),
        "backspace" | "bs" => Some(KeyCode::Backspace),
        "up" => Some(KeyCode::Up),
        "down" => Some(KeyCode::Down),
        "left" => Some(KeyCode::Left),
        "right" => Some(KeyCode::Right),
        "pgup" | "pageup" => Some(KeyCode::PageUp),
        "pgdn" | "pagedown" => Some(KeyCode::PageDown),
        "home" => Some(KeyCode::Home),
        "end" => Some(KeyCode::End),
        "backtick" => Some(KeyCode::Char('`')),
        s if s.len() == 1 => Some(KeyCode::Char(s.chars().next().unwrap())),
        _ => None,
    }
}

fn action_from_str(name: &str) -> Option<Action> {
    match name {
        "quit" => Some(Action::Quit),
        "resume_session" => Some(Action::ResumeSession),
        "toggle_preview" => Some(Action::TogglePreview),
        "toggle_preview_layout" => Some(Action::TogglePreviewLayout),
        "toggle_sort" => Some(Action::ToggleSort),
        "delete_word_backward" => Some(Action::DeleteWordBackward),
        "clear_search" => Some(Action::ClearSearch),
        "toggle_mouse_capture" => Some(Action::ToggleMouseCapture),
        "toggle_pane_focus" => Some(Action::TogglePaneFocus),
        "cycle_directory_scope" => Some(Action::CycleDirectoryScope),
        "cycle_agent_filter_forward" => Some(Action::CycleAgentFilterForward),
        "cycle_agent_filter_backward" => Some(Action::CycleAgentFilterBackward),
        "refresh_sessions" => Some(Action::RefreshSessions),
        "navigate_down" => Some(Action::NavigateDown),
        "navigate_up" => Some(Action::NavigateUp),
        "page_down" => Some(Action::PageDown),
        "page_up" => Some(Action::PageUp),
        "cursor_home" => Some(Action::CursorHome),
        "cursor_end" => Some(Action::CursorEnd),
        "cursor_left" => Some(Action::CursorLeft),
        "cursor_right" => Some(Action::CursorRight),
        "cursor_word_left" => Some(Action::CursorWordLeft),
        "cursor_word_right" => Some(Action::CursorWordRight),
        "delete_char_backward" => Some(Action::DeleteCharBackward),
        "switch_to_preview" => Some(Action::SwitchToPreview),
        "scroll_preview_down" => Some(Action::ScrollPreviewDown),
        "scroll_preview_up" => Some(Action::ScrollPreviewUp),
        "page_preview_down" => Some(Action::PagePreviewDown),
        "page_preview_up" => Some(Action::PagePreviewUp),
        "scroll_preview_to_top" => Some(Action::ScrollPreviewToTop),
        "scroll_preview_to_bottom" => Some(Action::ScrollPreviewToBottom),
        "copy_session_content" => Some(Action::CopySessionContent),
        "switch_to_results" => Some(Action::SwitchToResults),
        "shift_down" => Some(Action::ShiftDown),
        "shift_up" => Some(Action::ShiftUp),
        "shift_page_down" => Some(Action::ShiftPageDown),
        "shift_page_up" => Some(Action::ShiftPageUp),
        "relocate_session" => Some(Action::RelocateSession),
        _ => None,
    }
}

/// Runtime keybinding lookup table.
pub struct KeyBindings {
    map: HashMap<KeyCombo, Vec<Action>>,
}

static EMPTY: Vec<Action> = Vec::new();

impl KeyBindings {
    /// Build keybindings from user config merged with defaults.
    pub fn load(user_config: &HashMap<String, KeyOrKeys>) -> Self {
        // Build action→keys from defaults
        let mut action_keys: HashMap<Action, Vec<KeyCombo>> = HashMap::new();
        for (action, combos) in Self::defaults() {
            action_keys.insert(action, combos);
        }

        // Override with user config
        for (name, keys) in user_config {
            let Some(action) = action_from_str(name) else {
                eprintln!("warning: unknown keybinding action: {name}");
                continue;
            };
            let strings = match keys {
                KeyOrKeys::Single(s) => vec![s.clone()],
                KeyOrKeys::Multiple(v) => v.clone(),
            };
            let mut combos = Vec::new();
            for s in &strings {
                if let Some(combo) = KeyCombo::parse(s) {
                    combos.push(combo);
                } else {
                    eprintln!("warning: cannot parse key: {s}");
                }
            }
            action_keys.insert(action, combos);
        }

        // Build inverted map: key → actions
        let mut map: HashMap<KeyCombo, Vec<Action>> = HashMap::new();
        for (action, combos) in &action_keys {
            for combo in combos {
                map.entry(combo.clone()).or_default().push(*action);
            }
        }

        Self { map }
    }

    /// Look up actions for a key event.
    pub fn lookup(&self, key: &KeyEvent) -> &[Action] {
        let combo = KeyCombo::from_key_event(key);
        self.map.get(&combo).map_or(&EMPTY, |v| v.as_slice())
    }

    /// All default keybindings.
    fn defaults() -> Vec<(Action, Vec<KeyCombo>)> {
        let ctrl = KeyModifiers::CONTROL;
        let shift = KeyModifiers::SHIFT;
        let none = KeyModifiers::empty();

        let k = |code: KeyCode, mods: KeyModifiers| KeyCombo::new(code, mods);

        vec![
            // Global
            (
                Action::Quit,
                vec![
                    k(KeyCode::Esc, none),
                    k(KeyCode::Char('c'), ctrl),
                    k(KeyCode::Char('q'), ctrl),
                ],
            ),
            (Action::ResumeSession, vec![k(KeyCode::Enter, none)]),
            (Action::TogglePreview, vec![k(KeyCode::Char('`'), ctrl)]),
            (
                Action::TogglePreviewLayout,
                vec![k(KeyCode::Char('p'), ctrl)],
            ),
            (Action::ToggleSort, vec![k(KeyCode::Char('s'), ctrl)]),
            (
                Action::DeleteWordBackward,
                vec![k(KeyCode::Char('w'), ctrl), k(KeyCode::Backspace, ctrl)],
            ),
            (Action::ClearSearch, vec![k(KeyCode::Char('u'), ctrl)]),
            (
                Action::ToggleMouseCapture,
                vec![k(KeyCode::Char('e'), ctrl)],
            ),
            (Action::TogglePaneFocus, vec![k(KeyCode::Char('t'), ctrl)]),
            (
                Action::CycleDirectoryScope,
                vec![k(KeyCode::Char('d'), ctrl)],
            ),
            (Action::CycleAgentFilterForward, vec![k(KeyCode::Tab, none)]),
            (
                Action::CycleAgentFilterBackward,
                vec![k(KeyCode::BackTab, none)],
            ),
            (Action::RefreshSessions, vec![k(KeyCode::Char('r'), ctrl)]),
            (
                Action::RelocateSession,
                vec![k(KeyCode::Char('o'), ctrl)],
            ),
            // Results-focused
            (Action::NavigateDown, vec![k(KeyCode::Down, none)]),
            (Action::NavigateUp, vec![k(KeyCode::Up, none)]),
            (Action::PageDown, vec![k(KeyCode::PageDown, none)]),
            (Action::PageUp, vec![k(KeyCode::PageUp, none)]),
            (Action::CursorHome, vec![k(KeyCode::Home, none)]),
            (Action::CursorEnd, vec![k(KeyCode::End, none)]),
            (Action::CursorLeft, vec![k(KeyCode::Left, none)]),
            (Action::CursorRight, vec![k(KeyCode::Right, none)]),
            (Action::CursorWordLeft, vec![k(KeyCode::Left, ctrl)]),
            (Action::CursorWordRight, vec![k(KeyCode::Right, ctrl)]),
            (
                Action::DeleteCharBackward,
                vec![k(KeyCode::Backspace, none)],
            ),
            (Action::SwitchToPreview, vec![k(KeyCode::Char('`'), none)]),
            // Preview-focused
            (Action::ScrollPreviewDown, vec![k(KeyCode::Down, none)]),
            (Action::ScrollPreviewUp, vec![k(KeyCode::Up, none)]),
            (Action::PagePreviewDown, vec![k(KeyCode::PageDown, none)]),
            (Action::PagePreviewUp, vec![k(KeyCode::PageUp, none)]),
            (Action::ScrollPreviewToTop, vec![k(KeyCode::Home, none)]),
            (Action::ScrollPreviewToBottom, vec![k(KeyCode::End, none)]),
            (
                Action::CopySessionContent,
                vec![k(KeyCode::Char('c'), none)],
            ),
            (Action::SwitchToResults, vec![k(KeyCode::Char('`'), none)]),
            // Cross-pane shift
            (Action::ShiftDown, vec![k(KeyCode::Down, shift)]),
            (Action::ShiftUp, vec![k(KeyCode::Up, shift)]),
            (Action::ShiftPageDown, vec![k(KeyCode::PageDown, shift)]),
            (Action::ShiftPageUp, vec![k(KeyCode::PageUp, shift)]),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_keys() {
        let c = KeyCombo::parse("esc").unwrap();
        assert_eq!(c.code, KeyCode::Esc);
        assert_eq!(c.modifiers, KeyModifiers::empty());

        let c = KeyCombo::parse("enter").unwrap();
        assert_eq!(c.code, KeyCode::Enter);

        let c = KeyCombo::parse("backtick").unwrap();
        assert_eq!(c.code, KeyCode::Char('`'));
    }

    #[test]
    fn parse_with_modifiers() {
        let c = KeyCombo::parse("ctrl+s").unwrap();
        assert_eq!(c.code, KeyCode::Char('s'));
        assert_eq!(c.modifiers, KeyModifiers::CONTROL);

        let c = KeyCombo::parse("shift+up").unwrap();
        assert_eq!(c.code, KeyCode::Up);
        assert_eq!(c.modifiers, KeyModifiers::SHIFT);

        let c = KeyCombo::parse("ctrl+backspace").unwrap();
        assert_eq!(c.code, KeyCode::Backspace);
        assert_eq!(c.modifiers, KeyModifiers::CONTROL);
    }

    #[test]
    fn parse_shift_tab_to_backtab() {
        let c = KeyCombo::parse("shift+tab").unwrap();
        assert_eq!(c.code, KeyCode::BackTab);
        assert_eq!(c.modifiers, KeyModifiers::empty());
    }

    #[test]
    fn parse_case_insensitive() {
        let c = KeyCombo::parse("Ctrl+S").unwrap();
        assert_eq!(c.code, KeyCode::Char('s'));
        assert_eq!(c.modifiers, KeyModifiers::CONTROL);
    }

    #[test]
    fn default_bindings_load() {
        let kb = KeyBindings::load(&HashMap::new());
        // Esc should map to Quit
        let combo = KeyCombo::new(KeyCode::Esc, KeyModifiers::empty());
        let actions = kb.map.get(&combo).unwrap();
        assert!(actions.contains(&Action::Quit));
    }

    #[test]
    fn user_override_replaces_defaults() {
        let mut user = HashMap::new();
        // Override quit to only use F1 ... well, "q" since we don't support F keys
        user.insert("quit".to_string(), KeyOrKeys::Single("q".to_string()));
        let kb = KeyBindings::load(&user);

        // "q" should now have Quit
        let combo = KeyCombo::new(KeyCode::Char('q'), KeyModifiers::empty());
        let actions = kb.map.get(&combo).unwrap();
        assert!(actions.contains(&Action::Quit));

        // Esc should no longer have Quit
        let combo = KeyCombo::new(KeyCode::Esc, KeyModifiers::empty());
        let actions = kb.map.get(&combo);
        assert!(actions.is_none() || !actions.unwrap().contains(&Action::Quit));
    }
}
