//! Compact mod: btop tarzı yoğun tek-ekran dashboard. Açılışta default.
//!
//! Şarj + hızlı metrikler (üst şerit), güç grafiği + process listesi (yan yana),
//! sistem mini-bar'ları (alt) — hepsi tek ekranda.

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Axis, Cell, Chart, Dataset, GraphType, Paragraph, Row, Table},
    Frame,
};

use crate::app::App;
use crate::battery::Status;
use crate::ui::theme::{Theme, CPU_DANGER, CPU_WARN, TEMP_DANGER, TEMP_WARN};
use crate::ui::widgets;

/// Compact dashboard'u çizer.
pub fn render(app: &App, frame: &mut Frame, area: Rect, theme: &Theme) {
    let [strip, main, sys] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Fill(1),
        Constraint::Length(3),
    ])
    .areas(area);

    render_metric_strip(app, frame, strip, theme);
    let [pw, pr] =
        Layout::horizontal([Constraint::Percentage(58), Constraint::Fill(1)]).areas(main);
    render_power(app, frame, pw, theme);
    render_procs(app, frame, pr, theme);
    render_sys_strip(app, frame, sys, theme);
}

/// Üst şerit: şarj çubuğu + % + hızlı metrikler tek satırda.
fn render_metric_strip(app: &App, frame: &mut Frame, area: Rect, theme: &Theme) {
    let block = theme.panel("Charge", true);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let s = match &app.sample {
        Some(s) => s,
        None => {
            frame.render_widget(Paragraph::new(" reading battery… ").centered(), inner);
            return;
        }
    };
    let charging = s.status == Status::Charging;
    let color = theme.charge_color(s.capacity, charging);

    // [şarj çubuğu + %] | [inline metrikler]
    let [bar_cell, rest] =
        Layout::horizontal([Constraint::Length(20), Constraint::Fill(1)]).areas(inner);
    let [bar_area, pct_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(5)]).areas(bar_cell);
    frame.render_widget(
        widgets::BatteryCell {
            capacity: s.capacity,
            charging,
            theme,
        },
        bar_area,
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            format!("{}%", s.capacity),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )]))
        .right_aligned(),
        pct_area,
    );

    // Inline metrikler: status · rate · power · voltage · health · temp · cycles
    let rate = s.pct_per_hour();
    let mut spans = vec![
        Span::raw("  "),
        Span::styled(
            format!("{} {}", s.status.glyph(), s.status.label()),
            Style::default().fg(color),
        ),
    ];
    spans.push(Span::raw("  · "));
    push_metric(
        &mut spans,
        theme,
        "rate",
        rate.map(|r| format!("{r:+.1}%/h")),
    );
    push_metric(
        &mut spans,
        theme,
        "pwr",
        s.power_now.map(|w| format!("{w:.1}W")),
    );
    push_metric(&mut spans, theme, "V", s.voltage.map(|v| format!("{v:.2}")));
    push_metric(
        &mut spans,
        theme,
        "health",
        Some(format!("{:.0}%", s.health())),
    );
    push_metric(
        &mut spans,
        theme,
        "cpu",
        app.sys
            .as_ref()
            .and_then(|m| m.cpu_load)
            .map(|v| format!("{v:.0}%")),
    );
    push_metric(
        &mut spans,
        theme,
        "temp",
        app.sys
            .as_ref()
            .and_then(|m| m.temperature)
            .map(|t| format!("{t:.0}°C")),
    );
    push_metric(
        &mut spans,
        theme,
        "cyc",
        s.cycle_count.map(|c| c.to_string()),
    );
    frame.render_widget(Paragraph::new(Line::from(spans)), rest);
}

fn push_metric(spans: &mut Vec<Span>, theme: &Theme, label: &str, value: Option<String>) {
    spans.push(Span::styled(
        format!("{label} "),
        Style::default().fg(theme.dim),
    ));
    spans.push(Span::styled(
        value.unwrap_or_else(|| "—".into()),
        Style::default().fg(theme.fg),
    ));
    spans.push(Span::raw("  "));
}

