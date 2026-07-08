//! Trend tab: vertically stacked charts — capacity % on top (fixed 0–100),
//! power W below (dynamic [0, peak.max(15)] bounds). This fixes the issue a
//! reviewer caught where "power was squashed against the axis on a single scale".

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::Span,
    widgets::{Axis, Chart, Dataset, GraphType, Paragraph},
    Frame,
};

use crate::app::App;

use super::opt_fmt;

/// Render the Trend tab.
pub fn render(app: &App, frame: &mut Frame, area: Rect, theme: &crate::ui::theme::Theme) {
    let [cap_area, pow_area, sum_area] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Fill(1),
        Constraint::Length(3),
    ])
    .areas(area);

    if app.trend.is_empty() {
        let msg = if app.store.is_none() {
            " No history store (run `wattea import` + the daemon). "
        } else {
            " No data yet in the last 24h. "
        };
        frame.render_widget(
            Paragraph::new(msg)
                .centered()
                .block(theme.panel("Trend · last 24h", true)),
            area,
        );
        return;
    }

    let now = chrono_now_ts();
    let x_min = (now - crate::app::TREND_WINDOW_SECS as i64) as f64;
    let span_h = (now as f64 - x_min) / 3600.0;

    let cap_pts: Vec<(f64, f64)> = app
        .trend
        .iter()
        .map(|s| ((s.ts as f64 - x_min) / 3600.0, s.capacity as f64))
        .collect();
    let pow_pts: Vec<(f64, f64)> = app
        .trend
        .iter()
        .filter_map(|s| s.power_now.map(|p| ((s.ts as f64 - x_min) / 3600.0, p)))
        .collect();
    let powers: Vec<f64> = pow_pts.iter().map(|(_, p)| *p).collect();
    let pb = power_bounds(&powers);

    // --- Capacity chart (fixed 0–100) + critical 20% line ---
    let crit_pts = [(0.0, 20.0), (span_h, 20.0)];
    let cap_chart = Chart::new(vec![
        Dataset::default()
            // no legend: the panel title "Capacity %" + summary already explain it.
            .marker(ratatui::symbols::Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::new().fg(theme.accent))
            .data(&cap_pts),
        Dataset::default()
            .marker(ratatui::symbols::Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::new().fg(theme.warn))
            .data(&crit_pts),
    ])
    .block(theme.panel("Capacity %", false))
    .x_axis(
        Axis::default()
            .style(Style::new().fg(theme.dim))
            .bounds([0.0, span_h])
            .labels(vec![Span::raw("-24h"), Span::raw("-12h"), Span::raw("now")]),
    )
    .y_axis(
        Axis::default()
            .style(Style::new().fg(theme.dim))
            .bounds([0.0, 100.0])
            .labels(vec![Span::raw("0"), Span::raw("50"), Span::raw("100")]),
    );
    frame.render_widget(cap_chart, cap_area);

    // Find the row of the critical 20% line (warn color) and write "20" on the
    // left axis at that height (ratatui's Axis breaks with >3 labels, so it is manual).
    label_critical_20(frame, cap_area, theme.warn);

    // Peak marker removed: the Scatter+Block looked large/crude.
    // The peak value is shown in the summary bar ("peak X.XW") in accent_alt.
    let pow_chart = Chart::new(vec![Dataset::default()
        // no legend: the panel title "Power W" already explains it.
        .marker(ratatui::symbols::Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::new().fg(theme.warn))
        .data(&pow_pts)])
    .block(theme.panel("Power W", false))
    .x_axis(
        Axis::default()
            .style(Style::new().fg(theme.dim))
            .bounds([0.0, span_h])
            .labels(vec![Span::raw("-24h"), Span::raw("-12h"), Span::raw("now")]),
    )
    .y_axis(
        Axis::default()
            .style(Style::new().fg(theme.dim))
            .bounds(pb)
            .labels(vec![Span::raw("0"), Span::raw(format!("{:.0}", pb[1]))]),
    );
    frame.render_widget(pow_chart, pow_area);

    render_summary(app, frame, sum_area, theme);
}

