//! TestBackend ile UI render'ının kırılmadığını doğrular.

use ratatui::{Terminal, backend::TestBackend};

use crate::app::App;
use crate::battery::{Battery, BatteryInfo, BatterySample, Status};
use crate::ui;

/// Sahte (gerçek sysfs gerektirmeyen) bir örnekleme.
fn fake_sample() -> BatterySample {
    BatterySample {
        capacity: 57,
        status: Status::Charging,
        power_now: Some(17.36),
        voltage: Some(16.41),
        energy_now: Some(28.64),
        energy_full: 50.6,
        energy_full_design: 56.0,
        cycle_count: Some(110),
    }
}

#[test]
fn metric_computations_are_correct() {
    let s = fake_sample();
    // %/saat = 17.36 / 50.6 * 100 ≈ 34.3
    assert!((s.pct_per_hour().unwrap() - 34.31).abs() < 0.1);
    // sağlık = 50.6 / 56.0 * 100 ≈ 90.36
    assert!((s.health() - 90.36).abs() < 0.1);
    // şarjda → "time to full" hesaplanır olmalı
    assert!(s.time_remaining().is_some());
}

#[test]
fn discharge_time_to_empty() {
    let mut s = fake_sample();
    s.status = Status::Discharging;
    s.power_now = Some(10.0);
    s.energy_now = Some(40.0);
    // 40 Wh / 10 W = 4 saat
    let dur = s.time_remaining().unwrap();
    assert_eq!(dur.as_secs(), 4 * 3600);
}

fn fake_battery() -> Battery {
    Battery {
        dir: "/tmp".into(),
        info: BatteryInfo {
            name: "BAT0".into(),
            manufacturer: Some("SMP".into()),
            model: Some("L21M4PD0".into()),
            technology: Some("Li-poly".into()),
        },
    }
}

#[test]
fn renders_without_panic_on_small_and_large_areas() {
    let mut app = App::new(&fake_battery(), None);
    app.sample = Some(fake_sample());
    app.power_history.extend([5.0, 8.0, 12.0, 6.0, 10.0]);

    // Dar ve geniş alanların ikisinde de kırılmadan çizmeli.
    for (w, h) in [(80, 24), (120, 40), (60, 20)] {
        let backend = TestBackend::new(w, h);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| ui::view(&app, f)).unwrap();
    }
}

#[test]
fn pattern_tab_renders_with_and_without_data() {
    use crate::app::Tab;
    use crate::storage::HourlyBin;

    let mut app = App::new(&fake_battery(), None);
    app.sample = Some(fake_sample());
    app.tab = Tab::Pattern;

    // 1) Boş pattern (veri yok): kırılmadan çizmeli.
    for (w, h) in [(80, 24), (120, 40)] {
        let backend = TestBackend::new(w, h);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| ui::view(&app, f)).unwrap();
    }

    // 2) Dolu pattern: 24 saatlik sahte sepetler.
    app.pattern = (0..24)
        .map(|hour| HourlyBin {
            hour,
            avg_pct_per_hour: 10.0 + hour as f64,
            sample_count: 5,
        })
        .collect();
    for (w, h) in [(100, 30), (140, 50)] {
        let backend = TestBackend::new(w, h);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| ui::view(&app, f)).unwrap();
    }
}

#[test]
fn sessions_tab_renders_with_and_without_data() {
    use crate::app::Tab;
    use crate::storage::Session;

    let mut app = App::new(&fake_battery(), None);
    app.sample = Some(fake_sample());
    app.tab = Tab::Sessions;

    // 1) Boş (oturum yok).
    for (w, h) in [(80, 24), (120, 40)] {
        let backend = TestBackend::new(w, h);
        let mut term = Terminal::new(backend).unwrap();
        term.draw(|f| ui::view(&app, f)).unwrap();
    }

    // 2) Dolu: 3 sahte oturum.
    app.sessions = vec![
        Session {
            start_ts: 1000,
            end_ts: 6400,
            start_capacity: 90,
            end_capacity: 80,
            power_sum: 50.0,
            sample_count: 10,
        },
        Session {
            start_ts: 10000,
            end_ts: 10000,
            start_capacity: 50,
            end_capacity: 50,
            power_sum: 0.0,
            sample_count: 1,
        },
        Session {
            start_ts: 20000,
            end_ts: 56000,
            start_capacity: 40,
            end_capacity: 20,
            power_sum: 200.0,
            sample_count: 60,
        },
    ];
    let backend = TestBackend::new(120, 30);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| ui::view(&app, f)).unwrap();
}

#[test]
fn session_metrics_are_correct() {
    use crate::storage::Session;
    let s = Session {
        start_ts: 0,
        end_ts: 3600, // 1 saat
        start_capacity: 80,
        end_capacity: 70,
        power_sum: 100.0,
        sample_count: 60,
    };
    assert_eq!(s.capacity_drop(), 10);
    assert_eq!(s.duration_secs(), 3600);
    // 10% / 1h = 10 %/h
    assert!((s.avg_pct_per_hour().unwrap() - 10.0).abs() < 0.01);
    // 100W·60 örnek / 60 = 1.67W ortalama
    assert!((s.avg_power().unwrap() - 1.667).abs() < 0.01);
}

#[test]
fn renders_gracefully_with_no_sample_yet() {
    let app = App::new(&fake_battery(), None); // sample = None

    let backend = TestBackend::new(90, 24);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| ui::view(&app, f)).unwrap();
}
