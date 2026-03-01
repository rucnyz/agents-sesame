pub mod app;
pub mod filter_bar;
pub mod icons;
pub mod keybindings;
pub mod preview;
pub mod results_list;
pub mod theme;
pub mod utils;

use std::io;

use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use tui_scrollbar::{GlyphSet, ScrollBar, ScrollLengths};

use app::{App, FocusedPane};
use filter_bar::FilterBar;
use preview::Preview;
use results_list::ResultsList;
use theme::Theme;

pub fn run_tui(yolo: bool, directory: Option<&str>) -> anyhow::Result<()> {
    // Check if this is the first run (no index cache)
    let index_dir = crate::config::index_dir();
    let first_run = !index_dir.exists();

    // Load icons BEFORE entering alternate screen (queries terminal for protocol support)
    let icon_manager = icons::IconManager::new(&icons::assets_dir());

    let cfg = crate::config::AppConfig::load();
    let kb = keybindings::KeyBindings::load(&cfg.keybindings);
    let theme = Theme::from_config(&cfg.theme);

    // On first run, block until at least one adapter finishes before entering TUI.
    // This way the user sees the progress message in the normal terminal.
    let preloaded = if first_run {
        use crate::search::{LoadingMsg, SessionSearch};
        eprint!("Building session index for the first time, this may take a moment...");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut engine = SessionSearch::new();
            engine.load_progressive(false, &tx);
            let _ = tx.send(LoadingMsg::Done(Box::new(engine)));
        });
        // Wait for the first batch of sessions (one adapter completed)
        let mut first_sessions = Vec::new();
        let mut done_engine = None;
        match rx.recv() {
            Ok(LoadingMsg::Sessions(sessions)) => {
                first_sessions = sessions;
            }
            Ok(LoadingMsg::Done(engine)) => {
                done_engine = Some(engine);
            }
            Err(_) => {}
        }
        eprintln!(" done.");
        Some((first_sessions, done_engine, rx))
    } else {
        None
    };

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    // Enable enhanced keyboard protocol (kitty level 1) so Ctrl+Backspace
    // is properly distinguished from plain Backspace. Terminals that don't
    // support it silently ignore this escape sequence.
    let _ = execute!(
        stdout,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    );
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(yolo, kb, theme);
    app.icons = icon_manager;
    if let Some(dir) = directory {
        app.directory_filter = Some(dir.to_string());
        app.directory_scope = app::DirectoryScope::Local;
    }

    if let Some((sessions, done_engine, rx)) = preloaded {
        // Seed TUI with pre-loaded sessions from the first-run wait
        app.sessions = sessions;
        app.update_agent_counts();
        app.apply_filter();
        if let Some(engine) = done_engine {
            app.search_engine = *engine;
        } else {
            // Still loading — hand over the channel
            app.loading = true;
            app.loading_rx = Some(rx);
        }
    } else {
        app.start_loading();
    };

    let result = run_loop(&mut terminal, &mut app);

    // Restore terminal
    let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result?;

    // Resume session if selected
    if let Some(session) = app.resume_session {
        let cmd = app.search_engine.get_resume_command(&session, app.yolo);
        if !cmd.is_empty() {
            let mut command = std::process::Command::new(&cmd[0]);
            command.args(&cmd[1..]);
            // cd into session directory so agents like Claude Code work correctly
            if !session.directory.is_empty() {
                let dir = std::path::Path::new(&session.directory);
                if dir.is_dir() {
                    command.current_dir(dir);
                }
            }
            let status = command.status()?;
            std::process::exit(status.code().unwrap_or(1));
        }
    }

    Ok(())
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> io::Result<()> {
    let mut needs_redraw = true; // always draw the first frame
    let mut last_size = terminal.size()?;

    loop {
        if app.check_loading() {
            needs_redraw = true;
        }

        // Detect terminal resize even if Event::Resize wasn't fired (Wayland)
        let current_size = terminal.size()?;
        if current_size != last_size {
            last_size = current_size;
            if !app.query.is_empty() {
                app.preview_auto_scroll = true;
            }
            needs_redraw = true;
        }

        if needs_redraw {
            terminal.draw(|f| draw(f, app))?;
            needs_redraw = false;
        }

        if app.handle_events()? {
            needs_redraw = true;
        }

        if app.mouse_toggle_pending {
            app.mouse_toggle_pending = false;
            if app.mouse_captured {
                execute!(terminal.backend_mut(), EnableMouseCapture)?;
                app.status_msg = Some("Mouse: ON (scroll/click)".to_string());
            } else {
                execute!(terminal.backend_mut(), DisableMouseCapture)?;
                app.status_msg = Some("Mouse: OFF (select text)".to_string());
            }
            needs_redraw = true;
        }

        if app.should_quit {
            return Ok(());
        }
    }
}

