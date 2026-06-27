//! Render — Gauge (doluluk), Sparkline (güç trendi), metrikler.
//!
//! Lib crate'in parçasıdır, böylece render testleri (TestBackend) çalışabilir.

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph, Sparkline},
};

use crate::app::App;
use crate::battery::{BatterySample, Status};

/// Ana görünüm.
pub fn view(app: &App, frame: &mut Frame) {
    let area = frame.area();

    let [header, body, spark_area, health_area, help_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(9),
        Constraint::Length(7),
        Constraint::Length(3),
        Constraint::Length(1),
    ])
    .areas(area);

    render_header(app, frame, header);
    render_body(app, frame, body);
    render_sparkline(app, frame, spark_area);
    render_health(app, frame, health_area);
    render_help(frame, help_area);
}

fn render_header(app: &App, frame: &mut Frame, area: Rect) {
    let title = Line::from(vec![
        Span::raw(" "),
        Span::styled(
            "Wattea",
            Style::new().cyan().bold().add_modifier(Modifier::ITALIC),
        ),
        Span::raw("  battery consumption tracker").dim(),
    ]);

    let mut right = vec![Span::raw(" ")];
    if !app.info.name.is_empty() {
        right.push(Span::raw(app.info.name.as_str()).dim());
    }
    if let Some(m) = &app.info.model {
        right.push(Span::raw(" · ").dim());
        right.push(Span::raw(m).dim());
    }
    if let Some(m) = &app.info.manufacturer {
        right.push(Span::raw(" · ").dim());
        right.push(Span::raw(m).dim());
    }
    if let Some(t) = &app.info.technology {
        right.push(Span::raw(" · ").dim());
        right.push(Span::raw(t).dim());
    }

    let block = Block::default()
        .borders(Borders::TOP)
        .title(title)
        .title_bottom(Line::from(right).right_aligned());

    frame.render_widget(block, area);
}

fn render_body(app: &App, frame: &mut Frame, area: Rect) {
    let [gauge_area, metrics_area] =
        Layout::horizontal([Constraint::Percentage(40), Constraint::Fill(1)]).areas(area);

    render_gauge(app, frame, gauge_area);
    render_metrics(app, frame, metrics_area);
}

fn render_gauge(app: &App, frame: &mut Frame, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title("Charge");

    let Some(s) = &app.sample else {
        frame.render_widget(Paragraph::new("…").centered().block(block), area);
        return;
    };

    let color = capacity_color(s);
    let glyph = s.status.glyph();
    let label = format!(" {glyph} {}% ", s.capacity);

    let gauge = Gauge::default()
        .block(block)
        .gauge_style(Style::new().fg(color))
        .percent(u16::from(s.capacity))
        .label(label.bold());
    frame.render_widget(gauge, area);
}

fn render_metrics(app: &App, frame: &mut Frame, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title("Live metrics");

    let lines: Vec<Line> = match &app.sample {
        Some(s) => {
            let mut lines = Vec::with_capacity(7);
            lines.push(metric_line(
                "Status",
                format!("{} {}", s.status.glyph(), s.status.label()),
                status_color(s),
            ));
            lines.push(metric_line(
                "Power",
                opt_fmt(s.power_now, "W", 2),
                Color::Reset,
            ));
            lines.push(metric_line(
                "Drain rate",
                opt_fmt(s.pct_per_hour(), "%/h", 1),
                Color::Reset,
            ));
            lines.push(metric_line(
                "Voltage",
                opt_fmt(s.voltage, "V", 3),
                Color::Reset,
            ));
            lines.push(metric_line(
                match s.status {
                    Status::Charging => "Time to full",
                    _ => "Time to empty",
                },
                opt_dur(s.time_remaining()),
                Color::Reset,
            ));
            lines.push(metric_line(
                "Cycles",
                opt_fmt(s.cycle_count.map(|c| c as f64), "", 0),
                Color::Reset,
            ));
            lines.push(metric_line(
                "Updated",
                app.last_update
                    .map(|t| format!("{:.0}s ago", t.elapsed().as_secs()))
                    .unwrap_or_else(|| "—".into()),
                Color::DarkGray,
            ));
            lines
        }
        None => match &app.last_error {
            Some(e) => vec![Line::from(format!(" error: {e}")).red()],
            None => vec![Line::from(" reading battery… ").dim()],
        },
    };

    let para = Paragraph::new(lines).block(block);
    frame.render_widget(para, area);
}

