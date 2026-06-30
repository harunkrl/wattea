//! Live sekmesi: hero (BatteryCell + % + hız) + 2×3 metrik ızgarası +
//! geniş güç çizgi grafiği. Dar terminallerde sütunlar dikey istiflenir.

use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Axis, Chart, Dataset, GraphType, Paragraph},
    Frame,
};

use crate::app::App;
use crate::battery::Status;
use crate::ui::theme::{Theme, CPU_DANGER, CPU_WARN, TEMP_DANGER, TEMP_WARN};
use crate::ui::widgets;

use super::opt_fmt;

/// Live sekmesini çizer.
pub fn render(app: &App, frame: &mut Frame, area: Rect, theme: &Theme) {
    // Dar terminallerde sütunları dikey istifle (70 kolon altı).
    let wide = area.width >= 70;
    let [hero, right] = if wide {
        Layout::horizontal([Constraint::Percentage(38), Constraint::Fill(1)]).areas(area)
    } else {
        Layout::vertical([Constraint::Length(9), Constraint::Fill(1)]).areas(area)
    };

    render_hero(app, frame, hero, theme);
    // grid: 2 satır × kart. spark: Fill → hero ile aynı yükseklikte bitir.
    let [grid_area, spark_area] =
        Layout::vertical([Constraint::Length(10), Constraint::Fill(1)]).areas(right);
    render_metric_grid(app, frame, grid_area, theme);
    render_spark(app, frame, spark_area, theme);
}

fn render_hero(app: &App, frame: &mut Frame, area: Rect, theme: &Theme) {
    let block = theme.panel("Charge", true);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(s) = &app.sample else {
        frame.render_widget(Paragraph::new(" …").centered(), inner);
        return;
    };

    let charging = s.status == Status::Charging;
    let color = theme.charge_color(s.capacity, charging);

    // İçerik: border'dan +1 sol/sağ padding (yapışıklığı önler).
    let pad = Rect {
        x: inner.x + 1,
        y: inner.y,
        width: inner.width.saturating_sub(2),
        height: inner.height,
    };
    // [üst boşluk][bar+% (2 satır)][status][ara boşluk][info paneli]
    let [_, bar_row, status_row, _, info_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(pad);

    // Kalın batarya çubuğu (2 satır) + büyük % (sağda, dikey ortalanmış).
    let [bar_area, pct_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(7)]).areas(bar_row);
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
        .centered(),
        pct_area,
    );

    // Status satırı (ikon + etiket).
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                format!("{} {}", s.status.glyph(), s.status.label()),
                Style::default().fg(color),
            ),
        ])),
        status_row,
    );

    // Info paneli: label:value satırları (normal aralık, renk kodlu).
    let rate = s.pct_per_hour();
    let rate_str = match rate {
        Some(r) => format!("{r:+.1} %/h"),
        None => "—".into(),
    };
    let health = s.health();
    let mut info: Vec<Line> = vec![
        info_line(theme, "Rate", rate_str, color),
        info_line(theme, "Power", opt_fmt(s.power_now, "W", 2), theme.fg),
        info_line(theme, "Voltage", opt_fmt(s.voltage, "V", 3), theme.fg),
        info_line(
            theme,
            "Energy",
            format!(
                "{:.1} / {:.1} Wh",
                s.energy_now.unwrap_or(0.0),
                s.energy_full
            ),
            theme.fg,
        ),
        info_line(
            theme,
            "Design",
            format!("{:.1} Wh", s.energy_full_design),
            theme.dim,
        ),
        info_line(
            theme,
            "Health",
            format!("{health:.1}%"),
            theme.charge_color(health as u8, false),
        ),
        info_line(
            theme,
            "Cycles",
            opt_fmt(s.cycle_count.map(|c| c as f64), "", 0),
            theme.dim,
        ),
        info_line(theme, "Time", opt_dur(s.time_remaining()), theme.accent),
    ];

    // Korelasyon notu (spec §7.3): deşarj + CPU yüksekse ipucu.
    if s.status == Status::Discharging
        && app
            .sys
            .as_ref()
            .and_then(|m| m.cpu_load)
            .map(|v| v >= CPU_WARN)
            .unwrap_or(false)
    {
        info.push(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                "⚡ drain ↑ ~ CPU load",
                Style::default().fg(theme.accent_alt),
            ),
        ]));
    }
    frame.render_widget(Paragraph::new(info), info_area);
}

/// `Label   value` satırı: etiket dim (sabit genişlik), değer renkli.
fn info_line(theme: &Theme, label: &str, value: String, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::raw(" "),
        Span::styled(format!("{label:<8}"), Style::default().fg(theme.dim)),
        Span::styled(value, Style::default().fg(color)),
    ])
}

