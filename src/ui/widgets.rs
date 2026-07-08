//! Reusable custom/composite widgets.
//!
//! Shared by all tabs: `panel`, `tab_bar` (native Tabs wrapper).
//! Custom render widgets (`BatteryCell`) and composite helpers
//! (`metric_card`, `mini_bar`) are used across the dashboard.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Tabs, Widget},
};

use crate::ui::theme::Theme;

/// A consistent panel block (thin wrapper around theme.panel).
///
/// Single entry point: any future panel logic (header/footer etc.) goes here.
pub fn panel(theme: &Theme, title: &str, focused: bool) -> Block<'static> {
    theme.panel(title, focused)
}

/// Active tabbed native Tabs widget.
///
/// `active` → accent background/black foreground/bold; inactive → dim.
pub fn tab_bar(titles: &[&str], active: usize, theme: &Theme) -> Tabs<'static> {
    // Convert each title to an owned String → Line<'static> → Tabs<'static>.
    // (Line::from(&str) borrows; our titles do not outlive the function.)
    let highlights = titles
        .iter()
        .map(|t| Line::from((*t).to_string()))
        .collect::<Vec<_>>();
    Tabs::new(highlights)
        .select(active)
        .style(Style::default().fg(theme.dim))
        .highlight_style(
            Style::default()
                .fg(ratatui::style::Color::Black)
                .bg(theme.accent)
                .add_modifier(ratatui::style::Modifier::BOLD),
        )
        .divider(ratatui::symbols::DOT)
}

/// Horizontal charge bar (using `█░` block characters).
/// Fills left-to-right up to capacity%; color comes from `theme.charge_color`.
pub struct BatteryCell<'a> {
    pub capacity: u8, // 0..=100
    pub charging: bool,
    pub theme: &'a Theme,
}

impl Widget for BatteryCell<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let fill = self.theme.charge_color(self.capacity, self.charging);
        let total = area.width as usize;
        // Fill left-to-right: capacity% worth of columns filled.
        let filled_cols = ((self.capacity as u32 * area.width as u32) / 100) as usize;
        for x in 0..total {
            let is_filled = x < filled_cols;
            let col = area.x + x as u16;
            for y in area.y..area.y + area.height {
                let cell = &mut buf[(col, y)];
                cell.set_symbol(if is_filled { "█" } else { "░" });
                cell.set_style(Style::default().fg(if is_filled { fill } else { self.theme.dim }));
            }
        }
    }
}

/// Statistics card: panel title=label (label in the border title),
/// content = large value + optional mini-bar. NO label duplication.
/// Height requirement: border(2) + value(1) [+ bar(1)] = 3-4 lines.
pub fn metric_card(
    theme: &Theme,
    _label: &str,
    value: &str,
    color: Color,
    bar: Option<f64>,
) -> Paragraph<'static> {
    let mut lines = vec![Line::from(vec![Span::styled(
        value.to_string(),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )])];
    if let Some(pct) = bar {
        lines.push(Line::from(mini_bar_spans(theme, pct, color)));
    }
    Paragraph::new(lines)
}

/// A single-line labeled horizontal bar (label + █/░ + pct). Not a MiniBar
/// widget but a returned Line — used as a row in metric lists.
pub fn mini_bar(theme: &Theme, label: &str, pct: f64, color: Color) -> Line<'static> {
    let mut spans = vec![Span::styled(
        format!("{label:<8} "),
        Style::default().fg(theme.dim),
    )];
    spans.extend(mini_bar_spans(theme, pct, color));
    Line::from(spans)
}

pub fn mini_bar_spans(theme: &Theme, pct: f64, color: Color) -> Vec<Span<'static>> {
    let width = 10usize;
    let filled = ((pct.clamp(0.0, 100.0) / 100.0) * width as f64).round() as usize;
    let mut s = String::new();
    for i in 0..width {
        s.push(if i < filled { '█' } else { '░' });
    }
    // theme is not used here but kept in the signature: for future background color etc.
    let _ = theme;
    vec![Span::styled(s, Style::default().fg(color))]
}

