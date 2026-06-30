//! Sessions sekmesi: on-battery oturum tablosu. Üst özet kartı + severity
//! renklendirmesi (yüksek tüketim accent_alt) + footer toplamları.

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Style, Stylize},
    text::Line,
    widgets::{Cell, Paragraph, Row, Table, TableState},
    Frame,
};

use crate::app::App;
use crate::ui::theme::{Theme, SEVERE_DRAIN_PCT_H};

/// Sessions sekmesini çizer.
pub fn render(app: &App, frame: &mut Frame, area: Rect, theme: &Theme) {
    let block = theme.panel(
        &format!("Sessions · on-battery  ·  {} total", app.sessions.len()),
        true,
    );

    if app.sessions.is_empty() {
        let msg = if app.store.is_none() {
            " No history store. Run `wattea import` + the daemon. "
        } else {
            " No discharging sessions yet — unplug and let the daemon collect. "
        };
        frame.render_widget(Paragraph::new(msg).centered().block(block), area);
        return;
    }

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [sum_area, tbl_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Fill(1)]).areas(inner);

    render_summary_card(app, frame, sum_area, theme);
    render_table(app, frame, tbl_area, theme);
}

fn render_summary_card(app: &App, frame: &mut Frame, area: Rect, theme: &Theme) {
    let total_dur: i64 = app.sessions.iter().map(|s| s.duration_secs()).sum();
    let total_drop: i16 = app.sessions.iter().map(|s| s.capacity_drop().max(0)).sum();
    let total_hours = total_dur as f64 / 3600.0;
    let avg_pct_h = if total_hours > 0.0 {
        total_drop as f64 / total_hours
    } else {
        0.0
    };

    let line = Line::from(vec![
        ratatui::text::Span::raw(" sessions "),
        ratatui::text::Span::styled(
            app.sessions.len().to_string(),
            Style::new().fg(theme.accent).bold(),
        ),
        ratatui::text::Span::raw("  · on battery ").fg(theme.dim),
        ratatui::text::Span::styled(fmt_duration(total_dur), Style::new().fg(theme.fg)),
        ratatui::text::Span::raw("  · avg ").fg(theme.dim),
        ratatui::text::Span::styled(format!("{avg_pct_h:.1} %/h"), Style::new().fg(theme.warn)),
        ratatui::text::Span::raw("  · consumed ").fg(theme.dim),
        ratatui::text::Span::styled(format!("{total_drop}%"), Style::new().fg(theme.accent_alt)),
    ]);
    frame.render_widget(
        Paragraph::new(line).block(theme.panel("Summary", false)),
        area,
    );
}

fn render_table(app: &App, frame: &mut Frame, area: Rect, theme: &Theme) {
    let header = Row::new([
        Cell::from("When"),
        Cell::from("Duration"),
        Cell::from("% start→end"),
        Cell::from("Drop"),
        Cell::from("Avg %/h"),
        Cell::from("Avg W"),
        Cell::from("N"),
    ])
    .height(1)
    .style(Style::new().fg(theme.accent).bold());

    let total_w: f64 = app.sessions.iter().filter_map(|s| s.avg_power()).sum();
    let total_n: usize = app.sessions.iter().map(|s| s.sample_count).sum();

    let mut rows: Vec<Row> = app
        .sessions
        .iter()
        .map(|s| {
            let drop = s.capacity_drop();
            let drop_color = if drop >= 15 {
                theme.accent_alt
            } else if drop >= 5 {
                theme.warn
            } else {
                theme.ok
            };
            let pct_h = s.avg_pct_per_hour();
            let ph_color = match pct_h {
                Some(v) if v >= SEVERE_DRAIN_PCT_H => theme.accent_alt,
                Some(v) if v >= 5.0 => theme.warn,
                Some(_) => theme.ok,
                None => theme.dim,
            };
            Row::new(vec![
                Cell::from(fmt_session_when(s.start_ts)),
                Cell::from(fmt_duration(s.duration_secs())),
                Cell::from(format!("{} → {}", s.start_capacity, s.end_capacity)),
                Cell::from(format!("{drop:+}%")).style(Style::default().fg(drop_color)),
                Cell::from(
                    pct_h
                        .map(|v| format!("{v:.1}"))
                        .unwrap_or_else(|| "—".into()),
                )
                .style(Style::default().fg(ph_color)),
                Cell::from(
                    s.avg_power()
                        .map(|v| format!("{v:.1}"))
                        .unwrap_or_else(|| "—".into()),
                ),
                Cell::from(s.sample_count.to_string()),
            ])
        })
        .collect();

    // Footer toplamları.
    let total_dur: i64 = app.sessions.iter().map(|s| s.duration_secs()).sum();
    let total_drop: i16 = app.sessions.iter().map(|s| s.capacity_drop().max(0)).sum();
    let total_hours = total_dur as f64 / 3600.0;
    let avg_pct_h = if total_hours > 0.0 {
        total_drop as f64 / total_hours
    } else {
        0.0
    };
    rows.push(
        Row::new(vec![
            Cell::from("totals"),
            Cell::from(fmt_duration(total_dur)),
            Cell::from(""),
            Cell::from(format!("{total_drop:+}%")),
            Cell::from(format!("{avg_pct_h:.1}")),
            Cell::from(if total_n > 0 {
                format!("{:.1}", total_w / total_n as f64)
            } else {
                "—".into()
            }),
            Cell::from(total_n.to_string()),
        ])
        .style(Style::default().fg(theme.dim)),
    );

    let table = Table::new(
        rows,
        [
            Constraint::Length(20),
            Constraint::Length(10),
            Constraint::Length(11),
            Constraint::Length(7),
            Constraint::Length(8),
            Constraint::Length(7),
            Constraint::Length(5),
        ],
    )
    .header(header)
    .row_highlight_style(Style::default().bg(theme.surface));
    frame.render_stateful_widget(table, area, &mut TableState::default());
}

// --- sessions-only yardımcılar -----------------------------------------------

/// Oturum başlangıç zamanını "Mon DD HH:MM" biçiminde göster (UTC kabaca).
fn fmt_session_when(ts: i64) -> String {
    let days = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let secs = ts.rem_euclid(86400);
    let day_secs = 86400;
    let epoch_weekday = 4; // 1970-01-01 Perşembe
    let weekday = ((ts.div_euclid(day_secs) + epoch_weekday) as usize) % 7;
    let hour = (secs / 3600) as u8;
    let minute = ((secs % 3600) / 60) as u8;
    let day_of_month = 1 + (ts.div_euclid(day_secs) as usize % 28); // yaklaşık
    format!(
        "{} {:>2} {:02}:{:02}",
        days[weekday], day_of_month, hour, minute
    )
}

/// Saniyeyi "1h 23m" / "45m" / "30s" biçimine çevir.
fn fmt_duration(secs: i64) -> String {
    let secs = secs.max(0);
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        format!("{m}m")
    } else {
        format!("{secs}s")
    }
}
