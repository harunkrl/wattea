//! Processes tab: per-process **estimated** power consumption (live).
//!
//! This is an estimate (attribution) — not an exact watt value: CPU-time share
//! × RAPL total CPU power. When RAPL cannot be read, only CPU% is shown.

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Style, Stylize},
    text::Line,
    widgets::{Cell, Paragraph, Row, Table, TableState},
    Frame,
};

use crate::app::App;
use crate::ui::theme::Theme;

/// Render the Processes tab.
pub fn render(app: &App, frame: &mut Frame, area: Rect, theme: &Theme) {
    let [summary_area, tbl_area] =
        Layout::vertical([Constraint::Length(3), Constraint::Fill(1)]).areas(area);

    match app.process_view {
        crate::app::ProcessView::Live => {
            render_summary_live(app, frame, summary_area, theme);
            render_table_live(app, frame, tbl_area, theme);
        }
        crate::app::ProcessView::LastHour => {
            render_summary_history(app, frame, summary_area, theme);
            render_table_history(app, frame, tbl_area, theme);
        }
    }
}

fn render_summary_live(app: &App, frame: &mut Frame, area: Rect, theme: &Theme) {
    let n = app.processes.len();
    let total_est: f64 = app.processes.iter().map(|p| p.est_w).sum();
    let total_cpu: f64 = app.processes.iter().map(|p| p.cpu_pct).sum();
    let rapl_note = if app.processes.iter().any(|p| p.est_w > 0.0) {
        "RAPL: ok"
    } else {
        "RAPL: unavailable (CPU% only)"
    };

    let line = Line::from(vec![
        ratatui::text::Span::raw(" processes "),
        ratatui::text::Span::styled(n.to_string(), Style::default().fg(theme.accent).bold()),
        ratatui::text::Span::raw("  · top est. ").fg(theme.dim),
        ratatui::text::Span::styled(format!("{total_est:.2} W"), Style::default().fg(theme.warn)),
        ratatui::text::Span::raw("  · CPU% ").fg(theme.dim),
        ratatui::text::Span::styled(format!("{total_cpu:.0}%"), Style::default().fg(theme.fg)),
        ratatui::text::Span::raw("  · ").fg(theme.dim),
        ratatui::text::Span::styled(rapl_note, Style::default().fg(theme.dim)),
    ]);
    frame.render_widget(
        Paragraph::new(line).block(theme.panel("Summary", false)),
        area,
    );
}

fn render_table_live(app: &App, frame: &mut Frame, area: Rect, theme: &Theme) {
    let block = theme.panel("Top processes · estimated CPU power share", true);

    if app.processes.is_empty() {
        frame.render_widget(
            Paragraph::new(" collecting process samples… (wait 1s) ")
                .centered()
                .block(block),
            area,
        );
        return;
    }

    let header = Row::new([
        Cell::from("Process"),
        Cell::from("PID"),
        Cell::from("CPU%"),
        Cell::from("est. W"),
    ])
    .height(1)
    .style(Style::default().fg(theme.accent).bold());

    let rows = app.processes.iter().map(|p| {
        // Color by est_w: emphasize the high ones.
        let w_color = if p.est_w >= 1.0 {
            theme.accent_alt
        } else if p.est_w >= 0.3 {
            theme.warn
        } else {
            theme.fg
        };
        let cpu_color = if p.cpu_pct >= 50.0 {
            theme.accent_alt
        } else if p.cpu_pct >= 20.0 {
            theme.warn
        } else {
            theme.fg
        };
        Row::new(vec![
            Cell::from(truncate_name(&p.name, 28)),
            Cell::from(p.pid.to_string()),
            Cell::from(format!("{:.1}", p.cpu_pct)).style(Style::default().fg(cpu_color)),
            Cell::from(format!("{:.2}", p.est_w)).style(Style::default().fg(w_color)),
        ])
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(30),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(8),
        ],
    )
    .header(header)
    .block(block)
    .row_highlight_style(Style::default().bg(theme.surface));
    frame.render_stateful_widget(table, area, &mut TableState::default());
}

fn render_summary_history(app: &App, frame: &mut Frame, area: Rect, theme: &Theme) {
    let n = app.process_history.len();
    let total_avg: f64 = app.process_history.iter().map(|p| p.avg_w).sum();
    let note = if app.store.is_none() {
        "no store (run daemon)"
    } else if n == 0 {
        "collecting… (wait a few minutes)"
    } else {
        "window: last 1h"
    };
    let line = Line::from(vec![
        ratatui::text::Span::raw(" processes "),
        ratatui::text::Span::styled(n.to_string(), Style::default().fg(theme.accent).bold()),
        ratatui::text::Span::raw("  · top est. ").fg(theme.dim),
        ratatui::text::Span::styled(
            format!("{total_avg:.2} W avg",),
            Style::default().fg(theme.warn),
        ),
        ratatui::text::Span::raw("  · ").fg(theme.dim),
        ratatui::text::Span::styled(note, Style::default().fg(theme.dim)),
    ]);
    frame.render_widget(
        Paragraph::new(line).block(theme.panel("Summary", false)),
        area,
    );
}

fn render_table_history(app: &App, frame: &mut Frame, area: Rect, theme: &Theme) {
    let block = theme.panel("Top processes · last 1h aggregate (avg W)", true);
    if app.process_history.is_empty() {
        let msg = if app.store.is_none() {
            " No history store (run `wattea import` + the daemon). "
        } else {
            " Collecting process history… (run the daemon for a few minutes) "
        };
        frame.render_widget(Paragraph::new(msg).centered().block(block), area);
        return;
    }
    let header = Row::new([
        Cell::from("Process"),
        Cell::from("avg W"),
        Cell::from("avg CPU%"),
        Cell::from("samples"),
    ])
    .height(1)
    .style(Style::default().fg(theme.accent).bold());
    let rows = app.process_history.iter().map(|p| {
        let w_color = if p.avg_w >= 1.0 {
            theme.accent_alt
        } else if p.avg_w >= 0.3 {
            theme.warn
        } else {
            theme.fg
        };
        Row::new(vec![
            Cell::from(truncate_name(&p.name, 28)),
            Cell::from(format!("{:.2}", p.avg_w)).style(Style::default().fg(w_color)),
            Cell::from(format!("{:.1}", p.avg_cpu_pct)),
            Cell::from(p.sample_count.to_string()),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(30),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Length(8),
        ],
    )
    .header(header)
    .block(block)
    .row_highlight_style(Style::default().bg(theme.surface));
    frame.render_stateful_widget(table, area, &mut TableState::default());
}

/// Truncate long process names (the name is usually comm, 15 chars; kept as a fallback).
fn truncate_name(name: &str, max: usize) -> String {
    if name.chars().count() <= max {
        name.to_string()
    } else {
        let kept: String = name.chars().take(max - 1).collect();
        format!("{kept}…")
    }
}
