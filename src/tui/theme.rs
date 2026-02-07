use std::collections::HashMap;

use ratatui::style::Color;

use crate::model::UiConfig;

/// Parsed color theme for the TUI
#[derive(Debug, Clone)]
pub struct Theme {
    pub background: Color,
    pub text: Color,
    pub text_bright: Color,
    pub highlight: Color,
    pub dim: Color,
    pub red: Color,
    pub yellow: Color,
    pub green: Color,
    pub cyan: Color,
    pub purple: Color,
    pub blue: Color,
    pub selection_bg: Color,
    pub selection_border: Color,
    pub selection_id: Color,
    pub search_match_bg: Color,
    pub search_match_fg: Color,
    /// Per-tag colors
    pub tag_colors: HashMap<String, Color>,
}

impl Default for Theme {
    fn default() -> Self {
        let mut tag_colors = HashMap::new();
        tag_colors.insert("research".into(), Color::Rgb(0x44, 0x88, 0xFF));
        tag_colors.insert("design".into(), Color::Rgb(0x44, 0xDD, 0xFF));
        tag_colors.insert("ready".into(), Color::Rgb(0x44, 0xFF, 0x88));
        tag_colors.insert("bug".into(), Color::Rgb(0xFF, 0x44, 0x44));
        tag_colors.insert("cc".into(), Color::Rgb(0xCC, 0x66, 0xFF));
        tag_colors.insert("cc-added".into(), Color::Rgb(0xCC, 0x66, 0xFF));
        tag_colors.insert("needs-input".into(), Color::Rgb(0xFF, 0xD7, 0x00));

        Theme {
            background: Color::Rgb(0x0C, 0x00, 0x1B),
            text: Color::Rgb(0xB0, 0xAA, 0xFF),
            text_bright: Color::Rgb(0xFF, 0xFF, 0xFF),
            highlight: Color::Rgb(0xFB, 0x41, 0x96),
            dim: Color::Rgb(0x7D, 0x78, 0xBF),
            red: Color::Rgb(0xFF, 0x44, 0x44),
            yellow: Color::Rgb(0xFF, 0xD7, 0x00),
            green: Color::Rgb(0x44, 0xFF, 0x88),
            cyan: Color::Rgb(0x44, 0xDD, 0xFF),
            purple: Color::Rgb(0xCC, 0x66, 0xFF),
            blue: Color::Rgb(0x44, 0x88, 0xFF),
            selection_bg: Color::Rgb(0x3D, 0x14, 0x38),
            selection_border: Color::Rgb(0xFB, 0x41, 0x96),
            selection_id: Color::Rgb(0xDA, 0xB8, 0xF0),
            search_match_bg: Color::Rgb(0x40, 0xE0, 0xD0),
            search_match_fg: Color::Rgb(0x0C, 0x00, 0x1B),
            tag_colors,
        }
    }
}

/// Parse a hex color string like "#FF4444" into an RGB Color
fn parse_hex_color(hex: &str) -> Option<Color> {
    let hex = hex.strip_prefix('#')?;
    if hex.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
    let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
    let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
    Some(Color::Rgb(r, g, b))
}

impl Theme {
    /// Create a theme from project UI config, falling back to defaults
    pub fn from_config(ui: &UiConfig) -> Self {
        let mut theme = Theme::default();

        // Apply color overrides from [ui.colors]
        for (key, value) in &ui.colors {
            if let Some(color) = parse_hex_color(value) {
                match key.as_str() {
                    "background" => theme.background = color,
                    "text" => theme.text = color,
                    "text_bright" => theme.text_bright = color,
                    "highlight" => theme.highlight = color,
                    "dim" => theme.dim = color,
                    "red" => theme.red = color,
                    "yellow" => theme.yellow = color,
                    "green" => theme.green = color,
                    "cyan" => theme.cyan = color,
                    "purple" => theme.purple = color,
                    "blue" => theme.blue = color,
                    "selection_bg" => theme.selection_bg = color,
                    "selection_border" => theme.selection_border = color,
                    "selection_id" => theme.selection_id = color,
                    "search_match_bg" => theme.search_match_bg = color,
                    "search_match_fg" => theme.search_match_fg = color,
                    _ => {}
                }
            }
        }

        // Apply tag color overrides from [ui.tag_colors]
        for (tag, value) in &ui.tag_colors {
            if let Some(color) = parse_hex_color(value) {
                theme.tag_colors.insert(tag.clone(), color);
            }
        }

        theme
    }

    /// Get the color for a tag, falling back to text color
    pub fn tag_color(&self, tag: &str) -> Color {
        self.tag_colors.get(tag).copied().unwrap_or(self.text)
    }

    /// Get the color for a task state
    pub fn state_color(&self, state: crate::model::TaskState) -> Color {
        match state {
            crate::model::TaskState::Todo => self.text,
            crate::model::TaskState::Active => self.highlight,
            crate::model::TaskState::Blocked => self.red,
            crate::model::TaskState::Done => self.text,
            crate::model::TaskState::Parked => self.yellow,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_hex_color() {
        assert_eq!(
            parse_hex_color("#FF4444"),
            Some(Color::Rgb(0xFF, 0x44, 0x44))
        );
        assert_eq!(
            parse_hex_color("#0C001B"),
            Some(Color::Rgb(0x0C, 0x00, 0x1B))
        );
        assert_eq!(parse_hex_color("FF4444"), None); // missing #
        assert_eq!(parse_hex_color("#FF44"), None); // too short
        assert_eq!(parse_hex_color("#ZZZZZZ"), None); // invalid hex
    }

    #[test]
    fn test_default_theme_has_all_tag_colors() {
        let theme = Theme::default();
        assert_eq!(
            theme.tag_colors.get("research"),
            Some(&Color::Rgb(0x44, 0x88, 0xFF))
        );
        assert_eq!(
            theme.tag_colors.get("bug"),
            Some(&Color::Rgb(0xFF, 0x44, 0x44))
        );
        assert_eq!(
            theme.tag_colors.get("cc"),
            Some(&Color::Rgb(0xCC, 0x66, 0xFF))
        );
    }

    #[test]
    fn test_from_config_overrides() {
        let mut ui = UiConfig::default();
        ui.colors.insert("background".into(), "#000000".into());
        ui.tag_colors.insert("custom".into(), "#112233".into());

        let theme = Theme::from_config(&ui);
        assert_eq!(theme.background, Color::Rgb(0, 0, 0));
        assert_eq!(
            theme.tag_colors.get("custom"),
            Some(&Color::Rgb(0x11, 0x22, 0x33))
        );
        // Unchanged defaults still present
        assert_eq!(theme.text, Color::Rgb(0xB0, 0xAA, 0xFF));
    }

    #[test]
    fn test_tag_color_fallback() {
        let theme = Theme::default();
        assert_eq!(theme.tag_color("research"), Color::Rgb(0x44, 0x88, 0xFF));
        // Unknown tag falls back to text color
        assert_eq!(theme.tag_color("unknown"), theme.text);
    }

    #[test]
    fn test_state_color() {
        use crate::model::TaskState;
        let theme = Theme::default();
        assert_eq!(theme.state_color(TaskState::Active), theme.highlight);
        assert_eq!(theme.state_color(TaskState::Blocked), theme.red);
        assert_eq!(theme.state_color(TaskState::Done), theme.text);
        assert_eq!(theme.state_color(TaskState::Parked), theme.yellow);
    }
}