fn draw(f: &mut ratatui::Frame, app: &mut App) {
    let area = f.area();

    // Main layout: title, search, filter, content, footer
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title bar
            Constraint::Length(3), // search box
            Constraint::Length(1), // filter bar
            Constraint::Min(5),    // main content
            Constraint::Length(1), // footer
        ])
        .split(area);

    draw_title_bar(f, chunks[0], app);
    draw_search_box(f, chunks[1], app);
    app.filter_bar_area = chunks[2];
    draw_filter_bar(f, chunks[2], app);
    draw_content(f, chunks[3], app);
    draw_footer(f, chunks[4], app);
}

fn draw_title_bar(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let count = app.filtered.len();
    let total = app.total_count;
    let sort_label = match app.sort_column {
        app::SortColumn::Relevance => "relevance",
        app::SortColumn::Date => "date",
        app::SortColumn::Agent => "agent",
        app::SortColumn::Title => "title",
        app::SortColumn::Directory => "directory",
        app::SortColumn::Turns => "turns",
    };

    let mut spans = vec![Span::styled(
        " fr-rs ",
        Style::default()
            .fg(theme.primary)
            .add_modifier(Modifier::BOLD),
    )];

    if app.loading {
        spans.push(Span::styled(
            "  Loading sessions...",
            Style::default().fg(theme.tertiary),
        ));
    } else {
        spans.push(Span::styled(
            format!("  {count}/{total} sessions"),
            Style::default().fg(theme.on_surface_variant),
        ));
        spans.push(Span::styled(
            format!("  sort: {sort_label}"),
            Style::default().fg(theme.on_surface_variant),
        ));
        if app.directory_filter.is_some() {
            let (label, color) = match app.directory_scope {
                app::DirectoryScope::Local => ("local", theme.tertiary),
                app::DirectoryScope::Project => ("project", theme.secondary),
                app::DirectoryScope::Global => ("global", theme.on_surface_variant),
            };
            spans.push(Span::styled(
                format!("  scope: {label}"),
                Style::default().fg(color),
            ));
        }
    }

    if let Some(ref msg) = app.status_msg {
        let mut s = spans;
        s.push(Span::styled(
            format!("  {msg}"),
            Style::default().fg(theme.tertiary),
        ));
        f.render_widget(Line::from(s), area);
    } else {
        f.render_widget(Line::from(spans), area);
    }
}

fn draw_search_box(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.primary))
        .title(" Search ");

    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut spans = vec![Span::styled(
        "/ ",
        Style::default().fg(theme.on_surface_variant),
    )];

    if app.query.is_empty() {
        spans.push(Span::styled(
            "Type to search sessions...",
            Style::default().fg(theme.on_surface_variant),
        ));
    } else {
        spans.push(Span::styled(
            &app.query,
            Style::default().fg(theme.on_surface),
        ));
    }

    if let Some(dur) = app.last_search_time {
        let ms = dur.as_millis();
        spans.push(Span::styled(
            format!("  ({ms}ms)"),
            Style::default().fg(theme.on_surface_variant),
        ));
    }

    let line = Paragraph::new(Line::from(spans));
    f.render_widget(line, inner);

    // Show cursor only when results focused (typing active)
    if app.focused_pane == FocusedPane::Results {
        let cursor_x = inner.x
            + 2
            + unicode_width::UnicodeWidthStr::width(&app.query[..app.cursor_pos]) as u16;
        let cursor_y = inner.y;
        if cursor_x < inner.x + inner.width {
            f.set_cursor_position((cursor_x, cursor_y));
        }
    }
}

fn draw_filter_bar(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let filter_bar = FilterBar {
        active: app.agent_filter.as_deref(),
        counts: &app.agent_counts,
        total: app.total_count,
        theme: &app.theme,
    };
    f.render_widget(filter_bar, area);
}

