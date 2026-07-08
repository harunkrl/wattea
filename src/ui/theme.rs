//! Electric theme: semantic color tokens + threshold helpers.
//!
//! The whole UI sources colors through this struct; nowhere is a raw literal
//! like Color::Cyan used. Single built-in theme: Electric. When config-driven
//! themes are added later, only this file needs to change.

use ratatui::{
    style::{Color, Modifier, Style},
    widgets::{Block, Borders},
};

/// Severity thresholds (in raw units). Tuned in one place.
pub const BATTERY_OK: u8 = 50;
pub const BATTERY_WARN: u8 = 20; // below = danger
pub const CPU_WARN: f64 = 60.0;
pub const CPU_DANGER: f64 = 85.0;
pub const TEMP_WARN: f64 = 60.0;
pub const TEMP_DANGER: f64 = 75.0;
/// High-consumption session threshold (%/h) → accent_alt.
pub const SEVERE_DRAIN_PCT_H: f64 = 10.0;

/// Semantic color tokens. The UI sources colors through these.
pub struct Theme {
    pub bg: Color,
    pub surface: Color,
    pub border: Color,
    pub border_focus: Color,
    pub fg: Color,
    pub dim: Color,
    pub accent: Color,
    pub accent_alt: Color,
    pub ok: Color,
    pub warn: Color,
    pub danger: Color,
}

impl Theme {
    /// Built-in Electric palette (dark background + neon cyan/magenta/green).
    pub fn electric() -> Self {
        Self {
            bg: Color::Rgb(0x0a, 0x0e, 0x14),
            surface: Color::Rgb(0x0d, 0x12, 0x19),
            border: Color::Rgb(0x1c, 0x24, 0x33),
            border_focus: Color::Rgb(0x00, 0xe5, 0xff),
            fg: Color::Rgb(0xc8, 0xd3, 0xe0),
            dim: Color::Rgb(0x5a, 0x66, 0x78),
            accent: Color::Rgb(0x00, 0xe5, 0xff),
            accent_alt: Color::Rgb(0xff, 0x2e, 0x63),
            ok: Color::Rgb(0x39, 0xff, 0x14),
            warn: Color::Rgb(0xff, 0xae, 0x00),
            danger: Color::Rgb(0xff, 0x5c, 0x5c),
        }
    }

    /// Charging → accent; otherwise ok/warn/danger by capacity.
    pub fn charge_color(&self, capacity: u8, charging: bool) -> Color {
        if charging {
            self.accent
        } else if capacity >= BATTERY_OK {
            self.ok
        } else if capacity >= BATTERY_WARN {
            self.warn
        } else {
            self.danger
        }
    }

    /// value (same unit as the thresholds) → ok/warn/danger.
    pub fn severity(&self, value: f64, warn_at: f64, danger_at: f64) -> Color {
        if value >= danger_at {
            self.danger
        } else if value >= warn_at {
            self.warn
        } else {
            self.ok
        }
    }

    /// Style for the anomaly badge in the header.
    pub fn anomaly_badge(&self) -> Style {
        Style::default()
            .fg(self.bg)
            .bg(self.accent_alt)
            .add_modifier(Modifier::BOLD)
    }

    /// A consistent border+title panel. All panels use the same border color
    /// (border_focus = accent) → a consistent look across tabs.
    /// `focused` only makes the border bold for a subtle hierarchy (same color).
    pub fn panel(&self, title: &str, focused: bool) -> Block<'static> {
        let mut style = Style::default().fg(self.border_focus);
        if focused {
            style = style.add_modifier(Modifier::BOLD);
        }
        Block::default()
            .borders(Borders::ALL)
            .border_style(style)
            .title_style(Style::default().fg(self.accent))
            .title(title.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t() -> Theme {
        Theme::electric()
    }

    #[test]
    fn charge_color_charging_is_accent_regardless_of_capacity() {
        let t = t();
        assert_eq!(t.charge_color(5, true), t.accent);
        assert_eq!(t.charge_color(99, true), t.accent);
    }

    #[test]
    fn charge_color_discharging_bands() {
        let t = t();
        assert_eq!(t.charge_color(80, false), t.ok); // >=50
        assert_eq!(t.charge_color(50, false), t.ok); // boundary
        assert_eq!(t.charge_color(49, false), t.warn); // 20..49
        assert_eq!(t.charge_color(20, false), t.warn); // boundary
        assert_eq!(t.charge_color(19, false), t.danger); // <20
        assert_eq!(t.charge_color(0, false), t.danger);
    }

    #[test]
    fn severity_maps_to_three_bands() {
        let t = t();
        assert_eq!(t.severity(10.0, 60.0, 85.0), t.ok);
        assert_eq!(t.severity(60.0, 60.0, 85.0), t.warn); // ==warn_at
        assert_eq!(t.severity(84.9, 60.0, 85.0), t.warn);
        assert_eq!(t.severity(85.0, 60.0, 85.0), t.danger); // ==danger_at
        assert_eq!(t.severity(99.0, 60.0, 85.0), t.danger);
    }
}
