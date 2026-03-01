use ratatui::style::Color;
use serde::Deserialize;

/// Theme configuration from TOML config `[theme]` section.
/// Uses Material You color role names.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ThemeConfig {
    pub primary: Option<String>,
    pub on_surface: Option<String>,
    pub on_surface_variant: Option<String>,
    pub surface_variant: Option<String>,
    pub surface_container: Option<String>,
    pub secondary: Option<String>,
    pub tertiary: Option<String>,
    pub primary_container: Option<String>,
    pub error: Option<String>,
}

/// Resolved theme with concrete `Color` values.
#[allow(dead_code)]
pub struct Theme {
    /// Accent: focused borders, title bar brand, footer keys
    pub primary: Color,
    /// Normal text: query text, active sort headers
    pub on_surface: Color,
    /// Dim text: inactive borders, metadata, search prompt, placeholders
    pub on_surface_variant: Color,
    /// Selected row background
    pub surface_variant: Color,
    /// Scrollbar track
    pub surface_container: Color,
    /// Secondary accent: project scope label, user message prefix
    pub secondary: Color,
    /// Tertiary accent: local scope label, loading indicator, status messages
    pub tertiary: Color,
    /// Search highlight match background
    pub primary_container: Color,
    /// Error color
    pub error: Color,
}

impl Theme {
    pub fn from_config(config: &Option<ThemeConfig>) -> Self {
        let c = config.as_ref();
        Self {
            primary: parse_or(
                c.and_then(|c| c.primary.as_deref()),
                Color::Rgb(232, 123, 53),
            ),
            on_surface: parse_or(c.and_then(|c| c.on_surface.as_deref()), Color::White),
            on_surface_variant: parse_or(
                c.and_then(|c| c.on_surface_variant.as_deref()),
                Color::DarkGray,
            ),
            surface_variant: parse_or(
                c.and_then(|c| c.surface_variant.as_deref()),
                Color::Rgb(40, 40, 60),
            ),
            surface_container: parse_or(
                c.and_then(|c| c.surface_container.as_deref()),
                Color::Rgb(60, 60, 60),
            ),
            secondary: parse_or(c.and_then(|c| c.secondary.as_deref()), Color::Cyan),
            tertiary: parse_or(
                c.and_then(|c| c.tertiary.as_deref()),
                Color::Rgb(100, 255, 100),
            ),
            primary_container: parse_or(
                c.and_then(|c| c.primary_container.as_deref()),
                Color::Yellow,
            ),
            error: parse_or(c.and_then(|c| c.error.as_deref()), Color::Red),
        }
    }
}

fn parse_or(hex: Option<&str>, default: Color) -> Color {
    match hex {
        Some(h) => parse_hex_color(h).unwrap_or(default),
        None => default,
    }
}

fn parse_hex_color(hex: &str) -> Option<Color> {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}
