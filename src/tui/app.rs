use std::collections::HashMap;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

use super::keybindings::{Action, KeyBindings};
use ratatui::layout::Rect;
use tui_scrollbar::{ScrollBar, ScrollBarInteraction, ScrollCommand};

use crate::search::{LoadingMsg, SessionSearch};
use crate::session::Session;

use super::icons::IconManager;
use super::results_list::ResultsState;
use super::theme::Theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FocusedPane {
    Results,
    Preview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortColumn {
    /// Sort by BM25 relevance (when query active) or Date (when no query).
    Relevance,
    Agent,
    Title,
    Directory,
    Turns,
    Date,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Asc,
    Desc,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DirectoryScope {
    /// Exact match: only sessions from this exact directory.
    Local,
    /// Contains match: sessions whose directory contains the filter string.
    Project,
    /// No directory filter.
    Global,
}

pub struct App {
    pub query: String,
    pub cursor_pos: usize,
    pub sessions: Vec<Session>,
    pub filtered: Vec<Session>,
    pub results_state: ResultsState,
    pub preview_scroll: u16,
    pub show_preview: bool,
    pub preview_bottom: bool, // true = bottom, false = right
    pub agent_filter: Option<String>,
    pub agent_counts: HashMap<String, usize>,
    pub total_count: usize,
    pub sort_column: SortColumn,
    pub sort_direction: SortDirection,
    pub directory_filter: Option<String>,
    /// Directory scope mode (Local/Project/Global).
    pub directory_scope: DirectoryScope,
    pub search_engine: SessionSearch,
    pub should_quit: bool,
    pub resume_session: Option<Session>,
    pub yolo: bool,
    pub status_msg: Option<String>,
    pub search_dirty: bool,
    pub last_search_time: Option<Duration>,
    /// Area where results list is rendered (for mouse click mapping).
    pub results_area: ratatui::layout::Rect,
    /// Area where preview is rendered (for mouse scroll).
    pub preview_area: ratatui::layout::Rect,
    /// Icon manager for rendering agent icons.
    pub icons: Option<IconManager>,
    /// Whether mouse capture is enabled (toggle with Ctrl+M).
    pub mouse_captured: bool,
    /// Set when mouse capture state changed, to apply in run_loop.
    pub mouse_toggle_pending: bool,
    /// Total physical lines in preview content (set during render).
    pub preview_total_lines: usize,
    /// tui-scrollbar interaction state for results list.
    pub results_sb_interaction: ScrollBarInteraction,
    /// tui-scrollbar interaction state for preview.
    pub preview_sb_interaction: ScrollBarInteraction,
    /// Current results scrollbar widget (set during render for mouse events).
    pub results_scrollbar: Option<ScrollBar>,
    /// Current preview scrollbar widget (set during render for mouse events).
    pub preview_scrollbar: Option<ScrollBar>,
    /// Scrollbar area for results (set during render).
    pub results_sb_area: Rect,
    /// Scrollbar area for preview (set during render).
    pub preview_sb_area: Rect,
    /// Whether preview should auto-scroll to first match on next render.
    pub preview_auto_scroll: bool,
    /// Which pane is focused for navigation keys.
    pub focused_pane: FocusedPane,
    /// Area where filter bar is rendered (for mouse click).
    pub filter_bar_area: Rect,
    /// Last click info for double-click detection: (time, selected_index).
    last_click: Option<(Instant, usize)>,
    /// Whether sessions are still loading in background.
    pub loading: bool,
    /// Receiver for progressive background session loading.
    pub loading_rx: Option<std::sync::mpsc::Receiver<LoadingMsg>>,
    /// Configurable keybindings.
    pub keybindings: KeyBindings,
    /// Resolved theme colors.
    pub theme: Theme,
    /// BM25 relevance scores from the last search (id → score).
    search_scores: HashMap<String, f64>,
    /// Whether query was empty on last apply_filter (for detecting transitions).
    prev_query_empty: bool,
}

impl App {
    pub fn new(yolo: bool, keybindings: KeyBindings, theme: Theme) -> Self {
        Self {
            query: String::new(),
            cursor_pos: 0,
            sessions: Vec::new(),
            filtered: Vec::new(),
            results_state: ResultsState::default(),
            preview_scroll: 0,
            show_preview: true,
            preview_bottom: true,
            agent_filter: None,
            agent_counts: HashMap::new(),
            total_count: 0,
            sort_column: SortColumn::Date,
            sort_direction: SortDirection::Desc,
            directory_filter: None,
            directory_scope: DirectoryScope::Global,
            search_engine: SessionSearch::new(),
            should_quit: false,
            resume_session: None,
            yolo,
            status_msg: None,
            search_dirty: false,
            last_search_time: None,
            results_area: ratatui::layout::Rect::default(),
            preview_area: ratatui::layout::Rect::default(),
            icons: None,
            mouse_captured: true,
            mouse_toggle_pending: false,
            preview_total_lines: 0,
            results_sb_interaction: ScrollBarInteraction::new(),
            preview_sb_interaction: ScrollBarInteraction::new(),
            results_scrollbar: None,
            preview_scrollbar: None,
            results_sb_area: Rect::default(),
            preview_sb_area: Rect::default(),
            preview_auto_scroll: false,
            focused_pane: FocusedPane::Results,
            filter_bar_area: Rect::default(),
            last_click: None,
            loading: false,
            loading_rx: None,
            keybindings,
            theme,
            search_scores: HashMap::new(),
            prev_query_empty: true,
        }
    }

    /// Pre-warm jieba dictionary in background so first Ctrl+W doesn't lag.
    pub fn warm_jieba() {
        std::thread::spawn(|| {
            let _ = &*JIEBA;
        });
    }

    /// Start loading sessions in a background thread (progressive).
    pub fn start_loading(&mut self) {
        self.loading = true;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut engine = SessionSearch::new();
            engine.load_progressive(false, &tx);
            let _ = tx.send(LoadingMsg::Done(Box::new(engine)));
        });
        self.loading_rx = Some(rx);
    }

    /// Check for progressive loading updates (non-blocking, drains all available).
    /// Returns true if any updates were received (i.e. a redraw is needed).
    pub fn check_loading(&mut self) -> bool {
        if self.loading_rx.is_none() {
            return false;
        }

        let mut got_update = false;
        let mut done_engine: Option<SessionSearch> = None;

        // Drain all available messages
        loop {
            let msg = self.loading_rx.as_ref().unwrap().try_recv();
            match msg {
                Ok(LoadingMsg::Sessions(sessions)) => {
                    self.sessions = sessions;
                    self.update_agent_counts();
                    got_update = true;
                }
                Ok(LoadingMsg::Done(engine)) => {
                    done_engine = Some(*engine);
                    got_update = true;
                    break;
                }
                Err(_) => break,
            }
        }

        if let Some(engine) = done_engine {
            self.search_engine = engine;
            self.loading = false;
            self.loading_rx = None;
        }

        if got_update {
            self.apply_filter();
        }

        got_update
    }

    pub fn update_agent_counts(&mut self) {
        self.agent_counts.clear();
        let scope = self.directory_scope;
        let dir = self.directory_filter.as_deref();
        let mut total = 0;
        for s in &self.sessions {
            if !Self::session_matches_scope(s, scope, dir) {
                continue;
            }
            *self.agent_counts.entry(s.agent.clone()).or_insert(0) += 1;
            total += 1;
        }
        self.total_count = total;
    }

    /// Check if a session matches a directory scope.
    fn session_matches_scope(session: &Session, scope: DirectoryScope, dir: Option<&str>) -> bool {
        let Some(dir) = dir else {
            return true;
        };
        match scope {
            DirectoryScope::Global => true,
            DirectoryScope::Local => session.directory == dir,
            DirectoryScope::Project => session
                .directory
                .to_lowercase()
                .contains(&dir.to_lowercase()),
        }
    }

    pub fn apply_filter(&mut self) {
        let start = Instant::now();

        // Compute effective directory filter for search engine
        let effective_dir = match self.directory_scope {
            DirectoryScope::Global => None,
            _ => self.directory_filter.as_deref(),
        };

        // Capture scope params to avoid borrow issues in closures
        let scope = self.directory_scope;
        let dir_filter = self.directory_filter.clone();

        let has_query = !self.query.is_empty();

        // Auto-switch sort mode only on query state transitions:
        // - empty → non-empty: switch to Relevance
        // - non-empty → empty: switch back to Date
        if has_query && self.prev_query_empty {
            self.sort_column = SortColumn::Relevance;
        } else if !has_query && !self.prev_query_empty {
            self.sort_column = SortColumn::Date;
            self.sort_direction = SortDirection::Desc;
        }
        self.prev_query_empty = !has_query;

        if !has_query {
            self.filtered = self.sessions.clone();
            self.search_scores.clear();
            if let Some(ref agent) = self.agent_filter {
                self.filtered.retain(|s| &s.agent == agent);
            }
            self.filtered
                .retain(|s| Self::session_matches_scope(s, scope, dir_filter.as_deref()));
        } else {
            let scored_results = self.search_engine.search(
                &self.query,
                self.agent_filter.as_deref(),
                effective_dir,
                200,
            );
            self.search_scores.clear();
            self.filtered = scored_results
                .into_iter()
                .map(|(session, score)| {
                    self.search_scores.insert(session.id.clone(), score);
                    session
                })
                .collect();
            // search engine uses "contains" — for Local mode, further filter to exact
            if scope == DirectoryScope::Local {
                self.filtered
                    .retain(|s| Self::session_matches_scope(s, scope, dir_filter.as_deref()));
            }
        }

        // Sort results.
        // - Relevance mode: BM25 score (with query) or Date desc (without query)
        // - Column mode: chosen column primary, BM25 tiebreaker (with query)
        let dir = self.sort_direction;
        let scores = &self.search_scores;

        match self.sort_column {
            SortColumn::Relevance => {
                if has_query {
                    // Pure BM25 relevance sort
                    self.filtered.sort_by(|a, b| {
                        let sa = scores.get(&a.id).copied().unwrap_or(0.0);
                        let sb = scores.get(&b.id).copied().unwrap_or(0.0);
                        sb.total_cmp(&sa)
                    });
                } else {
                    // No query — fall back to Date desc
                    self.filtered.sort_by(|a, b| b.mtime.total_cmp(&a.mtime));
                }
            }
            _ => {
                let column_cmp = |a: &Session, b: &Session| -> std::cmp::Ordering {
                    let cmp = match self.sort_column {
                        SortColumn::Date => a.mtime.total_cmp(&b.mtime),
                        SortColumn::Agent => a.agent.cmp(&b.agent),
                        SortColumn::Title => a.title.cmp(&b.title),
                        SortColumn::Directory => a.directory.cmp(&b.directory),
                        SortColumn::Turns => a.message_count.cmp(&b.message_count),
                        SortColumn::Relevance => unreachable!(),
                    };
                    if dir == SortDirection::Desc {
                        cmp.reverse()
                    } else {
                        cmp
                    }
                };

                if has_query {
                    // Column primary, BM25 tiebreaker
                    self.filtered.sort_by(|a, b| {
                        let primary = column_cmp(a, b);
                        if primary != std::cmp::Ordering::Equal {
                            return primary;
                        }
                        let sa = scores.get(&a.id).copied().unwrap_or(0.0);
                        let sb = scores.get(&b.id).copied().unwrap_or(0.0);
                        sb.total_cmp(&sa)
                    });
                } else {
                    self.filtered.sort_by(column_cmp);
                }
            }
        }

        self.last_search_time = Some(start.elapsed());
        self.results_state.select_first();
        self.preview_scroll = 0;
        self.preview_auto_scroll = true;
        self.search_dirty = false;
    }

    /// Toggle sorting by the given column. If already active, flip direction;
    /// if different column, switch to it with a sensible default direction.
    pub fn toggle_sort_column(&mut self, column: SortColumn) {
        if self.sort_column == column {
            // Cycle: default direction → flipped → reset to Relevance
            let default_dir = match column {
                SortColumn::Date | SortColumn::Turns => SortDirection::Desc,
                _ => SortDirection::Asc,
            };
            if self.sort_direction == default_dir {
                // First click was default → flip
                self.sort_direction = match default_dir {
                    SortDirection::Asc => SortDirection::Desc,
                    SortDirection::Desc => SortDirection::Asc,
                };
            } else {
                // Already flipped → reset to Relevance
                self.sort_column = SortColumn::Relevance;
                self.sort_direction = SortDirection::Desc;
            }
        } else {
            self.sort_column = column;
            self.sort_direction = match column {
                SortColumn::Date | SortColumn::Turns => SortDirection::Desc,
                _ => SortDirection::Asc,
            };
        }
        self.search_dirty = true;
    }

    pub fn selected_session(&self) -> Option<&Session> {
        self.filtered.get(self.results_state.selected)
    }

    /// Poll for events and handle them. Returns true if a redraw is needed.
    pub fn handle_events(&mut self) -> std::io::Result<bool> {
        let mut needs_redraw = false;

        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) => {
                    self.handle_key(key);
                    needs_redraw = true;
                }
                Event::Mouse(mouse) => {
                    // Ignore Shift+mouse: let terminal handle native text selection
                    if !mouse.modifiers.contains(KeyModifiers::SHIFT) {
                        self.handle_mouse(mouse);
                        needs_redraw = true;
                    }
                }
                Event::Resize(..) => {
                    if !self.query.is_empty() {
                        self.preview_auto_scroll = true;
                    }
                    needs_redraw = true;
                }
                _ => {}
            }
        }

        if self.search_dirty {
            self.apply_filter();
            needs_redraw = true;
        }

        Ok(needs_redraw)
    }

    fn handle_key(&mut self, key: KeyEvent) {
        let preview_focused = self.focused_pane == FocusedPane::Preview;
        let actions: Vec<Action> = self.keybindings.lookup(&key).to_vec();

        let mut handled = false;
        for &action in &actions {
            match action {
                // Global actions
                Action::Quit => {
                    self.should_quit = true;
                    handled = true;
                }
                Action::ResumeSession => {
                    if let Some(session) = self.selected_session().cloned() {
                        self.resume_session = Some(session);
                        self.should_quit = true;
                    }
                    handled = true;
                }
                Action::TogglePreview => {
                    self.show_preview = !self.show_preview;
                    if !self.show_preview {
                        self.focused_pane = FocusedPane::Results;
                    }
                    handled = true;
                }
                Action::TogglePreviewLayout => {
                    self.preview_bottom = !self.preview_bottom;
                    handled = true;
                }
                Action::ToggleSort => {
                    if !self.query.is_empty() {
                        // With query: toggle Relevance ↔ Date
                        if self.sort_column == SortColumn::Relevance {
                            self.sort_column = SortColumn::Date;
                            self.sort_direction = SortDirection::Desc;
                        } else {
                            self.sort_column = SortColumn::Relevance;
                        }
                    } else {
                        // No query: toggle Date direction
                        self.toggle_sort_column(SortColumn::Date);
                    }
                    self.search_dirty = true;
                    handled = true;
                }
                Action::DeleteWordBackward => {
                    self.delete_word_backward();
                    self.search_dirty = true;
                    handled = true;
                }
                Action::ClearSearch => {
                    self.query.clear();
                    self.cursor_pos = 0;
                    self.search_dirty = true;
                    handled = true;
                }
                Action::ToggleMouseCapture => {
                    self.mouse_captured = !self.mouse_captured;
                    self.mouse_toggle_pending = true;
                    handled = true;
                }
                Action::TogglePaneFocus => {
                    if self.show_preview {
                        self.focused_pane = match self.focused_pane {
                            FocusedPane::Results => FocusedPane::Preview,
                            FocusedPane::Preview => FocusedPane::Results,
                        };
                    }
                    handled = true;
                }
                Action::CycleDirectoryScope => {
                    if self.directory_filter.is_some() {
                        self.directory_scope = match self.directory_scope {
                            DirectoryScope::Local => DirectoryScope::Project,
                            DirectoryScope::Project => DirectoryScope::Global,
                            DirectoryScope::Global => DirectoryScope::Local,
                        };
                        self.update_agent_counts();
                        self.search_dirty = true;
                    }
                    handled = true;
                }
                Action::CycleAgentFilterForward => {
                    self.cycle_agent_filter();
                    self.search_dirty = true;
                    handled = true;
                }
                Action::CycleAgentFilterBackward => {
                    self.cycle_agent_filter_back();
                    self.search_dirty = true;
                    handled = true;
                }
                Action::RefreshSessions => {
                    self.start_loading();
                    handled = true;
                }

                // Results-focused actions
                Action::NavigateDown if !preview_focused => {
                    self.navigate_results_next();
                    handled = true;
                }
                Action::NavigateUp if !preview_focused => {
                    self.navigate_results_prev();
                    handled = true;
                }
                Action::PageDown if !preview_focused => {
                    self.results_state.page_down(10, self.filtered.len());
                    self.preview_scroll = 0;
                    self.preview_auto_scroll = true;
                    handled = true;
                }
                Action::PageUp if !preview_focused => {
                    self.results_state.page_up(10);
                    self.preview_scroll = 0;
                    self.preview_auto_scroll = true;
                    handled = true;
                }
                Action::CursorHome if !preview_focused => {
                    self.cursor_pos = 0;
                    handled = true;
                }
                Action::CursorEnd if !preview_focused => {
                    self.cursor_pos = self.query.len();
                    handled = true;
                }
                Action::CursorLeft if !preview_focused => {
                    if self.cursor_pos > 0 {
                        let prev = self.query[..self.cursor_pos]
                            .char_indices()
                            .next_back()
                            .map(|(i, _)| i)
                            .unwrap_or(0);
                        self.cursor_pos = prev;
                    }
                    handled = true;
                }
                Action::CursorRight if !preview_focused => {
                    if self.cursor_pos < self.query.len() {
                        let next = self.query[self.cursor_pos..]
                            .char_indices()
                            .nth(1)
                            .map(|(i, _)| self.cursor_pos + i)
                            .unwrap_or(self.query.len());
                        self.cursor_pos = next;
                    }
                    handled = true;
                }
                Action::CursorWordLeft => {
                    self.cursor_pos = self.word_boundary_left();
                    handled = true;
                }
                Action::CursorWordRight => {
                    self.cursor_pos = self.word_boundary_right();
                    handled = true;
                }
                Action::DeleteCharBackward if !preview_focused => {
                    if self.cursor_pos > 0 {
                        let prev = self.query[..self.cursor_pos]
                            .char_indices()
                            .next_back()
                            .map(|(i, _)| i)
                            .unwrap_or(0);
                        self.query.drain(prev..self.cursor_pos);
                        self.cursor_pos = prev;
                        self.search_dirty = true;
                    }
                    handled = true;
                }
                Action::SwitchToPreview if !preview_focused => {
                    if self.show_preview {
                        self.focused_pane = FocusedPane::Preview;
                    }
                    handled = true;
                }

                // Preview-focused actions
                Action::ScrollPreviewDown if preview_focused => {
                    self.scroll_preview(1);
                    handled = true;
                }
                Action::ScrollPreviewUp if preview_focused => {
                    self.scroll_preview(-1);
                    handled = true;
                }
                Action::PagePreviewDown if preview_focused => {
                    self.scroll_preview(10);
                    handled = true;
                }
                Action::PagePreviewUp if preview_focused => {
                    self.scroll_preview(-10);
                    handled = true;
                }
                Action::ScrollPreviewToTop if preview_focused => {
                    self.preview_scroll = 0;
                    handled = true;
                }
                Action::ScrollPreviewToBottom if preview_focused => {
                    let visible = self.preview_area.height.saturating_sub(2) as usize;
                    self.preview_scroll = self.preview_total_lines.saturating_sub(visible) as u16;
                    handled = true;
                }
                Action::CopySessionContent if preview_focused => {
                    if let Some(session) = self.selected_session() {
                        if super::utils::copy_to_clipboard(&session.content) {
                            self.status_msg = Some("Copied to clipboard".to_string());
                        } else {
                            self.status_msg = Some("Copy failed (no clipboard tool)".to_string());
                        }
                    }
                    handled = true;
                }
                Action::SwitchToResults if preview_focused => {
                    self.focused_pane = FocusedPane::Results;
                    handled = true;
                }

                // Cross-pane shift navigation
                Action::ShiftDown => {
                    if preview_focused {
                        self.navigate_results_next();
                    } else {
                        self.scroll_preview(1);
                    }
                    handled = true;
                }
                Action::ShiftUp => {
                    if preview_focused {
                        self.navigate_results_prev();
                    } else {
                        self.scroll_preview(-1);
                    }
                    handled = true;
                }
                Action::ShiftPageDown => {
                    if preview_focused {
                        self.results_state.page_down(10, self.filtered.len());
                        self.preview_scroll = 0;
                        self.preview_auto_scroll = true;
                    } else {
                        self.scroll_preview(10);
                    }
                    handled = true;
                }
                Action::ShiftPageUp => {
                    if preview_focused {
                        self.results_state.page_up(10);
                        self.preview_scroll = 0;
                        self.preview_auto_scroll = true;
                    } else {
                        self.scroll_preview(-10);
                    }
                    handled = true;
                }

                // Focus-gated actions that don't match current focus → skip
                _ => {}
            }
        }

        // Fallback: if no action handled and it's a plain character, insert into search
        if !handled
            && let KeyCode::Char(c) = key.code
            && !key.modifiers.contains(KeyModifiers::CONTROL)
            && !preview_focused
        {
            self.query.insert(self.cursor_pos, c);
            self.cursor_pos += c.len_utf8();
            self.search_dirty = true;
        }
    }

    fn navigate_results_next(&mut self) {
        self.results_state.select_next(self.filtered.len());
        self.preview_scroll = 0;
        self.preview_auto_scroll = true;
    }

    fn navigate_results_prev(&mut self) {
        self.results_state.select_prev();
        self.preview_scroll = 0;
        self.preview_auto_scroll = true;
    }

    fn scroll_preview(&mut self, delta: i32) {
        if delta > 0 {
            self.preview_scroll = self.preview_scroll.saturating_add(delta as u16);
        } else {
            self.preview_scroll = self.preview_scroll.saturating_sub((-delta) as u16);
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        // For drag events, always delegate to scrollbar (uses widened hit area)
        if matches!(mouse.kind, MouseEventKind::Drag(_) | MouseEventKind::Up(_)) {
            self.handle_scrollbar_mouse(mouse);
            return;
        }

        match mouse.kind {
            MouseEventKind::Down(crossterm::event::MouseButton::Left) => {
                // Check if click is exactly on a scrollbar column
                let on_results_sb = self.results_scrollbar.is_some()
                    && self.on_scrollbar_column(mouse.column, mouse.row, self.results_sb_area);
                let on_preview_sb = self.preview_scrollbar.is_some()
                    && self.on_scrollbar_column(mouse.column, mouse.row, self.preview_sb_area);

                if on_results_sb || on_preview_sb {
                    // Scrollbar click: delegate to tui-scrollbar (widened for drag tracking)
                    self.handle_scrollbar_mouse(mouse);
                    return;
                }

                // Filter bar click
                if self.is_in_area(mouse.column, mouse.row, self.filter_bar_area) {
                    if let Some(agent) = super::filter_bar::filter_hit_test(
                        mouse.column,
                        self.filter_bar_area,
                        &self.agent_counts,
                        self.total_count,
                    ) {
                        self.agent_filter = agent;
                        self.search_dirty = true;
                    }
                    return;
                }

                // Regular click: switch focus
                if self.is_in_area(mouse.column, mouse.row, self.preview_area) {
                    self.focused_pane = FocusedPane::Preview;
                } else if self.is_in_area(mouse.column, mouse.row, self.results_area) {
                    self.focused_pane = FocusedPane::Results;
                    let area = self.results_area;

                    // Header click: toggle sort column
                    let header_y = area.y + 1; // border + header row
                    if mouse.row == header_y && mouse.column > area.x {
                        let inner_width = (area.width.saturating_sub(2)) as usize; // subtract borders
                        let col = (mouse.column - area.x - 1) as usize;
                        let widths = super::results_list::compute_column_widths(inner_width);
                        if let Some(sort_col) = super::results_list::hit_test_header(col, &widths) {
                            self.toggle_sort_column(sort_col);
                        }
                        return;
                    }

                    // Select row on click
                    let content_y = area.y + 2;
                    let content_bottom = area.y + area.height.saturating_sub(1);
                    if mouse.row >= content_y && mouse.row < content_bottom {
                        let row_in_view = (mouse.row - content_y) as usize;
                        let new_selected = self.results_state.offset + row_in_view;
                        if new_selected < self.filtered.len() {
                            // Double-click detection: resume session
                            if let Some((last_time, last_idx)) = self.last_click
                                && last_idx == new_selected
                                && last_time.elapsed() < Duration::from_millis(400)
                                && let Some(session) = self.filtered.get(new_selected).cloned()
                            {
                                self.resume_session = Some(session);
                                self.should_quit = true;
                                return;
                            }
                            self.last_click = Some((Instant::now(), new_selected));
                            self.results_state.selected = new_selected;
                            self.preview_scroll = 0;
                            self.preview_auto_scroll = true;
                        }
                    }
                }
            }
            MouseEventKind::ScrollDown => {
                if self.is_in_area(mouse.column, mouse.row, self.preview_area) {
                    self.preview_scroll = self.preview_scroll.saturating_add(1);
                } else {
                    self.results_state.select_next(self.filtered.len());
                    self.preview_scroll = 0;
                    self.preview_auto_scroll = true;
                }
            }
            MouseEventKind::ScrollUp => {
                if self.is_in_area(mouse.column, mouse.row, self.preview_area) {
                    self.preview_scroll = self.preview_scroll.saturating_sub(1);
                } else {
                    self.results_state.select_prev();
                    self.preview_scroll = 0;
                    self.preview_auto_scroll = true;
                }
            }
            _ => {}
        }
    }

    /// Widen a 1-column scrollbar area for easier mouse targeting (3 cols).
    fn widen_hit_area(area: Rect) -> Rect {
        let expand = 2u16;
        Rect {
            x: area.x.saturating_sub(expand),
            y: area.y,
            width: area.width + expand,
            height: area.height,
        }
    }

    /// Delegate mouse events to tui-scrollbar widgets. Returns true if handled.
    /// Only forwards click/drag events; scroll wheel is handled by our own logic
    /// so that hover-based pane detection works correctly.
    fn handle_scrollbar_mouse(&mut self, mouse: MouseEvent) -> bool {
        // Don't let scrollbar steal scroll wheel events — we handle those ourselves
        if matches!(
            mouse.kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        ) {
            return false;
        }

        // Try results scrollbar (wider hit area for easier clicking)
        if let Some(ref sb) = self.results_scrollbar.clone() {
            let hit_area = Self::widen_hit_area(self.results_sb_area);
            if let Some(ScrollCommand::SetOffset(offset)) =
                sb.handle_mouse_event(hit_area, mouse, &mut self.results_sb_interaction)
            {
                let visible_rows = self.results_area.height.saturating_sub(3) as usize;
                let max_offset = self.filtered.len().saturating_sub(visible_rows);
                let new_offset = offset.min(max_offset);
                self.results_state.offset = new_offset;
                self.results_state.selected = self
                    .results_state
                    .selected
                    .clamp(new_offset, new_offset + visible_rows.saturating_sub(1));
                self.preview_scroll = 0;
                self.preview_auto_scroll = true;
                return true;
            }
        }

        // Try preview scrollbar (wider hit area for easier clicking)
        if let Some(ref sb) = self.preview_scrollbar.clone() {
            let hit_area = Self::widen_hit_area(self.preview_sb_area);
            if let Some(ScrollCommand::SetOffset(offset)) =
                sb.handle_mouse_event(hit_area, mouse, &mut self.preview_sb_interaction)
            {
                self.preview_scroll = offset as u16;
                return true;
            }
        }

        false
    }

    /// Check if a click is on or near (1 col tolerance) the scrollbar column.
    fn on_scrollbar_column(&self, col: u16, row: u16, sb_area: Rect) -> bool {
        if sb_area.width == 0 || sb_area.height == 0 {
            return false;
        }
        row >= sb_area.y
            && row < sb_area.y + sb_area.height
            && col >= sb_area.x.saturating_sub(1)
            && col <= sb_area.x + sb_area.width
    }

    fn is_in_area(&self, x: u16, y: u16, area: ratatui::layout::Rect) -> bool {
        x >= area.x && x < area.x + area.width && y >= area.y && y < area.y + area.height
    }

    fn word_boundary_left(&self) -> usize {
        find_prev_word_start(&self.query, self.cursor_pos)
    }

    fn word_boundary_right(&self) -> usize {
        find_next_word_end(&self.query, self.cursor_pos)
    }

    fn delete_word_backward(&mut self) {
        if self.cursor_pos == 0 {
            return;
        }
        let boundary = find_prev_word_start(&self.query, self.cursor_pos);
        self.query.drain(boundary..self.cursor_pos);
        self.cursor_pos = boundary;
    }

    fn cycle_agent_filter(&mut self) {
        let agents = self.available_agents();
        match &self.agent_filter {
            None => {
                if let Some(first) = agents.first() {
                    self.agent_filter = Some(first.clone());
                }
            }
            Some(current) => {
                let pos = agents.iter().position(|a| a == current);
                match pos {
                    Some(i) if i + 1 < agents.len() => {
                        self.agent_filter = Some(agents[i + 1].clone());
                    }
                    _ => self.agent_filter = None,
                }
            }
        }
    }

    fn cycle_agent_filter_back(&mut self) {
        let agents = self.available_agents();
        match &self.agent_filter {
            None => {
                if let Some(last) = agents.last() {
                    self.agent_filter = Some(last.clone());
                }
            }
            Some(current) => {
                let pos = agents.iter().position(|a| a == current);
                match pos {
                    Some(0) | None => self.agent_filter = None,
                    Some(i) => self.agent_filter = Some(agents[i - 1].clone()),
                }
            }
        }
    }

    fn available_agents(&self) -> Vec<String> {
        let mut agents: Vec<String> = self
            .agent_counts
            .iter()
            .filter(|(_, count)| **count > 0)
            .map(|(name, _)| name.clone())
            .collect();
        agents.sort();
        agents
    }
}