fn render_metric_grid(app: &App, frame: &mut Frame, area: Rect, theme: &Theme) {
    let s = match &app.sample {
        Some(s) => s,
        None => {
            frame.render_widget(theme.panel("Metrics", false), area);
            return;
        }
    };
    let sys = app.sys.as_ref();

    // Öncelik sırasıyla kartlar: (label, value, color, Option<bar_pct>).
    // 2 sütun modunda (rows*cols=4) en kritik 4 korunur.
    let health = s.health();
    let cards: Vec<(&str, String, Color, Option<f64>)> = vec![
        (
            "Health",
            format!("{health:.1}%"),
            theme.charge_color(health as u8, false),
            Some(health),
        ),
        (
            "CPU load",
            opt_fmt(sys.and_then(|m| m.cpu_load), "%", 0),
            sys.and_then(|m| m.cpu_load)
                .map(|v| theme.severity(v, CPU_WARN, CPU_DANGER))
                .unwrap_or(theme.dim),
            sys.and_then(|m| m.cpu_load),
        ),
        (
            "Temp",
            opt_fmt(sys.and_then(|m| m.temperature), "°C", 0),
            sys.and_then(|m| m.temperature)
                .map(|v| theme.severity(v, TEMP_WARN, TEMP_DANGER))
                .unwrap_or(theme.dim),
            sys.and_then(|m| m.temperature),
        ),
        ("Power", opt_fmt(s.power_now, "W", 2), theme.fg, None),
        ("Voltage", opt_fmt(s.voltage, "V", 3), theme.fg, None),
        ("Time", opt_dur(s.time_remaining()), theme.fg, None),
    ];

    // Sağ sütun yeterince genişse 3 sütun (6 kart), yoksa 2 sütun (4 kart).
    let cols = if area.width >= 48 { 3 } else { 2 };
    let rows = 2usize;
    let take = rows * cols;
    let row_rects = Layout::vertical(vec![Constraint::Fill(1); rows]).split(area);
    for r in 0..rows {
        let cells = Layout::horizontal(vec![Constraint::Fill(1); cols]).split(row_rects[r]);
        for c in 0..cols {
            let idx = r * cols + c;
            if idx >= take || idx >= cards.len() {
                break;
            }
            let (label, value, color, bar) = &cards[idx];
            // Önce border, sonra içeriği dikey+yatay ortala (border'a boşluk bırak).
            let block = theme.panel(label, false);
            let inner = block.inner(cells[c]);
            frame.render_widget(block, cells[c]);
            let content_h: u16 = if bar.is_some() { 2 } else { 1 };
            let cy = inner.y + inner.height.saturating_sub(content_h) / 2;
            let content_area = Rect {
                y: cy,
                height: content_h,
                ..inner
            };
            // Değer: yatay ortalanmış, büyük.
            frame.render_widget(
                Paragraph::new(Line::from(vec![Span::styled(
                    value.clone(),
                    Style::default().fg(*color).add_modifier(Modifier::BOLD),
                )]))
                .alignment(Alignment::Center),
                content_area,
            );
            // Bar (varsa): değerin altında, yatay ortalanmış.
            if let Some(pct) = bar {
                let bar_area = Rect {
                    y: cy + 1,
                    height: 1,
                    ..content_area
                };
                frame.render_widget(
                    Paragraph::new(Line::from(widgets::mini_bar_spans(theme, *pct, *color)))
                        .alignment(Alignment::Center),
                    bar_area,
                );
            }
        }
    }
}

fn render_spark(app: &App, frame: &mut Frame, area: Rect, theme: &Theme) {
    // Trend ile tutarlı: Braille çizgi grafiği (bar sparkline değil).
    let now = app.power_history.back().copied();
    let peak = app.power_history.iter().copied().fold(0.0_f64, f64::max);

    // Başlık: sol "Power draw · last 5 min (W)" + sağ now/peak rozeti.
    let block = theme.panel("Power draw · last 5 min (W)", false).title_top(
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
    // Grafik içeriğini border'dan biraz uzaklaştır (padding).
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
    let y_max = peak.max(5.0); // en az 5W ölçek, düz çizgiyi önle

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

/// Option<Duration> → "Hh MMm" (None → "—"). (live-only yardımcı.)
fn opt_dur(d: Option<std::time::Duration>) -> String {
    match d {
        Some(d) => {
            let mins = d.as_secs() / 60;
            format!("{}h {:02}m", mins / 60, mins % 60)
        }
        None => "—".into(),
    }
}
