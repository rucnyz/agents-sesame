pub mod app;
pub mod filter_bar;
pub mod icons;
pub mod keybindings;
pub mod preview;
pub mod results_list;
pub mod utils;

use std::io;

use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use tui_scrollbar::{GlyphSet, ScrollBar, ScrollLengths};

use app::{App, FocusedPane};
use filter_bar::FilterBar;
use preview::Preview;
use results_list::ResultsList;

pub fn run_tui(yolo: bool, directory: Option<&str>) -> anyhow::Result<()> {
    // Load icons BEFORE entering alternate screen (queries terminal for protocol support)
    let icon_manager = icons::IconManager::new(&icons::assets_dir());

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let cfg = crate::config::AppConfig::load();
    let kb = keybindings::KeyBindings::load(&cfg.keybindings);

    let mut app = App::new(yolo, kb);
    app.icons = icon_manager;
    if let Some(dir) = directory {
        app.directory_filter = Some(dir.to_string());
        app.directory_scope = app::DirectoryScope::Local;
    }
    app.start_loading();

    let result = run_loop(&mut terminal, &mut app);

    // Restore terminal
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
    let count = app.filtered.len();
    let total = app.total_count;
    let sort_label = if app.sort_by_time {
        "time"
    } else {
        "relevance"
    };

    let mut spans = vec![Span::styled(
        " fr-rs ",
        Style::default()
            .fg(Color::Rgb(232, 123, 53))
            .add_modifier(Modifier::BOLD),
    )];

    if app.loading {
        spans.push(Span::styled(
            "  Loading sessions...",
            Style::default().fg(Color::Yellow),
        ));
    } else {
        spans.push(Span::styled(
            format!("  {count}/{total} sessions"),
            Style::default().fg(Color::DarkGray),
        ));
        spans.push(Span::styled(
            format!("  sort: {sort_label}"),
            Style::default().fg(Color::DarkGray),
        ));
        if app.directory_filter.is_some() {
            let (label, color) = match app.directory_scope {
                app::DirectoryScope::Local => ("local", Color::Rgb(100, 255, 100)),
                app::DirectoryScope::Project => ("project", Color::Rgb(100, 200, 255)),
                app::DirectoryScope::Global => ("global", Color::DarkGray),
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
            Style::default().fg(Color::Green),
        ));
        f.render_widget(Line::from(s), area);
    } else {
        f.render_widget(Line::from(spans), area);
    }
}

fn draw_search_box(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(232, 123, 53)))
        .title(" Search ");

    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut spans = vec![Span::styled("/ ", Style::default().fg(Color::DarkGray))];

    if app.query.is_empty() {
        spans.push(Span::styled(
            "Type to search sessions...",
            Style::default().fg(Color::DarkGray),
        ));
    } else {
        spans.push(Span::styled(&app.query, Style::default().fg(Color::White)));
    }

    if let Some(dur) = app.last_search_time {
        let ms = dur.as_millis();
        spans.push(Span::styled(
            format!("  ({ms}ms)"),
            Style::default().fg(Color::DarkGray),
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
    };
    f.render_widget(filter_bar, area);
}

fn draw_content(f: &mut ratatui::Frame, area: Rect, app: &mut App) {
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
        };
        f.render_stateful_widget(results, chunks[0], &mut app.results_state);

        let selected = app.filtered.get(app.results_state.selected);

        let mut badge_lines = Vec::new();
        let mut total_lines: usize = 0;
        let mut first_match_row: Option<usize> = None;
        let preview = Preview {
            session: selected,
            scroll: app.preview_scroll,
            query: &app.query,
            badge_lines: &mut badge_lines,
            total_lines: &mut total_lines,
            first_match_row: &mut first_match_row,
            focused: app.focused_pane == FocusedPane::Preview,
        };
        f.render_widget(preview, chunks[1]);

        // Store total lines for scrollbar drag calculation
        app.preview_total_lines = total_lines;

        // Auto-scroll to first match if needed
        let preview_visible = chunks[1].height.saturating_sub(2) as usize;
        if app.preview_auto_scroll {
            app.preview_auto_scroll = false;
            if let Some(match_row) = first_match_row {
                // Scroll so the match is ~3 lines from the top
                app.preview_scroll = match_row.saturating_sub(3) as u16;
            }
        }

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
            .track_style(Style::default().fg(Color::Rgb(60, 60, 60)))
            .thumb_style(Style::default().fg(Color::DarkGray));
            f.render_widget(&scrollbar, sb_area);
            app.preview_scrollbar = Some(scrollbar);
            app.preview_sb_area = sb_area;
        } else {
            app.preview_scrollbar = None;
        }

        // Overlay agent icons on preview badge lines
        if let Some(session) = selected {
            let agent = session.agent.clone();
            draw_preview_icons(f, chunks[1], &badge_lines, app.preview_scroll, &agent, app);
        }
    } else {
        results_area = area;
        app.results_area = area;
        app.preview_area = Rect::default();

        let results = ResultsList {
            sessions: &app.filtered,
            query: &app.query,
            focused: true, // always focused when preview hidden
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
        .track_style(Style::default().fg(Color::Rgb(60, 60, 60)))
        .thumb_style(Style::default().fg(Color::DarkGray));
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
    let key = Style::default().fg(Color::Rgb(232, 123, 53));
    let dim = Style::default().fg(Color::DarkGray);

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