/// Top header block with brand + live clock + anomaly badge.
///
/// `pulse` comes from the SystemTime second parity (no binary change needed,
/// the `view()` signature is preserved). When there are anomalies the badge
/// pulses with accent_alt.
pub fn branded_header<'a>(theme: &'a Theme, anomaly_count: usize) -> Block<'a> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let pulse = secs.is_multiple_of(2);
    let clock = format!("{:02}:{:02}", (secs % 86400) / 3600, (secs % 3600) / 60);

    // Anomaly badge style (based on pulse).
    let badge = if pulse {
        Style::default()
            .fg(theme.bg)
            .bg(theme.accent_alt)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(theme.accent_alt)
            .add_modifier(Modifier::BOLD)
    };

    // Combine in a single line: brand (left) + clock/badge (right). Borders::BOTTOM
    // separates from the body; the title sits on the line just above the border.
    let mut title_spans: Vec<Span> = vec![
        Span::raw(" "),
        Span::styled(
            "WATTEA",
            Style::default()
                .fg(theme.accent)
                .bold()
                .add_modifier(Modifier::ITALIC),
        ),
        Span::styled(
            "  battery consumption tracker",
            Style::default().fg(theme.dim),
        ),
    ];
    // Right-align the right side: clock + anomaly badge.
    let pad = " ".repeat(2);
    title_spans.push(Span::raw(pad));
    title_spans.push(Span::styled(clock, Style::default().fg(theme.fg)));
    if anomaly_count > 0 {
        title_spans.push(Span::raw("  "));
        title_spans.push(Span::styled(format!("⚡ {}", anomaly_count), badge));
    }

    Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(theme.border))
        .title(ratatui::text::Line::from(title_spans))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn tab_bar_renders_without_panic() {
        let theme = Theme::electric();
        let mut term = Terminal::new(TestBackend::new(60, 5)).unwrap();
        term.draw(|f| {
            let t = tab_bar(&["1 Live", "2 Trend", "3 Pattern", "4 Sessions"], 1, &theme);
            f.render_widget(t, ratatui::layout::Rect::new(0, 0, 60, 1));
        })
        .unwrap();
    }

    #[test]
    fn battery_cell_fill_proportion_matches_capacity() {
        // 50% fill, a 4-wide area → 2 columns "█" from the left, 2 "░" on the right.
        let theme = Theme::electric();
        let mut term = Terminal::new(TestBackend::new(8, 2)).unwrap();
        term.draw(|f| {
            f.render_widget(
                BatteryCell {
                    capacity: 50,
                    charging: false,
                    theme: &theme,
                },
                ratatui::layout::Rect::new(0, 0, 4, 1),
            );
        })
        .unwrap();
        let view = term.backend().buffer().content().to_vec();
        // (0,0) filled (█), (2,0) empty (░).
        assert_eq!(view[0].symbol(), "█");
        assert_eq!(view[2].symbol(), "░");
    }

    #[test]
    fn battery_cell_renders_at_extremes() {
        let theme = Theme::electric();
        for cap in [0u8, 100u8] {
            let mut term = Terminal::new(TestBackend::new(6, 6)).unwrap();
            term.draw(|f| {
                f.render_widget(
                    BatteryCell {
                        capacity: cap,
                        charging: false,
                        theme: &theme,
                    },
                    ratatui::layout::Rect::new(0, 0, 4, 6),
                );
            })
            .unwrap();
        }
    }

    #[test]
    fn mini_bar_fill_count_matches_pct() {
        let theme = Theme::electric();
        // 50% → 10-wide bar → 5 filled.
        let line = mini_bar(&theme, "CPU", 50.0, theme.ok);
        // spans[0] label, spans[1] bar.
        let filled = line.spans[1].content.chars().filter(|c| *c == '█').count();
        assert_eq!(filled, 5);
    }

    #[test]
    fn mini_bar_clamps_overflow_and_underflow() {
        let theme = Theme::electric();
        let over = mini_bar(&theme, "X", 150.0, theme.ok);
        assert_eq!(
            over.spans[1].content.chars().filter(|c| *c == '█').count(),
            10
        );
        let under = mini_bar(&theme, "Y", -20.0, theme.ok);
        assert_eq!(
            under.spans[1].content.chars().filter(|c| *c == '█').count(),
            0
        );
    }

    #[test]
    fn branded_header_renders_with_and_without_anomalies() {
        let theme = Theme::electric();
        let mut term = Terminal::new(TestBackend::new(80, 4)).unwrap();
        // No anomalies.
        term.draw(|f| {
            f.render_widget(
                branded_header(&theme, 0),
                ratatui::layout::Rect::new(0, 0, 80, 3),
            );
        })
        .unwrap();
        // With anomalies.
        term.draw(|f| {
            f.render_widget(
                branded_header(&theme, 3),
                ratatui::layout::Rect::new(0, 0, 80, 3),
            );
        })
        .unwrap();
    }
}