// ---------------------------------------------------------------------------
// CJK-aware word boundary helpers (jieba for Han, script-class for others)
// Ported from fast-resume's SmartInput._find_prev_word_start
// ---------------------------------------------------------------------------

use std::sync::LazyLock;
static JIEBA: LazyLock<jieba_rs::Jieba> = LazyLock::new(jieba_rs::Jieba::new);

/// CJK ideographs only (the subset jieba can segment).
fn is_han(c: char) -> bool {
    matches!(c as u32,
        0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x20000..=0x2A6DF | 0xF900..=0xFAFF
    )
}

#[derive(PartialEq)]
enum CharClass {
    Space,
    Han,
    Hiragana,
    Katakana,
    Hangul,
    Alpha,
    Punct,
}

fn char_class(c: char) -> CharClass {
    if c.is_whitespace() {
        return CharClass::Space;
    }
    if is_han(c) {
        return CharClass::Han;
    }
    let cp = c as u32;
    if (0x3040..=0x309F).contains(&cp) {
        return CharClass::Hiragana;
    }
    if (0x30A0..=0x30FF).contains(&cp) {
        return CharClass::Katakana;
    }
    if (0xAC00..=0xD7AF).contains(&cp) || (0x1100..=0x11FF).contains(&cp) {
        return CharClass::Hangul;
    }
    if c.is_alphanumeric() || c == '_' {
        return CharClass::Alpha;
    }
    CharClass::Punct
}