fn draw_content(f: &mut ratatui::Frame, area: Rect, app: &mut App) {
    // Copy theme colors needed for scrollbars to avoid borrow conflicts with &mut app
    let sb_track = app.theme.surface_container;
    let sb_thumb = app.theme.on_surface_variant;
    let results_area;

    if app.show_preview {
        let (direction, constraints) = if app.preview_bottom {
            (
                Direction::Vertical,
                vec![Constraint::Percentage(50), Constraint::Percentage(50)],
            )
        } else {
            (
                Direction::Horizontal,
                vec![Constraint::Percentage(50), Constraint::Percentage(50)],
            )
        };

        let chunks = Layout::default()
            .direction(direction)
            .constraints(constraints)
            .split(area);

        results_area = chunks[0];
        app.results_area = chunks[0];
        app.preview_area = chunks[1];

        let results = ResultsList {
            sessions: &app.filtered,
            query: &app.query,
            focused: app.focused_pane == FocusedPane::Results,
            sort_column: app.sort_column,
            sort_direction: app.sort_direction,
            theme: &app.theme,
        };
        f.render_stateful_widget(results, chunks[0], &mut app.results_state);

        let selected = app.filtered.get(app.results_state.selected);

        let mut badge_lines = Vec::new();
        let mut total_lines: usize = 0;
        let mut rendered_scroll: u16 = 0;
        let preview = Preview {
            session: selected,
            scroll: &mut app.preview_scroll,
            auto_scroll: &mut app.preview_auto_scroll,
            query: &app.query,
            badge_lines: &mut badge_lines,
            total_lines: &mut total_lines,
            rendered_scroll: &mut rendered_scroll,
            focused: app.focused_pane == FocusedPane::Preview,
            theme: &app.theme,
        };
        f.render_widget(preview, chunks[1]);

        // Store total lines for scrollbar drag calculation
        app.preview_total_lines = total_lines;

        let preview_visible = chunks[1].height.saturating_sub(2) as usize;

        // Preview scrollbar (tui-scrollbar with fractional thumb)
        if total_lines > preview_visible {
            let sb_area = Rect {
                x: chunks[1].x + chunks[1].width.saturating_sub(1),
                y: chunks[1].y + 1,
                width: 1,
                height: chunks[1].height.saturating_sub(2),
            };
            let scrollbar = ScrollBar::vertical(ScrollLengths {
                content_len: total_lines,
                viewport_len: preview_visible,
            })
            .offset(app.preview_scroll as usize)
            .glyph_set(GlyphSet::unicode())
            .track_style(Style::default().fg(sb_track))
            .thumb_style(Style::default().fg(sb_thumb));
            f.render_widget(&scrollbar, sb_area);
            app.preview_scrollbar = Some(scrollbar);
            app.preview_sb_area = sb_area;
        } else {
            app.preview_scrollbar = None;
        }

        // Overlay agent icons on preview badge lines
        if let Some(session) = selected {
            let agent = session.agent.clone();
            draw_preview_icons(f, chunks[1], &badge_lines, rendered_scroll, &agent, app);
        }
    } else {
        results_area = area;
        app.results_area = area;
        app.preview_area = Rect::default();

        let results = ResultsList {
            sessions: &app.filtered,
            query: &app.query,
            focused: true, // always focused when preview hidden
            sort_column: app.sort_column,
            sort_direction: app.sort_direction,
            theme: &app.theme,
        };
        f.render_stateful_widget(results, area, &mut app.results_state);
    }

    // Results list scrollbar (tui-scrollbar with fractional thumb)
    let results_visible = results_area.height.saturating_sub(3) as usize; // borders + header
    if app.filtered.len() > results_visible {
        let sb_area = Rect {
            x: results_area.x + results_area.width.saturating_sub(1),
            y: results_area.y + 1,
            width: 1,
            height: results_area.height.saturating_sub(2),
        };
        let scrollbar = ScrollBar::vertical(ScrollLengths {
            content_len: app.filtered.len(),
            viewport_len: results_visible,
        })
        .offset(app.results_state.offset)
        .glyph_set(GlyphSet::unicode())
        .track_style(Style::default().fg(sb_track))
        .thumb_style(Style::default().fg(sb_thumb));
        f.render_widget(&scrollbar, sb_area);
        app.results_scrollbar = Some(scrollbar);
        app.results_sb_area = sb_area;
    } else {
        app.results_scrollbar = None;
    }

    // Render agent icons overlay on results list rows
    draw_agent_icons(f, results_area, app);
}

