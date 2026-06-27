//! TestBackend ile UI render'ının kırılmadığını doğrular.

use ratatui::{backend::TestBackend, Terminal};

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
    let mut app = App::new(&fake_battery());
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
fn renders_gracefully_with_no_sample_yet() {
    let app = App::new(&fake_battery()); // sample = None

    let backend = TestBackend::new(90, 24);
    let mut term = Terminal::new(backend).unwrap();
    term.draw(|f| ui::view(&app, f)).unwrap();
}
