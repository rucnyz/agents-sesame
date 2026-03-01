use std::collections::HashMap;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;

use crate::config;

/// Order of agent filter keys (None = "All").
const FILTER_ORDER: &[&str] = &[
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

pub struct FilterBar<'a> {
    pub active: Option<&'a str>,
    pub counts: &'a HashMap<String, usize>,
    pub total: usize,
}

impl Widget for FilterBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut spans = Vec::new();

        // "All" button
        let all_label = format!(" All({}) ", self.total);
        let all_style = if self.active.is_none() {
            Style::default()
                .fg(Color::Black)
                .bg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled(all_label, all_style));
        spans.push(Span::raw(" "));

        // Agent buttons
        for &agent in FILTER_ORDER {
            let count = self.counts.get(agent).copied().unwrap_or(0);
            if count == 0 {
                continue;
            }

            let color = config::get_agent_config(agent)
                .map(|c| parse_hex_color(c.color))
                .unwrap_or(Color::White);

            let badge = config::get_agent_config(agent)
                .map(|c| c.badge)
                .unwrap_or(agent);

            let label = format!(" {badge}({count}) ");

            let style = if self.active == Some(agent) {
                Style::default()
                    .fg(Color::Black)
                    .bg(color)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(color)
            };
            spans.push(Span::styled(label, style));
            spans.push(Span::raw(" "));
        }

        let line = Line::from(spans);
        line.render(area, buf);
    }
}

/// Hit-test: given an x coordinate within the filter bar area,
/// return which filter was clicked.
/// - `Some(None)` → "All" clicked
/// - `Some(Some(agent))` → specific agent clicked
/// - `None` → nothing clicked
pub fn filter_hit_test(
    x: u16,
    area: Rect,
    counts: &HashMap<String, usize>,
    total: usize,
) -> Option<Option<String>> {
    if x < area.x || x >= area.x + area.width {
        return None;
    }

    let rel_x = (x - area.x) as usize;
    let mut pos: usize = 0;

    // "All" button: " All(N) " + " " separator
    let all_label = format!(" All({total}) ");
    let all_width = all_label.width();
    if rel_x < pos + all_width {
        return Some(None);
    }
    pos += all_width + 1; // +1 for separator space

    // Agent buttons
    for &agent in FILTER_ORDER {
        let count = counts.get(agent).copied().unwrap_or(0);
        if count == 0 {
            continue;
        }
        let badge = config::get_agent_config(agent)
            .map(|c| c.badge)
            .unwrap_or(agent);
        let label = format!(" {badge}({count}) ");
        let label_width = label.width();
        if rel_x < pos + label_width {
            return Some(Some(agent.to_string()));
        }
        pos += label_width + 1; // +1 for separator space
    }

    None
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
