use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, StatefulWidget, Widget};
use unicode_width::UnicodeWidthStr;

use crate::config;
use crate::session::Session;

use super::utils::{
    format_directory, format_time_ago, get_age_color, highlight_spans, pad_to_width,
    truncate_to_width,
};

pub struct ResultsList<'a> {
    pub sessions: &'a [Session],
    pub query: &'a str,
    pub focused: bool,
}

#[derive(Default)]
pub struct ResultsState {
    pub selected: usize,
    pub offset: usize,
}

impl ResultsState {
    pub fn select_next(&mut self, total: usize) {
        if total == 0 {
            return;
        }
        self.selected = (self.selected + 1).min(total - 1);
    }

    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn select_first(&mut self) {
        self.selected = 0;
    }

    pub fn page_down(&mut self, page_size: usize, total: usize) {
        if total == 0 {
            return;
        }
        self.selected = (self.selected + page_size).min(total - 1);
    }

    pub fn page_up(&mut self, page_size: usize) {
        self.selected = self.selected.saturating_sub(page_size);
    }

    fn ensure_visible(&mut self, visible_rows: usize) {
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + visible_rows {
            self.offset = self.selected.saturating_sub(visible_rows - 1);
        }
    }
}

impl StatefulWidget for ResultsList<'_> {
    type State = ResultsState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let border_color = if self.focused {
            Color::Rgb(232, 123, 53)
        } else {
            Color::DarkGray
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height < 2 || inner.width < 20 {
            return;
        }

        let width = inner.width as usize;
        let visible_rows = (inner.height as usize).saturating_sub(1); // 1 for header

        state.ensure_visible(visible_rows);

        // Column widths based on terminal width
        let (agent_w, dir_w, turns_w, date_w, show_dir) = if width >= 120 {
            (10, 28, 6, 14, true)
        } else if width >= 90 {
            (10, 22, 5, 12, true)
        } else if width >= 60 {
            (10, 16, 5, 10, true)
        } else {
            (10, 0, 4, 10, false)
        };
        let title_w = width
            .saturating_sub(agent_w + turns_w + date_w + 3) // 3 for separators
            .saturating_sub(if show_dir { dir_w + 1 } else { 0 });

        // Header
        let mut header_spans = vec![
            Span::styled(
                pad_to_width("Agent", agent_w),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                pad_to_width("Title", title_w),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
        ];
        if show_dir {
            header_spans.push(Span::styled(
                pad_to_width("Directory", dir_w),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            ));
            header_spans.push(Span::raw(" "));
        }
        header_spans.push(Span::styled(
            pad_to_width("Trn", turns_w),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ));
        header_spans.push(Span::raw(" "));
        header_spans.push(Span::styled(
            pad_to_width("Date", date_w),
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        ));

        let header_line = Line::from(header_spans);
        let header_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };
        header_line.render(header_area, buf);

        // Rows
        let now = chrono::Local::now().naive_local();
        for (i, session) in self
            .sessions
            .iter()
            .skip(state.offset)
            .take(visible_rows)
            .enumerate()
        {
            let row_y = inner.y + 1 + i as u16;
            let row_idx = state.offset + i;
            let is_selected = row_idx == state.selected;

            let agent_color = config::get_agent_config(&session.agent)
                .map(|c| parse_hex_color(c.color))
                .unwrap_or(Color::White);

            let badge = config::get_agent_config(&session.agent)
                .map(|c| c.badge)
                .unwrap_or(session.agent.as_str());

            let age_hours =
                now.signed_duration_since(session.timestamp).num_seconds() as f64 / 3600.0;
            let date_color = get_age_color(age_hours);

            let title_display = truncate_to_width(&session.title, title_w);
            let date_display = format_time_ago(session.timestamp);
            let turns_display = if session.message_count > 0 {
                format!("{}", session.message_count)
            } else {
                "-".to_string()
            };

            // Agent column: "● badge" with colored dot
            let agent_display = format!("● {badge}");
            let mut row_spans = vec![
                Span::styled(
                    pad_to_width(&agent_display, agent_w),
                    Style::default().fg(agent_color),
                ),
                Span::raw(" "),
            ];

            // Title column with query highlighting
            let title_spans = highlight_spans(&title_display, self.query, Color::White);
            let title_used: usize = title_spans.iter().map(|s| s.content.width()).sum();
            row_spans.extend(title_spans);
            // Pad remaining width
            if title_used < title_w {
                row_spans.push(Span::raw(" ".repeat(title_w - title_used)));
            }
            row_spans.push(Span::raw(" "));

            if show_dir {
                let dir_display = format_directory(&session.directory, dir_w);
                row_spans.push(Span::styled(
                    pad_to_width(&dir_display, dir_w),
                    Style::default().fg(Color::DarkGray),
                ));
                row_spans.push(Span::raw(" "));
            }

            row_spans.push(Span::styled(
                pad_to_width(&turns_display, turns_w),
                Style::default().fg(Color::DarkGray),
            ));
            row_spans.push(Span::raw(" "));
            row_spans.push(Span::styled(
                pad_to_width(&date_display, date_w),
                Style::default().fg(date_color),
            ));

            let row_area = Rect {
                x: inner.x,
                y: row_y,
                width: inner.width,
                height: 1,
            };

            if is_selected {
                // Fill background for selection
                for x in row_area.x..row_area.x + row_area.width {
                    if let Some(cell) = buf.cell_mut((x, row_y)) {
                        cell.set_style(Style::default().bg(Color::Rgb(40, 40, 60)));
                    }
                }
                let line = Line::from(row_spans);
                let styled_line = line.style(Style::default().bg(Color::Rgb(40, 40, 60)));
                styled_line.render(row_area, buf);
            } else {
                Line::from(row_spans).render(row_area, buf);
            }
        }
    }
}

fn parse_hex_color(hex: &str) -> Color {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return Color::White;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(255);
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(255);
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(255);
    Color::Rgb(r, g, b)
}