fn render_sparkline(app: &App, frame: &mut Frame, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Power draw · last 5 min (W)");

    let data: Vec<u64> = app
        .power_history
        .iter()
        .map(|w| (w * 100.0) as u64) // 0.01 W çözünürlük
        .collect();

    let max = data.iter().copied().max().unwrap_or(1);
    let current = app
        .sample
        .as_ref()
        .and_then(|s| s.power_now)
        .map(|w| format!("{w:.2} W now · peak {:.2} W", max as f64 / 100.0))
        .unwrap_or_default();

    let sparkline = Sparkline::default()
        .data(data)
        .block(block)
        .max(max)
        .style(Style::new().fg(Color::LightYellow));

    frame.render_widget(sparkline, area);

    // Akım değerini köşeye yaz (sparkline bloğunun iç altına küçük overlay).
    if !current.is_empty() {
        let badge = Paragraph::new(current.clone())
            .dim()
            .alignment(Alignment::Right);
        let inner = Rect {
            y: area.bottom().saturating_sub(2),
            ..area
        };
        frame.render_widget(badge, inner);
    }
}

fn render_health(app: &App, frame: &mut Frame, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title("Health");

    let line = match &app.sample {
        Some(s) => {
            let health = s.health();
            let color = if health >= 80.0 {
                Color::Green
            } else if health >= 60.0 {
                Color::Yellow
            } else {
                Color::Red
            };
            let degr = if s.energy_full_design > 0.0 {
                format!(
                    " · {:.1} Wh / {:.1} Wh design",
                    s.energy_full, s.energy_full_design
                )
            } else {
                String::new()
            };
            Line::from(vec![
                Span::raw(" Health "),
                Span::styled(format!("{health:.1}%"), Style::new().fg(color).bold()),
                Span::raw(degr).dim(),
            ])
        }
        None => Line::from(" —").dim(),
    };

    frame.render_widget(Paragraph::new(line).block(block), area);
}

fn render_help(frame: &mut Frame, area: Rect) {
    let help = Line::from(vec![
        " r ".bold().cyan(),
        "refresh ".dim(),
        " q ".bold().cyan(),
        "quit ".dim(),
        "  Wattea ".dim(),
    ]);
    frame.render_widget(Paragraph::new(help), area);
}

// --- yardımcılar --------------------------------------------------------------

/// ` Label `  ` value ` biçiminde bir satır; etiket sol, değer sağ.
fn metric_line(label: &str, value: String, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::raw(" "),
        Span::raw(format!("{label:<12}")).dim(),
        Span::styled(value, Style::new().fg(color)),
    ])
}

fn opt_fmt(v: Option<f64>, unit: &str, prec: usize) -> String {
    match v {
        Some(x) => format!("{x:.*} {unit}", prec),
        None => "—".into(),
    }
}

fn opt_dur(d: Option<std::time::Duration>) -> String {
    match d {
        Some(d) => {
            let mins = d.as_secs() / 60;
            format!("{}h {:02}m", mins / 60, mins % 60)
        }
        None => "—".into(),
    }
}

/// Doluluğa göre renk: şarjda cyan, değilse yüksek/mid/düşük.
fn capacity_color(s: &BatterySample) -> Color {
    if s.status == Status::Charging {
        Color::Cyan
    } else if s.capacity >= 50 {
        Color::Green
    } else if s.capacity >= 20 {
        Color::Yellow
    } else {
        Color::Red
    }
}

fn status_color(s: &BatterySample) -> Color {
    match s.status {
        Status::Charging => Color::Cyan,
        Status::Discharging => Color::LightGreen,
        Status::Full => Color::Green,
        _ => Color::DarkGray,
    }
}