/// Summary bar of the Trend tab: sample count, average/peak power, % drop.
fn render_summary(app: &App, frame: &mut Frame, area: Rect, theme: &crate::ui::theme::Theme) {
    let powers: Vec<f64> = app.trend.iter().filter_map(|s| s.power_now).collect();
    let avg = (!powers.is_empty()).then(|| powers.iter().sum::<f64>() / powers.len() as f64);
    let peak = powers.iter().copied().fold(0.0_f64, f64::max);
    let drop = (!app.trend.is_empty()).then(|| {
        app.trend.first().unwrap().capacity as i16 - app.trend.last().unwrap().capacity as i16
    });

    let mut spans = vec![
        Span::raw(" samples "),
        Span::styled(
            format!("{}", app.trend.len()),
            Style::new().fg(theme.accent).bold(),
        ),
        Span::raw("  · avg power ").fg(theme.dim),
        Span::styled(opt_fmt(avg, "W", 2), Style::new().fg(theme.fg)),
        Span::raw("  · peak ").fg(theme.dim),
        Span::styled(format!("{peak:.2} W"), Style::new().fg(theme.warn)),
        Span::raw("  · charge delta ").fg(theme.dim),
        Span::styled(
            drop.map(|d| format!("{d:+}%"))
                .unwrap_or_else(|| "—".into()),
            Style::new().fg(match drop {
                Some(d) if d > 0 => Color::Red,
                Some(d) if d < 0 => theme.ok,
                _ => theme.dim,
            }),
        ),
    ];
    let _ = &mut spans;
    frame.render_widget(
        Paragraph::new(ratatui::text::Line::from(spans)).block(theme.panel("Summary", false)),
        area,
    );
}

// --- trend-only helpers -------------------------------------------------------

/// Dynamic y-bounds for the power chart: `[0.0, peak.max(15.0)]`.
///
/// Finds the row of the critical 20% line (the warn-colored Braille cells) and
/// writes "20" on the left axis at that height (in warn color).
/// Done manually because ratatui's Axis places more than 3 labels incorrectly.
fn label_critical_20(frame: &mut Frame, area: Rect, warn: Color) {
    // Find the first warn-colored cell → the y=20 row + the plot's left edge.
    let buf = frame.buffer_mut();
    let mut hit_row: Option<u16> = None;
    let mut plot_left: Option<u16> = None;
    'outer: for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if buf[(x, y)].fg == warn {
                hit_row = Some(y);
                plot_left = Some(x);
                break 'outer;
            }
        }
    }
    let (row, plot_left) = match (hit_row, plot_left) {
        (Some(r), Some(p)) => (r, p),
        _ => return, // line not found (area too small)
    };
    // Write "20" just before the plot's left edge (right-aligned with the labels).
    if plot_left >= 2 {
        let a = &mut buf[(plot_left - 2, row)];
        a.set_symbol("2");
        a.fg = warn;
        let b = &mut buf[(plot_left - 1, row)];
        b.set_symbol("0");
        b.fg = warn;
    }
}

/// A 15W floor prevents the chart from flattening at low consumption.
fn power_bounds(powers: &[f64]) -> [f64; 2] {
    let peak = powers.iter().copied().fold(0.0_f64, f64::max);
    [0.0, peak.max(15.0)]
}

/// Current Unix timestamp (seconds).
fn chrono_now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::power_bounds;

    #[test]
    fn power_bounds_dynamic_and_floored() {
        assert_eq!(power_bounds(&[]), [0.0, 15.0]); // floor
        assert_eq!(power_bounds(&[5.0]), [0.0, 15.0]); // below floor
        assert_eq!(power_bounds(&[22.1, 8.0]), [0.0, 22.1]); // uses peak
        assert_eq!(power_bounds(&[3.0, 4.0]), [0.0, 15.0]); // floored up
    }
}