/// Find byte position of previous word start (for Ctrl+W / Ctrl+Backspace / Ctrl+Left).
fn find_prev_word_start(text: &str, byte_pos: usize) -> usize {
    let text = &text[..byte_pos];
    let trimmed = text.trim_end();
    if trimmed.is_empty() {
        return 0;
    }

    let last_char = trimmed.chars().next_back().unwrap();

    if is_han(last_char) {
        // Find extent of Han run backwards
        let cjk_end = trimmed.len();
        let mut cjk_start = 0;
        for (i, c) in trimmed.char_indices().rev() {
            if !is_han(c) {
                cjk_start = i + c.len_utf8();
                break;
            }
        }
        // Segment with jieba, walk tokens to find boundary
        let cjk_text = &trimmed[cjk_start..cjk_end];
        let tokens = JIEBA.cut(cjk_text, false);
        let mut offset = cjk_start;
        for token in &tokens {
            let token_end = offset + token.len();
            if token_end >= cjk_end {
                return offset;
            }
            offset = token_end;
        }
        return cjk_start;
    }

    // Non-Han: script-boundary approach
    let cls = char_class(last_char);
    trimmed
        .char_indices()
        .rev()
        .find(|&(_, c)| {
            let cc = char_class(c);
            cc != cls || cc == CharClass::Space || cc == CharClass::Punct
        })
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0)
}

/// Find byte position of next word end (for Ctrl+Right).
fn find_next_word_end(text: &str, byte_pos: usize) -> usize {
    let rest = &text[byte_pos..];
    let after_space = rest.trim_start();
    let space_len = rest.len() - after_space.len();

    if after_space.is_empty() {
        return text.len();
    }

    let first_char = after_space.chars().next().unwrap();

    if is_han(first_char) {
        // Find extent of Han run forwards
        let mut cjk_len = 0;
        for c in after_space.chars() {
            if !is_han(c) {
                break;
            }
            cjk_len += c.len_utf8();
        }
        let tokens = JIEBA.cut(&after_space[..cjk_len], false);
        if let Some(first_token) = tokens.first() {
            return byte_pos + space_len + first_token.len();
        }
        return byte_pos + space_len + cjk_len;
    }

    // Non-Han: script-boundary approach
    let cls = char_class(first_char);
    let end = after_space
        .char_indices()
        .find(|&(_, c)| {
            let cc = char_class(c);
            cc != cls || cc == CharClass::Space || cc == CharClass::Punct
        })
        .map(|(i, _)| i)
        .unwrap_or(after_space.len());
    byte_pos + space_len + end
}
