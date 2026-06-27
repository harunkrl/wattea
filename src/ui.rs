//! Render — Gauge (doluluk), Sparkline (güç trendi), metrikler.
//!
//! Lib crate'in parçasıdır, böylece render testleri (TestBackend) çalışabilir.

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span},
    widgets::{Axis, Block, Borders, Chart, Dataset, Gauge, GraphType, Paragraph, Sparkline},
};

use crate::app::App;
use crate::battery::{BatterySample, Status};

/// Ana görünüm: sekme seçimine göre Live veya Trend paneli.
pub fn view(app: &App, frame: &mut Frame) {
    let area = frame.area();

    let [header, body, help_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);

    render_header(app, frame, header);
    match app.tab {
        crate::app::Tab::Live => render_live(app, frame, body),
        crate::app::Tab::Trend => render_trend(app, frame, body),
    }
    render_help(app, frame, help_area);
}

/// Live sekmesi: gauge + metrics + sparkline + health.
fn render_live(app: &App, frame: &mut Frame, area: Rect) {
    let [body, spark_area, health_area] = Layout::vertical([
        Constraint::Length(9),
        Constraint::Length(7),
        Constraint::Length(3),
    ])
    .areas(area);

    let [gauge_area, metrics_area] =
        Layout::horizontal([Constraint::Percentage(40), Constraint::Fill(1)]).areas(body);

    render_gauge(app, frame, gauge_area);
    render_metrics(app, frame, metrics_area);
    render_sparkline(app, frame, spark_area);
    render_health(app, frame, health_area);
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

/// Trend sekmesi: SQLite'ten 24 saatlik kapasite & güç çizgi grafiği.
fn render_trend(app: &App, frame: &mut Frame, area: Rect) {
    let [chart_area, summary_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(5)]).areas(area);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!("Trend · last 24h  ·  {} samples", app.trend.len()));

    if app.trend.is_empty() {
        let msg = if app.store.is_none() {
            " No history store (run `wattea import` + the daemon). "
        } else {
            " No data yet in the last 24h. "
        };
        frame.render_widget(Paragraph::new(msg).centered().block(block), chart_area);
        return;
    }

    let now = chrono_now_ts();
    let x_min = (now - crate::app::TREND_WINDOW_SECS as i64) as f64;
    let x_max = now as f64;

    // Verileri (x=ts, y=değer) noktalarına çevir; x'i 0-tabanlı saate normalize et.
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

    let datasets = vec![
        Dataset::default()
            .name("capacity %")
            .marker(ratatui::symbols::Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::new().green())
            .data(&cap_pts),
        Dataset::default()
            .name("power W")
            .marker(ratatui::symbols::Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::new().yellow())
            .data(&pow_pts),
    ];

    let chart = Chart::new(datasets)
        .block(block)
        .x_axis(
            Axis::default()
                .title("hours ago")
                .style(Style::new().cyan())
                .bounds([0.0, (x_max - x_min) / 3600.0])
                .labels(vec![Span::raw("-24h"), Span::raw("-12h"), Span::raw("now")]),
        )
        .y_axis(
            Axis::default()
                .title("% / W")
                .style(Style::new().cyan())
                .bounds([0.0, 100.0])
                .labels(vec![Span::raw("0"), Span::raw("50"), Span::raw("100")]),
        );
    frame.render_widget(chart, chart_area);

    render_trend_summary(app, frame, summary_area);
}

/// Trend sekmesinin özet satırı: ortalama/zirve güç, % düşüşü.
fn render_trend_summary(app: &App, frame: &mut Frame, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title("Summary");

    let powers: Vec<f64> = app.trend.iter().filter_map(|s| s.power_now).collect();
    let avg = (!powers.is_empty()).then(|| powers.iter().sum::<f64>() / powers.len() as f64);
    let peak = powers.iter().cloned().fold(0.0_f64, f64::max);
    let drop = (!app.trend.is_empty()).then(|| {
        app.trend.first().unwrap().capacity as i16 - app.trend.last().unwrap().capacity as i16
    });

    let line = Line::from(vec![
        Span::raw(" samples "),
        Span::styled(format!("{}", app.trend.len()), Style::new().bold()),
        Span::raw("  · avg power ").dim(),
        Span::raw(opt_fmt(avg, "W", 2)),
        Span::raw("  · peak ").dim(),
        Span::styled(format!("{peak:.2} W"), Style::new().yellow()),
        Span::raw("  · charge delta ").dim(),
        Span::styled(
            drop.map(|d| format!("{d:+}%"))
                .unwrap_or_else(|| "—".into()),
            Style::new().fg(match drop {
                Some(d) if d > 0 => Color::Red,
                Some(d) if d < 0 => Color::Green,
                _ => Color::Reset,
            }),
        ),
    ]);
    frame.render_widget(Paragraph::new(line).block(block), area);
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

fn render_help(app: &App, frame: &mut Frame, area: Rect) {
    let tabs: [(&str, bool); 2] = [
        ("1 Live", app.tab == crate::app::Tab::Live),
        ("2 Trend", app.tab == crate::app::Tab::Trend),
    ];
    let mut spans = vec![Span::raw(" ")];
    for (label, active) in tabs {
        if active {
            spans.push(Span::styled(
                format!(" {label} "),
                Style::new().black().on_cyan().bold(),
            ));
        } else {
            spans.push(Span::raw(format!(" {label} ")).dim());
        }
        spans.push(Span::raw(" "));
    }
    spans.push(" Tab ".bold().cyan());
    spans.push("switch ".dim());
    spans.push(" r ".bold().cyan());
    spans.push("refresh ".dim());
    spans.push(" q ".bold().cyan());
    spans.push("quit ".dim());
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

// --- yardımcılar --------------------------------------------------------------

/// Şimdiki Unix zaman damgası (saniye). Trend x-ekseni normalizasyonu için.
fn chrono_now_ts() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

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
