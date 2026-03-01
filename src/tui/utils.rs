use chrono::NaiveDateTime;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

/// Format a timestamp as a human-readable "time ago" string.
pub fn format_time_ago(dt: NaiveDateTime) -> String {
    let now = chrono::Local::now().naive_local();
    let duration = now.signed_duration_since(dt);

    let seconds = duration.num_seconds();
    if seconds < 60 {
        return "just now".to_string();
    }

    let minutes = duration.num_minutes();
    if minutes < 60 {
        return format!("{minutes}m ago");
    }

    let hours = duration.num_hours();
    if hours < 24 {
        return format!("{hours}h ago");
    }

    let days = duration.num_days();
    if days < 7 {
        return format!("{days}d ago");
    }

    if days < 30 {
        let weeks = days / 7;
        return format!("{weeks}w ago");
    }

    if days < 365 {
        let months = days / 30;
        return format!("{months}mo ago");
    }

    let years = days / 365;
    format!("{years}y ago")
}

/// Format a directory path, replacing home directory with `~`.
pub fn format_directory(path: &str, max_width: usize) -> String {
    let home = dirs::home_dir().unwrap_or_default();
    let home_str = home.to_string_lossy();
    let display = path.replace(&*home_str, "~");

    if display.width() <= max_width {
        return display;
    }

    // Truncate from the left with "..." prefix
    let available = max_width.saturating_sub(3);
    let mut width = 0;
    let mut start_idx = display.len();
    // Walk from the end, accumulating display width
    for (i, c) in display.char_indices().rev() {
        let cw = c.width().unwrap_or(0);
        if width + cw > available {
            break;
        }
        width += cw;
        start_idx = i;
    }
    format!("...{}", &display[start_idx..])
}

/// Truncate a string to fit within `max_cells` terminal cells.
/// Appends "..." if truncated.
pub fn truncate_to_width(s: &str, max_cells: usize) -> String {
    let sw = s.width();
    if sw <= max_cells {
        return s.to_string();
    }
    if max_cells <= 3 {
        let mut out = String::new();
        let mut w = 0;
        for c in s.chars() {
            let cw = c.width().unwrap_or(0);
            if w + cw > max_cells {
                break;
            }
            out.push(c);
            w += cw;
        }
        return out;
    }
    let target = max_cells - 3; // reserve for "..."
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw > target {
            break;
        }
        out.push(c);
        w += cw;
    }
    format!("{out}...")
}

/// Pad a string with spaces so it occupies exactly `target_cells` terminal cells.
/// Truncates if wider.
pub fn pad_to_width(s: &str, target_cells: usize) -> String {
    let sw = s.width();
    if sw >= target_cells {
        truncate_to_width(s, target_cells)
    } else {
        format!("{s}{}", " ".repeat(target_cells - sw))
    }
}

/// Get the display width of a string in terminal cells.
#[allow(dead_code)]
pub fn display_width(s: &str) -> usize {
    s.width()
}

/// Get age-based color for timestamps (green → yellow → orange → gray).
pub fn get_age_color(age_hours: f64) -> Color {
    if age_hours < 1.0 {
        Color::Rgb(0, 255, 100) // bright green
    } else if age_hours < 6.0 {
        Color::Rgb(100, 255, 100) // green
    } else if age_hours < 24.0 {
        Color::Rgb(200, 255, 100) // yellow-green
    } else if age_hours < 48.0 {
        Color::Rgb(255, 255, 100) // yellow
    } else if age_hours < 72.0 {
        Color::Rgb(255, 200, 80) // orange-yellow
    } else if age_hours < 168.0 {
        Color::Rgb(255, 150, 50) // orange
    } else {
        Color::Rgb(140, 140, 140) // gray
    }
}

/// Highlight query terms in text, returning owned Spans.
pub fn highlight_spans(text: &str, query: &str, base_color: Color) -> Vec<Span<'static>> {
    let base_style = Style::default().fg(base_color);

    if query.is_empty() || text.is_empty() {
        return vec![Span::styled(text.to_string(), base_style)];
    }

    let highlight_style = base_style.add_modifier(Modifier::BOLD | Modifier::REVERSED);

    // Extract only freetext terms (skip structured prefixes like agent:, dir:, date:, -agent:)
    let terms: Vec<String> = query
        .split_whitespace()
        .filter(|t| {
            !t.starts_with("agent:")
                && !t.starts_with("-agent:")
                && !t.starts_with("dir:")
                && !t.starts_with("date:")
        })
        .map(|t| t.to_lowercase())
        .collect();

    if terms.is_empty() {
        return vec![Span::styled(text.to_string(), base_style)];
    }

    let lower_text = text.to_lowercase();

    // Find all match positions
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

/// Copy text to clipboard (Linux: wl-copy or xclip).
pub fn copy_to_clipboard(text: &str) -> bool {
    // Try wl-copy first (Wayland)
    if let Ok(mut child) = std::process::Command::new("wl-copy")
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        use std::io::Write;
        if let Some(ref mut stdin) = child.stdin {
            let _ = stdin.write_all(text.as_bytes());
        }
        return child.wait().map(|s| s.success()).unwrap_or(false);
    }

    // Fallback to xclip
    if let Ok(mut child) = std::process::Command::new("xclip")
        .args(["-selection", "clipboard"])
        .stdin(std::process::Stdio::piped())
        .spawn()
    {
        use std::io::Write;
        if let Some(ref mut stdin) = child.stdin {
            let _ = stdin.write_all(text.as_bytes());
        }
        return child.wait().map(|s| s.success()).unwrap_or(false);
    }

    false
}
