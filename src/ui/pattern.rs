//! Pattern tab: average %/h pattern by hour/day. Intensity gradient
//! (green→red), peak bar in accent_alt, lowest in green, the "now" hour marked with ▲.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::Line,
    widgets::{Bar, BarChart, BarGroup, Paragraph},
};

use crate::app::App;
use crate::ui::theme::Theme;

/// Render the Pattern tab.
pub fn render(app: &App, frame: &mut Frame, area: Rect, theme: &Theme) {
    let [chart_area, summary_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(3)]).areas(area);

    let (axis_label, total) = match app.pattern_axis {
        crate::app::PatternAxis::Hourly => ("hour of day · avg %/h", 24u8),
        crate::app::PatternAxis::Weekday => ("day of week · avg %/h", 7u8),
    };
    let title = format!(
        "Pattern · {axis_label}  ·  d: toggle  ·  {} bins",
        app.pattern.len()
    );
    let block = theme.panel(&title, true);

    if app.pattern.is_empty() {
        let msg = if app.store.is_none() {
            " No history store. Run `wattea import` + the daemon. "
        } else {
            " No discharging samples yet — let the daemon collect data. "
        };
        frame.render_widget(Paragraph::new(msg).centered().block(block), chart_area);
        return;
    }

    let by_idx: std::collections::HashMap<u8, &crate::storage::HourlyBin> =
        app.pattern.iter().map(|b| (b.hour, b)).collect();
    let labels: Vec<String> = match app.pattern_axis {
        crate::app::PatternAxis::Hourly => (0..total).map(|h| format!("{h:02}")).collect(),
        crate::app::PatternAxis::Weekday => ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
    };

    let max_rate = app
        .pattern
        .iter()
        .map(|b| b.avg_pct_per_hour)
        .fold(1.0_f64, f64::max);
    let scale = 100.0;

    // Peak / lowest indices (by avg_pct_per_hour).
    let peak_idx = app
        .pattern
        .iter()
        .max_by(|a, b| {
            a.avg_pct_per_hour
                .partial_cmp(&b.avg_pct_per_hour)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|b| b.hour)
        .unwrap_or(0);
    let trough_idx = app
        .pattern
        .iter()
        .min_by(|a, b| {
            a.avg_pct_per_hour
                .partial_cmp(&b.avg_pct_per_hour)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|b| b.hour)
        .unwrap_or(0);

    // Current hour (roughly UTC) — for the "now" ▲ marker in hourly mode.
    let now_hour = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| ((d.as_secs() / 3600) % 24) as u8)
        .unwrap_or(0);

    let bars: Vec<Bar> = (0..total)
        .map(|i| {
            let idx = i;
            let bin = by_idx.get(&idx).copied();
            let rate = bin.map(|b| b.avg_pct_per_hour).unwrap_or(0.0);
            let samples = bin.map(|b| b.sample_count).unwrap_or(0);
            let intensity = (rate / max_rate).clamp(0.0, 1.0);
            let color: Color = if idx == peak_idx {
                theme.accent_alt
            } else if idx == trough_idx {
                theme.ok
            } else {
                theme.severity(intensity * 100.0, 50.0, 75.0)
            };
            let now_mark = if app.pattern_axis == crate::app::PatternAxis::Hourly && idx == now_hour
            {
                "▲"
            } else {
                ""
            };
            let text_value = if samples == 0 {
                String::new()
            } else {
                format!("{rate:.0}{now_mark}")
            };
            Bar::default()
                .label(Line::from(labels[i as usize].as_str()))
                .value((rate * scale / max_rate.max(1.0)).round() as u64)
                .text_value(text_value)
                .style(Style::new().fg(color))
        })
        .collect();

    let chart = BarChart::default()
        .block(block)
        .data(BarGroup::default().bars(&bars))
        .bar_width(if app.pattern_axis == crate::app::PatternAxis::Hourly {
            2
        } else {
            5
        })
        .bar_gap(1)
        .bar_style(Style::new().fg(theme.warn))
        .value_style(Style::new().fg(theme.fg).bold())
        .label_style(Style::new().dim())
        .max(scale.round() as u64);
    frame.render_widget(chart, chart_area);

    render_summary(app, frame, summary_area, theme);
}

/// Pattern summary: bin count, total samples, peak/low hour.
fn render_summary(app: &App, frame: &mut Frame, area: Rect, theme: &Theme) {
    let (axis_unit, total_bins) = match app.pattern_axis {
        crate::app::PatternAxis::Hourly => ("hour", 24u8),
        crate::app::PatternAxis::Weekday => ("day", 7u8),
    };
    let peak = app.pattern.iter().max_by(|a, b| {
        a.avg_pct_per_hour
            .partial_cmp(&b.avg_pct_per_hour)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let trough = app.pattern.iter().min_by(|a, b| {
        a.avg_pct_per_hour
            .partial_cmp(&b.avg_pct_per_hour)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let total_samples: usize = app.pattern.iter().map(|b| b.sample_count).sum();

    let label_for = |idx: u8| -> String {
        match app.pattern_axis {
            crate::app::PatternAxis::Hourly => format!("{idx:02}:00"),
            crate::app::PatternAxis::Weekday => ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"]
                .get(idx as usize)
                .map(|s| s.to_string())
                .unwrap_or_default(),
        }
    };

    let mut spans = vec![
        ratatui::text::Span::raw(" bins "),
        ratatui::text::Span::styled(
            format!("{}/{}", app.pattern.len(), total_bins),
            Style::new().fg(theme.accent).bold(),
        ),
        ratatui::text::Span::raw("  · samples ").fg(theme.dim),
        ratatui::text::Span::styled(total_samples.to_string(), Style::new().fg(theme.fg)),
    ];
    if let Some(p) = peak {
        spans.push(ratatui::text::Span::raw(format!("  · peak {axis_unit} ")).fg(theme.dim));
        spans.push(ratatui::text::Span::styled(
            format!("{} ({:.1}%/h)", label_for(p.hour), p.avg_pct_per_hour),
            Style::new().fg(theme.accent_alt).bold(),
        ));
    }
    if let (Some(t), Some(p)) = (trough, peak)
        && t.hour != p.hour
    {
        spans.push(ratatui::text::Span::raw(format!("  · low {axis_unit} ")).fg(theme.dim));
        spans.push(ratatui::text::Span::styled(
            format!("{} ({:.1}%/h)", label_for(t.hour), t.avg_pct_per_hour),
            Style::new().fg(theme.ok),
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).block(theme.panel("Summary", false)),
        area,
    );
}
