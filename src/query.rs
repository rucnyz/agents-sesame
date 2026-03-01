use chrono::{Local, NaiveDateTime, TimeDelta};
use regex::Regex;
use std::sync::LazyLock;

#[derive(Debug, Clone, PartialEq)]
pub enum DateOp {
    Exact,
    LessThan,
    GreaterThan,
}

#[derive(Debug, Clone)]
pub struct DateFilter {
    pub op: DateOp,
    pub cutoff: NaiveDateTime,
    pub negated: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ParsedQuery {
    pub text: String,
    pub agent: Option<Filter>,
    pub directory: Option<Filter>,
    pub date: Option<DateFilter>,
}

static KEYWORD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(-?)(agent|dir|date):(?:"([^"]+)"|(\S+))"#).unwrap());

static RELATIVE_TIME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([<>])?(\d+)(m|h|d|w|mo|y)$").unwrap());

fn parse_filter_value(value: &str) -> Filter {
    let mut include = Vec::new();
    let mut exclude = Vec::new();

    for part in value.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some(stripped) = part.strip_prefix('!').or_else(|| part.strip_prefix('-')) {
            exclude.push(stripped.to_lowercase());
        } else {
            include.push(part.to_lowercase());
        }
    }

    Filter { include, exclude }
}

fn parse_date_value(value: &str) -> Option<DateFilter> {
    let now = Local::now().naive_local();
    let (value, negated) = if let Some(v) = value.strip_prefix('!') {
        (v, true)
    } else {
        (value, false)
    };

    match value.to_lowercase().as_str() {
        "today" => {
            let start = now.date().and_hms_opt(0, 0, 0)?;
            Some(DateFilter {
                op: DateOp::Exact,
                cutoff: start,
                negated,
            })
        }
        "yesterday" => {
            let start = (now - TimeDelta::days(1)).date().and_hms_opt(0, 0, 0)?;
            Some(DateFilter {
                op: DateOp::Exact,
                cutoff: start,
                negated,
            })
        }
        _ => {
            let caps = RELATIVE_TIME_RE.captures(value)?;
            let op_str = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let amount: i64 = caps.get(2)?.as_str().parse().ok()?;
            let unit = caps.get(3)?.as_str();

            let delta = match unit {
                "m" => TimeDelta::minutes(amount),
                "h" => TimeDelta::hours(amount),
                "d" => TimeDelta::days(amount),
                "w" => TimeDelta::weeks(amount),
                "mo" => TimeDelta::days(amount * 30),
                "y" => TimeDelta::days(amount * 365),
                _ => return None,
            };

            let cutoff = now - delta;
            let op = match op_str {
                "<" => DateOp::LessThan,
                ">" => DateOp::GreaterThan,
                _ => DateOp::LessThan, // default: newer than
            };

            Some(DateFilter {
                op,
                cutoff,
                negated,
            })
        }
    }
}

pub fn parse_query(query: &str) -> ParsedQuery {
    let mut result = ParsedQuery::default();
    let mut remaining = query.to_string();

    // Extract keywords in reverse order to preserve positions
    let matches: Vec<_> = KEYWORD_RE.find_iter(query).collect();
    for m in matches.iter().rev() {
        remaining.replace_range(m.range(), "");
    }

    for caps in KEYWORD_RE.captures_iter(query) {
        let negated_prefix = &caps[1] == "-";
        let keyword = &caps[2];
        let value = caps
            .get(3)
            .or_else(|| caps.get(4))
            .map(|m| m.as_str())
            .unwrap_or("");

        match keyword {
            "agent" => {
                let mut filter = parse_filter_value(value);
                if negated_prefix {
                    // Move all includes to excludes
                    filter.exclude.append(&mut filter.include);
                }
                result.agent = Some(filter);
            }
            "dir" => {
                let mut filter = parse_filter_value(value);
                if negated_prefix {
                    filter.exclude.append(&mut filter.include);
                }
                result.directory = Some(filter);
            }
            "date" => {
                if let Some(mut df) = parse_date_value(value) {
                    if negated_prefix {
                        df.negated = !df.negated;
                    }
                    result.date = Some(df);
                }
            }
            _ => {}
        }
    }

    result.text = remaining.split_whitespace().collect::<Vec<_>>().join(" ");
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_text() {
        let q = parse_query("hello world");
        assert_eq!(q.text, "hello world");
        assert!(q.agent.is_none());
    }

    #[test]
    fn test_agent_filter() {
        let q = parse_query("agent:claude hello");
        assert_eq!(q.text, "hello");
        let f = q.agent.unwrap();
        assert_eq!(f.include, vec!["claude"]);
    }

    #[test]
    fn test_agent_exclude() {
        let q = parse_query("agent:!codex");
        let f = q.agent.unwrap();
        assert!(f.include.is_empty());
        assert_eq!(f.exclude, vec!["codex"]);
    }

    #[test]
    fn test_negated_prefix() {
        let q = parse_query("-agent:claude");
        let f = q.agent.unwrap();
        assert!(f.include.is_empty());
        assert_eq!(f.exclude, vec!["claude"]);
    }

    #[test]
    fn test_dir_filter() {
        let q = parse_query("dir:projects search");
        assert_eq!(q.text, "search");
        let f = q.directory.unwrap();
        assert_eq!(f.include, vec!["projects"]);
    }

    #[test]
    fn test_date_today() {
        let q = parse_query("date:today");
        assert!(q.date.is_some());
        let d = q.date.unwrap();
        assert_eq!(d.op, DateOp::Exact);
        assert!(!d.negated);
    }

    #[test]
    fn test_date_relative() {
        let q = parse_query("date:<1h");
        assert!(q.date.is_some());
        let d = q.date.unwrap();
        assert_eq!(d.op, DateOp::LessThan);
    }

    #[test]
    fn test_multiple_filters() {
        let q = parse_query("agent:claude dir:fast-resume date:<1d hello");
        assert_eq!(q.text, "hello");
        assert!(q.agent.is_some());
        assert!(q.directory.is_some());
        assert!(q.date.is_some());
    }
}
