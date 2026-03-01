use chrono::NaiveDateTime;

#[derive(Debug, Clone)]
pub struct Session {
    pub id: String,
    pub agent: String,
    pub title: String,
    pub directory: String,
    pub timestamp: NaiveDateTime,
    pub content: String,
    pub message_count: usize,
    pub mtime: f64,
    pub yolo: bool,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct RawAdapterStats {
    pub agent: String,
    pub data_dir: String,
    pub available: bool,
    pub file_count: usize,
    pub total_bytes: u64,
}

#[derive(Debug)]
#[allow(dead_code)]
pub struct ParseError {
    pub agent: String,
    pub file_path: String,
    pub error_type: String,
    pub message: String,
}

/// Truncate a title string, preferring word boundaries.
pub fn truncate_title(text: &str, max_length: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max_length {
        return text.to_string();
    }

    let chars: Vec<char> = text.chars().take(max_length).collect();
    let truncated: String = chars.into_iter().collect();

    // Try to break at last space for cleaner truncation
    if let Some(last_space) = truncated.rfind(' ')
        && last_space > max_length / 2
    {
        return format!("{}...", &truncated[..last_space]);
    }
    format!("{truncated}...")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_short() {
        assert_eq!(truncate_title("hello world", 100), "hello world");
    }

    #[test]
    fn test_truncate_at_word() {
        let long = "hello world this is a very long title";
        let result = truncate_title(long, 20);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 23); // 20 + "..."
    }

    #[test]
    fn test_truncate_strips() {
        assert_eq!(truncate_title("  hello  ", 100), "hello");
    }
}
