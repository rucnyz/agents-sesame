use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget, Wrap};
use unicode_width::UnicodeWidthChar;

use crate::config;
use crate::session::Session;

use super::theme::Theme;
use super::utils::highlight_spans;

pub struct Preview<'a> {
    pub session: Option<&'a Session>,
    pub scroll: u16,
    pub query: &'a str,
    /// Output: physical row positions (pre-scroll, accounting for wrap) for icon overlay.
    pub badge_lines: &'a mut Vec<usize>,
    /// Output: total physical rows (for scrollbar).
    pub total_lines: &'a mut usize,
    /// Output: physical row of first query match (for auto-scroll).
    pub first_match_row: &'a mut Option<usize>,
    pub focused: bool,
    pub theme: &'a Theme,
}

impl Widget for Preview<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let theme = self.theme;
        let border_color = if self.focused {
            theme.primary
        } else {
            theme.on_surface_variant
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .title(" Preview ");

        let Some(session) = self.session else {
            let empty = Paragraph::new("No session selected")
                .block(block)
                .style(Style::default().fg(theme.on_surface_variant));
            empty.render(area, buf);
            return;
        };

        let agent_color = config::get_agent_config(&session.agent)
            .map(|c| parse_hex_color(c.color))
            .unwrap_or(theme.on_surface);
        let agent_badge = config::get_agent_config(&session.agent)
            .map(|c| c.badge)
            .unwrap_or(&session.agent);

        // Extract preview content — show context around match if query given
        let preview_text = extract_preview_content(&session.content, self.query);

        // Build lines from content
        let (lines, badge_indices, first_match_logical) =
            build_preview_lines(&preview_text, self.query, agent_color, agent_badge, theme);

        // Convert logical line indices to physical row positions (accounting for wrap)
        let inner_width = block.inner(area).width as usize;
        let mut physical_row: usize = 0;
        let mut physical_badge_positions = Vec::new();
        let mut first_match_physical: Option<usize> = None;
        for (i, line) in lines.iter().enumerate() {
            if badge_indices.contains(&i) {
                physical_badge_positions.push(physical_row);
            }
            if first_match_logical == Some(i) {
                first_match_physical = Some(physical_row);
            }
            let rows = wrapped_line_count(line, inner_width);
            physical_row += rows;
        }
        *self.badge_lines = physical_badge_positions;
        *self.total_lines = physical_row;
        *self.first_match_row = first_match_physical;

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
/// Returns (lines, badge_line_indices, first_match_line) where badge_line_indices are the line numbers
/// of assistant first-lines (for icon overlay), and first_match_line is the logical line index
/// of the first highlighted match.
fn build_preview_lines(
    text: &str,
    query: &str,
    agent_color: Color,
    agent_badge: &str,
    theme: &Theme,
) -> (Vec<Line<'static>>, Vec<usize>, Option<usize>) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut badge_indices: Vec<usize> = Vec::new();
    let mut first_match_line: Option<usize> = None;
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
                        Span::styled(
                            code_line.to_string(),
                            Style::default().fg(theme.on_surface_variant),
                        ),
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
                        .fg(theme.secondary)
                        .add_modifier(Modifier::BOLD),
                )];
                let hl = highlight_spans(&content, query, theme.secondary);
                let has_match = hl.len() > 1;
                spans.extend(hl);
                lines.push(Line::from(spans));
                if has_match && first_match_line.is_none() {
                    first_match_line = Some(lines.len() - 1);
                }
                first_line = false;
            } else if line == "..." {
                lines.push(Line::from(Span::styled(
                    "   ⋯".to_string(),
                    Style::default().fg(theme.on_surface_variant),
                )));
            } else if line.starts_with("...") {
                lines.push(Line::from(Span::styled(
                    line.to_string(),
                    Style::default().fg(theme.on_surface_variant),
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
                    let hl = highlight_spans(content, query, theme.on_surface);
                    let has_match = hl.len() > 1;
                    spans.extend(hl);
                    lines.push(Line::from(spans));
                    if has_match && first_match_line.is_none() {
                        first_match_line = Some(lines.len() - 1);
                    }
                    first_line = false;
                } else {
                    let spans = highlight_spans(line, query, theme.on_surface);
                    if spans.len() > 1 && first_match_line.is_none() {
                        first_match_line = Some(lines.len());
                    }
                    lines.push(Line::from(spans));
                }
            } else if !line.is_empty() {
                let spans = highlight_spans(line, query, theme.on_surface);
                if spans.len() > 1 && first_match_line.is_none() {
                    first_match_line = Some(lines.len());
                }
                lines.push(Line::from(spans));
            }

            i += 1;
        }

        // Add blank line between messages
        lines.push(Line::from(""));
    }

    (lines, badge_indices, first_match_line)
}

