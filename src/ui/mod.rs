//! Render — Gauge (doluluk), Sparkline (güç trendi), metrikler.
//!
//! Lib crate'in parçasıdır, böylece render testleri (TestBackend) çalışabilir.
//!
//! Bu modül kökü (mod.rs) yalnızca dispatch + header + help + paylaşılan
//! yardımcıları içerir. Sekme çizimleri `live`, `trend`, `pattern`,
//! `sessions` alt modüllerinde; özel widget'lar `widgets` modülündedir.

pub mod compact;
pub mod live;
pub mod pattern;
pub mod processes;
pub mod sessions;
pub mod theme;
pub mod trend;
pub mod widgets;

pub use theme::Theme;

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::app::App;

/// Ana görünüm: sekme seçimine göre ilgili paneli çizer.
pub fn view(app: &App, frame: &mut Frame) {
    let theme = Theme::electric();
    let area = frame.area();

    let [header, body, help_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);

    render_header(app, frame, header, &theme);
    if app.compact {
        compact::render(app, frame, body, &theme);
    } else {
        match app.tab {
            crate::app::Tab::Live => live::render(app, frame, body, &theme),
            crate::app::Tab::Trend => trend::render(app, frame, body, &theme),
            crate::app::Tab::Pattern => pattern::render(app, frame, body, &theme),
            crate::app::Tab::Sessions => sessions::render(app, frame, body, &theme),
            crate::app::Tab::Processes => processes::render(app, frame, body, &theme),
        }
    }
    render_help(app, frame, help_area, &theme);
}

fn render_header(app: &App, frame: &mut Frame, area: Rect, theme: &Theme) {
    frame.render_widget(widgets::branded_header(theme, app.anomalies.len()), area);
}

fn render_help(app: &App, frame: &mut Frame, area: Rect, theme: &Theme) {
    let tabs: [(&str, bool); 5] = [
        ("1 Live", app.tab == crate::app::Tab::Live),
        ("2 Trend", app.tab == crate::app::Tab::Trend),
        ("3 Pattern", app.tab == crate::app::Tab::Pattern),
        ("4 Sessions", app.tab == crate::app::Tab::Sessions),
        ("5 Procs", app.tab == crate::app::Tab::Processes),
    ];
    let mut spans = vec![Span::raw(" ")];
    for (label, active) in tabs {
        if active {
            spans.push(Span::styled(
                format!(" {label} "),
                Style::default().fg(theme.bg).bg(theme.accent).bold(),
            ));
        } else {
            spans.push(Span::styled(
                format!(" {label} "),
                Style::default().fg(theme.dim),
            ));
        }
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(" Tab ", Style::default().fg(theme.accent).bold()));
    spans.push(Span::styled("switch ", Style::default().fg(theme.dim)));
    spans.push(Span::styled(" c ", Style::default().fg(theme.accent).bold()));
    spans.push(
        Span::styled(
            if app.compact { "tabs " } else { "compact " },
            Style::default().fg(theme.dim),
        ),
    );
    if app.tab == crate::app::Tab::Pattern {
        spans.push(Span::styled(" d ", Style::default().fg(theme.accent).bold()));
        spans.push(Span::styled("hour/day ", Style::default().fg(theme.dim)));
    }
    if app.tab == crate::app::Tab::Processes {
        spans.push(Span::styled(" d ", Style::default().fg(theme.accent).bold()));
        spans.push(Span::styled(
            if app.process_view == crate::app::ProcessView::LastHour {
                "live "
            } else {
                "last hour "
            },
            Style::default().fg(theme.dim),
        ));
    }
    spans.push(Span::styled(" r ", Style::default().fg(theme.accent).bold()));
    spans.push(Span::styled("refresh ", Style::default().fg(theme.dim)));
    spans.push(Span::styled(" q ", Style::default().fg(theme.accent).bold()));
    spans.push(Span::styled("quit ", Style::default().fg(theme.dim)));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// Option<f64> → "<değer> <birim>" (None → "—").
/// Live ve Trend sekmeleri ortak kullanır → pub(super).
pub(super) fn opt_fmt(v: Option<f64>, unit: &str, prec: usize) -> String {
    match v {
        Some(x) => format!("{x:.*} {unit}", prec),
        None => "—".into(),
    }
}