/// Güç çizgi grafiği (canlı, Trend ile tutarlı Braille).
fn render_power(app: &App, frame: &mut Frame, area: Rect, theme: &Theme) {
    let now = app.power_history.back().copied();
    let peak = app.power_history.iter().copied().fold(0.0_f64, f64::max);
    let block = theme.panel("Power draw · 5min", false).title_top(
        Line::from(vec![
            Span::raw(" "),
            Span::styled(
                format!("now {:.1}W", now.unwrap_or(0.0)),
                Style::default().fg(theme.accent),
            ),
            Span::styled(" · ", Style::default().fg(theme.dim)),
            Span::styled(
                format!("peak {:.1}W", peak),
                Style::default().fg(theme.warn),
            ),
        ])
        .right_aligned(),
    );
    let inner = Rect {
        x: block.inner(area).x + 1,
        y: block.inner(area).y,
        width: block.inner(area).width.saturating_sub(2),
        height: block.inner(area).height,
    };
    frame.render_widget(block, area);

    if app.power_history.is_empty() {
        frame.render_widget(Paragraph::new(" …").centered(), inner);
        return;
    }
    let n = app.power_history.len() as f64;
    let pts: Vec<(f64, f64)> = app
        .power_history
        .iter()
        .enumerate()
        .map(|(i, w)| (i as f64, *w))
        .collect();
    let y_max = peak.max(5.0);
    let chart = Chart::new(vec![Dataset::default()
        .marker(ratatui::symbols::Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::new().fg(theme.accent))
        .data(&pts)])
    .x_axis(
        Axis::default()
            .style(Style::new().fg(theme.dim))
            .bounds([0.0, n.max(1.0)])
            .labels(vec![Span::raw("-5m"), Span::raw("now")]),
    )
    .y_axis(
        Axis::default()
            .style(Style::new().fg(theme.dim))
            .bounds([0.0, y_max])
            .labels(vec![Span::raw("0"), Span::raw(format!("{y_max:.0}"))]),
    );
    frame.render_widget(chart, inner);
}

/// Process listesi (compact): name + est_w + cpu%.
fn render_procs(app: &App, frame: &mut Frame, area: Rect, theme: &Theme) {
    let block = theme.panel("Top processes · est W", false);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.processes.is_empty() {
        frame.render_widget(Paragraph::new(" …").centered(), inner);
        return;
    }
    let max_rows = inner.height as usize;
    let rows: Vec<Row> = app
        .processes
        .iter()
        .take(max_rows.saturating_sub(1).max(1))
        .map(|p| {
            let w_color = if p.est_w >= 1.0 {
                theme.accent_alt
            } else if p.est_w >= 0.3 {
                theme.warn
            } else {
                theme.fg
            };
            Row::new(vec![
                Cell::from(truncate(p.name.as_str(), inner.width as usize / 2)),
                Cell::from(format!("{:.2}W", p.est_w)).style(Style::default().fg(w_color)),
                Cell::from(format!("{:.0}%", p.cpu_pct)).style(Style::default().fg(theme.dim)),
            ])
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Fill(1),
            Constraint::Length(7),
            Constraint::Length(6),
        ],
    )
    .column_spacing(1);
    frame.render_widget(table, inner);
}

/// Alt şerit: sistem mini-bar'ları (CPU / parlaklık / sıcaklık) tek satırda.
fn render_sys_strip(app: &App, frame: &mut Frame, area: Rect, theme: &Theme) {
    let block = theme.panel("System", false);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let sys = app.sys.as_ref();
    let cpu = sys.and_then(|m| m.cpu_load);
    let bright = sys.and_then(|m| m.brightness);
    let temp = sys.and_then(|m| m.temperature);

    let mut spans: Vec<Span> = vec![Span::raw(" ")];
    add_bar(&mut spans, theme, "CPU", cpu, Some((CPU_WARN, CPU_DANGER)));
    spans.push(Span::raw("  "));
    add_bar(&mut spans, theme, "Bright", bright, None);
    spans.push(Span::raw("  "));
    add_bar(
        &mut spans,
        theme,
        "Temp",
        temp,
        Some((TEMP_WARN, TEMP_DANGER)),
    );
    frame.render_widget(Paragraph::new(Line::from(spans)), inner);
}

fn add_bar(
    spans: &mut Vec<Span>,
    theme: &Theme,
    label: &str,
    value: Option<f64>,
    sev: Option<(f64, f64)>,
) {
    spans.push(Span::styled(
        format!("{label} "),
        Style::default().fg(theme.dim),
    ));
    if let Some(v) = value {
        let color = sev
            .map(|(w, d)| theme.severity(v, w, d))
            .unwrap_or(theme.fg);
        spans.extend(widgets::mini_bar_spans(theme, v, color));
        spans.push(Span::styled(
            format!(" {v:>3.0}"),
            Style::default().fg(color),
        ));
    } else {
        spans.push(Span::styled("—", Style::default().fg(theme.dim)));
    }
}

/// Uzun metni kırp.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let kept: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{kept}…")
    }
}