/// Count physical rows a Line occupies when wrapped to `max_width` cells,
/// matching ratatui's `Wrap { trim: false }` behavior: wide chars that don't
/// fit on the current row start a new row instead of being split.
fn wrapped_line_count(line: &Line, max_width: usize) -> usize {
    if max_width == 0 {
        return 1;
    }
    let mut rows: usize = 1;
    let mut col: usize = 0;
    for span in &line.spans {
        for ch in span.content.chars() {
            let w = ch.width().unwrap_or(0);
            if w == 0 {
                continue;
            }
            if col + w > max_width && col > 0 {
                rows += 1;
                col = w;
            } else {
                col += w;
            }
        }
    }
    rows
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

#[cfg(test)]
mod tests {
    use super::*;

    fn line_from(text: &str) -> Line<'static> {
        Line::from(text.to_string())
    }

    fn styled_line(spans: Vec<(&str, Color)>) -> Line<'static> {
        Line::from(
            spans
                .into_iter()
                .map(|(t, c)| Span::styled(t.to_string(), Style::default().fg(c)))
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn wrap_empty_line() {
        let line = line_from("");
        assert_eq!(wrapped_line_count(&line, 80), 1);
    }

    #[test]
    fn wrap_short_ascii() {
        let line = line_from("hello world");
        assert_eq!(wrapped_line_count(&line, 80), 1);
    }

    #[test]
    fn wrap_exact_fit() {
        // 10 chars in width 10 → 1 row
        let line = line_from("abcdefghij");
        assert_eq!(wrapped_line_count(&line, 10), 1);
    }

    #[test]
    fn wrap_one_over() {
        // 11 chars in width 10 → 2 rows
        let line = line_from("abcdefghijk");
        assert_eq!(wrapped_line_count(&line, 10), 2);
    }

    #[test]
    fn wrap_double_width_exact() {
        // 5 Chinese chars = 10 cells, width 10 → 1 row
        let line = line_from("你好世界啊");
        assert_eq!(wrapped_line_count(&line, 10), 1);
    }

    #[test]
    fn wrap_double_width_boundary() {
        // 5 Chinese chars = 10 cells, width 9 → the 5th char (col 8+2=10 > 9) wraps
        // Row 1: 4 chars (8 cells), Row 2: 1 char (2 cells) → 2 rows
        let line = line_from("你好世界啊");
        assert_eq!(wrapped_line_count(&line, 9), 2);
    }

    #[test]
    fn wrap_double_width_odd_boundary() {
        // 3 Chinese chars = 6 cells, width 5
        // Row 1: 2 chars (4 cells), can't fit 3rd (4+2=6 > 5) → wrap
        // Row 2: 1 char (2 cells) → 2 rows
        let line = line_from("你好世");
        assert_eq!(wrapped_line_count(&line, 5), 2);
    }

    #[test]
    fn wrap_mixed_ascii_cjk() {
        // "a你b" = 1 + 2 + 1 = 4 cells, width 3
        // Row 1: 'a' (1 cell), '你' (1+2=3, fits) → 3 cells
        // Row 2: 'b' (1 cell) → 2 rows
        let line = line_from("a你b");
        assert_eq!(wrapped_line_count(&line, 3), 2);
    }

    #[test]
    fn wrap_cjk_pushes_to_next_row() {
        // "ab你" = 1 + 1 + 2 = 4 cells, width 3
        // Row 1: 'a' (1), 'b' (2), '你' would be 2+2=4 > 3 → wrap
        // Row 2: '你' (2) → 2 rows
        let line = line_from("ab你");
        assert_eq!(wrapped_line_count(&line, 3), 2);
    }

    #[test]
    fn wrap_multiple_spans() {
        // "» " (2 cells) + "你好世界" (8 cells) = 10 cells, width 6
        // Row 1: '»' (1) + ' ' (2) + '你' (4) + '好' would be 4+2=6, fits → 6 cells
        // Row 2: '世' (2) + '界' (4) → 4 cells → 2 rows
        let line = styled_line(vec![("» ", Color::Cyan), ("你好世界", Color::White)]);
        assert_eq!(wrapped_line_count(&line, 6), 2);
    }

    #[test]
    fn wrap_zero_width() {
        let line = line_from("hello");
        assert_eq!(wrapped_line_count(&line, 0), 1);
    }

    #[test]
    fn wrap_long_line() {
        // 100 ASCII chars in width 10 → 10 rows
        let line = line_from(&"a".repeat(100));
        assert_eq!(wrapped_line_count(&line, 10), 10);
    }

    #[test]
    fn wrap_long_cjk_line() {
        // 20 Chinese chars = 40 cells, width 11
        // Each row fits 5 chars (10 cells), 6th would be 10+2=12 > 11
        // 20 / 5 = 4 rows
        let line = line_from(&"你".repeat(20));
        assert_eq!(wrapped_line_count(&line, 11), 4);
    }

    #[test]
    fn wrap_width_1_ascii() {
        // Each ASCII char takes 1 row
        let line = line_from("abc");
        assert_eq!(wrapped_line_count(&line, 1), 3);
    }

    #[test]
    fn wrap_width_1_cjk() {
        // Each CJK char is 2 cells wide, but width is 1
        // '你': col=0, 0+2=2 > 1, so wrap → row 2, col=2
        // But col=2 > 1, so this is tricky... Actually:
        // Start: rows=1, col=0
        // '你': w=2, col(0)+2=2 > 1, rows=2, col=2
        // '好': w=2, col(2)+2=4 > 1, rows=3, col=2
        // Each CJK char wraps to its own row
        let line = line_from("你好世");
        assert_eq!(wrapped_line_count(&line, 1), 3);
    }

    #[test]
    fn parse_hex_color_valid() {
        assert_eq!(parse_hex_color("#E87B35"), Color::Rgb(232, 123, 53));
        assert_eq!(parse_hex_color("4285F4"), Color::Rgb(66, 133, 244));
    }

    #[test]
    fn parse_hex_color_invalid() {
        assert_eq!(parse_hex_color("xyz"), Color::White);
        assert_eq!(parse_hex_color(""), Color::White);
    }
}
