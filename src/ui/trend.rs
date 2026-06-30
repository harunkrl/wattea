//! Trend sekmesi: dikey stacked grafikler — üstte kapasite % (sabit 0–100),
//! altta güç W (dinamik [0, peak.max(15)] bounds). Reviewer'ın yakaladığı
//! "power tek eksende sıkışıyor" hatasının düzeltmesi.

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style, Stylize},
    text::Span,
    widgets::{Axis, Chart, Dataset, GraphType, Paragraph},
    Frame,
};

use crate::app::App;

use super::opt_fmt;

/// Trend sekmesini çizer.
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

    // --- Capacity chart (sabit 0–100) + critical 20% çizgisi ---
    let crit_pts = [(0.0, 20.0), (span_h, 20.0)];
    let cap_chart = Chart::new(vec![
        Dataset::default()
            // legend yok: panel başlığı "Capacity %" + özet zaten açıklıyor.
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

    // Critical 20% çizgisinin (warn rengi) satırını bul ve o yüksekliğe
    // sol eksende "20" yaz (ratatui Axis >3 etiketi kırık olduğu için manuel).
    label_critical_20(frame, cap_area, theme.warn);

    // Peak işaretçisi kaldırıldı: Scatter+Block büyük/kaba görünüyordu.
    // Peak değeri özet çubuğunda ("peak X.XW") accent_alt ile yazılı.
    let pow_chart = Chart::new(vec![Dataset::default()
        // legend yok: panel başlığı "Power W" zaten açıklıyor.
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

/// Trend sekmesinin özet çubuğu: örnek sayısı, ortalama/zirve güç, % düşüşü.
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

// --- trend-only yardımcılar ---------------------------------------------------

/// Power grafiği için dinamik y-bounds: `[0.0, peak.max(15.0)]`.
/// Critical 20% çizgisinin (warn rengindeki Braille hücreler) satırını bulup
/// sol eksende o yüksekliğe "20" yazar (warn renginde).
/// ratatui Axis 3'ten fazla etiketi kırık yerleştirdiği için manuel yapılır.
fn label_critical_20(frame: &mut Frame, area: Rect, warn: Color) {
    // warn renkli ilk hücreyi bul → y=20 satırı + plot sol kenarı.
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
        _ => return, // çizgi bulunamadı (çok küçük alan)
    };
    // "20"yi plot sol kenarından hemen önceye yaz (etiketlerle sağa hizalı).
    if plot_left >= 2 {
        let a = &mut buf[(plot_left - 2, row)];
        a.set_symbol("2");
        a.fg = warn;
        let b = &mut buf[(plot_left - 1, row)];
        b.set_symbol("0");
        b.fg = warn;
    }
}

/// 15W döşemesi, düşük tüketimde grafiğin düzleşmesini engeller.
fn power_bounds(powers: &[f64]) -> [f64; 2] {
    let peak = powers.iter().copied().fold(0.0_f64, f64::max);
    [0.0, peak.max(15.0)]
}

/// Şimdiki Unix zaman damgası (saniye).
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