fn draw_agent_icons(f: &mut ratatui::Frame, results_area: Rect, app: &mut App) {
    let Some(ref mut icons) = app.icons else {
        return;
    };

    // Results area has 1px border + 1 header row = content starts at y+2, x+1
    let content_x = results_area.x + 1;
    let content_y = results_area.y + 2;
    let visible_rows = results_area.height.saturating_sub(3) as usize; // borders + header

    for (i, session) in app
        .filtered
        .iter()
        .skip(app.results_state.offset)
        .take(visible_rows)
        .enumerate()
    {
        let row_y = content_y + i as u16;
        if row_y >= results_area.y + results_area.height - 1 {
            break;
        }

        if !icons.has_icon(&session.agent) {
            continue;
        }

        let icon_rect = Rect {
            x: content_x,
            y: row_y,
            width: 2,
            height: 1,
        };

        if let Some(protocol) = icons.get_protocol(&session.agent, icon_rect) {
            let img = ratatui_image::StatefulImage::default();
            f.render_stateful_widget(img, icon_rect, protocol);
        }
    }
}

fn draw_preview_icons(
    f: &mut ratatui::Frame,
    preview_area: Rect,
    badge_lines: &[usize],
    scroll: u16,
    agent: &str,
    app: &mut App,
) {
    let Some(ref mut icons) = app.icons else {
        return;
    };

    if !icons.has_icon(agent) {
        return;
    }

    // Preview has a 1-cell border on each side
    let content_x = preview_area.x + 1;
    let content_y = preview_area.y + 1;
    let visible_height = preview_area.height.saturating_sub(2) as usize; // top + bottom border

    for &line_idx in badge_lines {
        // Convert logical line index to physical row (assuming 1:1 without wrap)
        let physical_row = line_idx as i32 - scroll as i32;
        if physical_row < 0 || physical_row as usize >= visible_height {
            continue;
        }

        let row_y = content_y + physical_row as u16;
        if row_y >= preview_area.y + preview_area.height - 1 {
            break;
        }

        let icon_rect = Rect {
            x: content_x,
            y: row_y,
            width: 2,
            height: 1,
        };

        if let Some(protocol) = icons.get_protocol(agent, icon_rect) {
            let img = ratatui_image::StatefulImage::default();
            f.render_stateful_widget(img, icon_rect, protocol);
        }
    }
}

fn draw_footer(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let theme = &app.theme;
    let key = Style::default().fg(theme.primary);
    let dim = Style::default().fg(theme.on_surface_variant);

    let spans = if app.focused_pane == FocusedPane::Preview {
        vec![
            Span::styled(" ↑↓", key),
            Span::styled(" scroll ", dim),
            Span::styled(" Home/End", key),
            Span::styled(" top/btm ", dim),
            Span::styled(" c", key),
            Span::styled(" copy ", dim),
            Span::styled(" ^T", key),
            Span::styled(" results ", dim),
            Span::styled(" Enter", key),
            Span::styled(" resume ", dim),
            Span::styled(" Tab", key),
            Span::styled(" filter ", dim),
            Span::styled(" Esc/^Q", key),
            Span::styled(" quit", dim),
        ]
    } else {
        vec![
            Span::styled(" ↑↓", key),
            Span::styled(" nav ", dim),
            Span::styled(" ^T", key),
            Span::styled(" preview ", dim),
            Span::styled(" Enter/2×click", key),
            Span::styled(" resume ", dim),
            Span::styled(" Tab", key),
            Span::styled(" filter ", dim),
            Span::styled(" ^S", key),
            Span::styled(" sort ", dim),
            Span::styled(" ^`", key),
            Span::styled(" toggle ", dim),
            Span::styled(" ^P", key),
            Span::styled(" layout ", dim),
            Span::styled(" ^D", key),
            Span::styled(" scope ", dim),
            Span::styled(" Esc/^Q", key),
            Span::styled(" quit", dim),
        ]
    };
    f.render_widget(Line::from(spans), area);
}
