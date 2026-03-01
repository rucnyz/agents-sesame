use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget, Wrap};

use crate::config;
use crate::session::Session;

pub struct Preview<'a> {
    pub session: Option<&'a Session>,
    pub scroll: u16,
    pub query: &'a str,
    /// Output: physical row positions (pre-scroll, accounting for wrap) for icon overlay.
    pub badge_lines: &'a mut Vec<usize>,
}

impl Widget for Preview<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .title(" Preview ");

        let Some(session) = self.session else {
            let empty = Paragraph::new("No session selected")
                .block(block)
                .style(Style::default().fg(Color::DarkGray));
            empty.render(area, buf);
            return;
        };

        let agent_color = config::get_agent_config(&session.agent)
            .map(|c| parse_hex_color(c.color))
            .unwrap_or(Color::White);
        let agent_badge = config::get_agent_config(&session.agent)
            .map(|c| c.badge)
            .unwrap_or(&session.agent);

        // Extract preview content — show context around match if query given
        let preview_text = extract_preview_content(&session.content, self.query);

        // Build lines from content
        let (lines, badge_indices) =
            build_preview_lines(&preview_text, self.query, agent_color, agent_badge);

        // Convert logical badge indices to physical row positions (accounting for wrap)
        let inner_width = block.inner(area).width as usize;
        let mut physical_row: usize = 0;
        let mut physical_badge_positions = Vec::new();
        for (i, line) in lines.iter().enumerate() {
            if badge_indices.contains(&i) {
                physical_badge_positions.push(physical_row);
            }
            let line_width = line.width();
            let rows = if line_width == 0 || inner_width == 0 {
                1
            } else {
                line_width.div_ceil(inner_width)
            };
            physical_row += rows;
        }
        *self.badge_lines = physical_badge_positions;

        let paragraph = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((self.scroll, 0));

        paragraph.render(area, buf);
    }
}

/// Extract the relevant portion of content for preview.
/// If query matches, scroll to show context around the match.
fn extract_preview_content(content: &str, _query: &str) -> String {
    // No truncation — show full content, let the user scroll
    content.to_string()
}

/// Build styled lines from preview text, matching Python's _render_message logic.
/// Returns (lines, badge_line_indices) where badge_line_indices are the line numbers
/// of assistant first-lines (for icon overlay).
fn build_preview_lines(
    text: &str,
    query: &str,
    agent_color: Color,
    agent_badge: &str,
) -> (Vec<Line<'static>>, Vec<usize>) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut badge_indices: Vec<usize> = Vec::new();
    let messages = text.split("\n\n");

    for msg in messages {
        let msg = msg.trim_end();
        if msg.trim().is_empty() {
            lines.push(Line::from(""));
            continue;
        }

        let msg_lines: Vec<&str> = msg.split('\n').collect();
        let is_user = msg.starts_with("» ");
        let mut first_line = true;
        let mut i = 0;

        while i < msg_lines.len() {
            let line = msg_lines[i];

            // Check for code block start: ```language
            if line.starts_with("```") {
                // Collect code block content
                i += 1;
                while i < msg_lines.len() && !msg_lines[i].starts_with("```") {
                    // Render code lines with dim style and indent
                    let code_line = msg_lines[i];
                    lines.push(Line::from(vec![
                        Span::styled("  ", Style::default()),
                        Span::styled(code_line.to_string(), Style::default().fg(Color::DarkGray)),
                    ]));
                    i += 1;
                }
                // Skip closing ```
                if i < msg_lines.len() && msg_lines[i].starts_with("```") {
                    i += 1;
                }
                continue;
            }

            if let Some(content) = line.strip_prefix("» ") {
                // User message
                let content = if content.chars().count() > 200 {
                    let truncated: String = content.chars().take(200).collect();
                    format!("{truncated} ...")
                } else {
                    content.to_string()
                };
                let mut spans = vec![Span::styled(
                    "» ".to_string(),
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )];
                spans.extend(highlight_spans(&content, query, Color::Cyan));
                lines.push(Line::from(spans));
                first_line = false;
            } else if line == "..." {
                lines.push(Line::from(Span::styled(
                    "   ⋯".to_string(),
                    Style::default().fg(Color::DarkGray),
                )));
            } else if line.starts_with("...") {
                lines.push(Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(Color::DarkGray),
                )));
            } else if line.starts_with("  ") || (!is_user && !line.is_empty()) {
                // Assistant response
                if first_line {
                    let content = line.trim_start();
                    badge_indices.push(lines.len());
                    // Leave space for icon overlay: "   " (3 chars) + badge + content
                    let mut spans = vec![
                        Span::styled("   ".to_string(), Style::default()), // icon space
                        Span::styled(
                            format!("{agent_badge} "),
                            Style::default()
                                .fg(agent_color)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ];
                    spans.extend(highlight_spans(content, query, Color::White));
                    lines.push(Line::from(spans));
                    first_line = false;
                } else {
                    let spans = highlight_spans(line, query, Color::White);
                    lines.push(Line::from(spans));
                }
            } else if !line.is_empty() {
                let spans = highlight_spans(line, query, Color::White);
                lines.push(Line::from(spans));
            }

            i += 1;
        }

        // Add blank line between messages
        lines.push(Line::from(""));
    }

    (lines, badge_indices)
}

/// Highlight query terms in text, returning owned Spans.
fn highlight_spans(text: &str, query: &str, base_color: Color) -> Vec<Span<'static>> {
    let base_style = Style::default().fg(base_color);

    if query.is_empty() || text.is_empty() {
        return vec![Span::styled(text.to_string(), base_style)];
    }

    let highlight_style = base_style.add_modifier(Modifier::BOLD | Modifier::REVERSED);
    let lower_text = text.to_lowercase();
    let terms: Vec<String> = query.split_whitespace().map(|t| t.to_lowercase()).collect();

    if terms.is_empty() {
        return vec![Span::styled(text.to_string(), base_style)];
    }

    // Find all match positions (byte indices, safe because lowercase preserves boundaries for CJK)
    let mut matches: Vec<(usize, usize)> = Vec::new();
    for term in &terms {
        let mut start = 0;
        while start < lower_text.len() {
            let Some(pos) = lower_text[start..].find(term.as_str()) else {
                break;
            };
            let abs_pos = start + pos;
            let end = abs_pos + term.len();
            matches.push((abs_pos, end));
            // Advance past match start by one full character
            start = abs_pos
                + lower_text[abs_pos..]
                    .chars()
                    .next()
                    .map(|c| c.len_utf8())
                    .unwrap_or(1);
        }
    }

    if matches.is_empty() {
        return vec![Span::styled(text.to_string(), base_style)];
    }

    // Sort and merge overlapping
    matches.sort_by_key(|m| m.0);
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for m in matches {
        if let Some(last) = merged.last_mut()
            && m.0 <= last.1
        {
            last.1 = last.1.max(m.1);
            continue;
        }
        merged.push(m);
    }

    // Build spans
    let mut spans = Vec::new();
    let mut pos = 0;
    for (s, e) in merged {
        if s > pos {
            spans.push(Span::styled(text[pos..s].to_string(), base_style));
        }
        spans.push(Span::styled(text[s..e].to_string(), highlight_style));
        pos = e;
    }
    if pos < text.len() {
        spans.push(Span::styled(text[pos..].to_string(), base_style));
    }

    spans
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
