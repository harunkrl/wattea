//! wattea — TUI batarya paneli.
//!
//! Canlı sysfs verisinden ratatui dashboard çizer.

use std::time::Duration;

use color_eyre::eyre::Result;
use crossterm::event::{Event, EventStream, KeyCode};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use tokio::{select, time::interval};

use wattea::app::{App, Message};
use wattea::battery::Battery;
use wattea::storage::Store;
use wattea::ui;
use wattea::upower;

fn main() -> Result<()> {
    color_eyre::install()?;

    // `wattea import` → UPower backfill. `wattea export [file]` → CSV.
    // `wattea` → TUI.
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && (args[1] == "import" || args[1] == "backfill") {
        return run_import();
    }
    if args.len() > 1 && args[1] == "export" {
        return run_export(args.get(2).map(|s| s.as_str()));
    }

    let battery = Battery::detect()?;
    // DB varsa aç: hem canlı sysfs hem SQLite history birlikte gösterilir.
    let store = wattea::db_path().and_then(|p| Store::open(&p).ok());

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, battery, store);
    ratatui::restore();

    result
}

/// `wattea import`: UPower `.dat` history'sini SQLite'a aktar (geri-dolum).
fn run_import() -> Result<()> {
    let battery = Battery::detect()?;
    let db = wattea::db_path().ok_or_else(|| color_eyre::eyre::eyre!("veri dizini bulunamadı"))?;
    let store = Store::open(&db)?;
    let before = store.count()?;

    let live = battery.read()?;
    let n = upower::import(
        &store,
        battery.info.model.as_deref(),
        live.energy_full,
        live.energy_full_design,
    )?;
    let after = store.count()?;

    println!("🔋 UPower history → SQLite");
    println!("   import edilen örnek: {n}");
    println!("   depodaki toplam    : {before} → {after}");
    println!("   veritabanı         : {}", db.display());
    Ok(())
}

/// `wattea export [file]`: tüm örnekleri CSV'ye aktar.
/// Dosya verilmezse stdout'a yazar.
fn run_export(dest: Option<&str>) -> Result<()> {
    let db = wattea::db_path().ok_or_else(|| color_eyre::eyre::eyre!("veri dizini bulunamadı"))?;
    let store = Store::open(&db)?;
    let samples = store.query_all()?;

    let csv = to_csv(&samples);
    match dest {
        Some(path) => {
            std::fs::write(path, &csv)?;
            println!("✅ {} satır → {}", samples.len(), path);
        }
        None => {
            use std::io::Write;
            let mut out = std::io::stdout().lock();
            out.write_all(csv.as_bytes())?;
        }
    }
    Ok(())
}

/// Basit, güvenli CSV üretimi (RFC 4180 alıntılama).
fn to_csv(samples: &[wattea::storage::Sample]) -> String {
    let header = "timestamp,datetime_utc,capacity_pct,status,power_w,voltage_v,energy_now_wh,energy_full_wh,cycle_count,cpu_load_pct,brightness_pct,temperature_c";
    let mut out = String::from(header);
    out.push('\n');
    for s in samples {
        let dt = format_ts_utc(s.ts);
        let power = s.power_now.map(|v| format!("{v:.4}")).unwrap_or_default();
        let voltage = s.voltage.map(|v| format!("{v:.4}")).unwrap_or_default();
        let energy_now = s.energy_now.map(|v| format!("{v:.4}")).unwrap_or_default();
        let row = format!(
            "{},{},{},{},{},{},{},{:.4},{},{:.2},{:.2},{:.2}\n",
            s.ts,
            csv_field(&dt),
            s.capacity,
            csv_field(s.status.label()),
            power,
            voltage,
            energy_now,
            s.energy_full,
            s.cycle_count.unwrap_or(0),
            opt2(s.cpu_load),
            opt2(s.brightness),
            opt2(s.temperature),
        );
        out.push_str(&row);
    }
    out
}

/// Alan virgül/tırnak/yeni satır içeriyorsa çift tırnakla sar.
fn csv_field(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Option<f64> → CSV alanı (None ise boş).
fn opt2(v: Option<f64>) -> String {
    v.map(|x| format!("{x:.2}")).unwrap_or_default()
}

/// Unix epoch → "YYYY-MM-DD HH:MM:SS" (UTC).
fn format_ts_utc(ts: i64) -> String {
    let days = ts.div_euclid(86400);
    let secs = ts.rem_euclid(86400);
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    // 1970-01-01'den itibaren gün → yıl/ay/gün (Gregorian).
    let (y, mo, d) = days_to_ymd(days);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}:{s:02}")
}

fn days_to_ymd(days_since_epoch: i64) -> (i32, u32, u32) {
    // Howard Hinnant'in civil_from_days algoritması.
    let z = days_since_epoch + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (y as i32 + if m <= 2 { 1 } else { 0 }, m as u32, d as u32)
}

/// Async ana döngü: her tick'te sysfs'i oku, klavye olaylarını dinle.
fn run(terminal: &mut DefaultTerminal, battery: Battery, store: Option<Store>) -> Result<()> {
    install_panic_hook();

    let mut app = App::new(&battery, store);
    app.refresh(&battery); // ilk örnekleme hemen

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let mut events = EventStream::new();
        let mut tick = interval(Duration::from_secs(1));

        loop {
            terminal.draw(|frame| ui::view(&app, frame))?;

            select! {
                Some(Ok(event)) = events.next() => {
                    if let Event::Key(key) = event {
                        let msg = match key.code {
                            KeyCode::Char('q') | KeyCode::Esc => Message::Quit,
                            KeyCode::Char('r') => Message::Refresh,
                            KeyCode::Tab => Message::NextTab,
                            KeyCode::Char('1') => {
                                app.goto_tab(wattea::app::Tab::Live);
                                continue;
                            }
                            KeyCode::Char('2') => {
                                app.goto_tab(wattea::app::Tab::Trend);
                                continue;
                            }
                            KeyCode::Char('3') => {
                                app.goto_tab(wattea::app::Tab::Pattern);
                                continue;
                            }
                            KeyCode::Char('4') => {
                                app.goto_tab(wattea::app::Tab::Sessions);
                                continue;
                            }
                            KeyCode::Char('d') if app.tab == wattea::app::Tab::Pattern => {
                                Message::TogglePatternAxis
                            }
                            _ => continue,
                        };
                        handle(&mut app, msg, &battery);
                    }
                }
                _ = tick.tick() => handle(&mut app, Message::Refresh, &battery),
            }

            if app.should_quit {
                break;
            }
        }
        Ok::<(), color_eyre::Report>(())
    })?;
    Ok(())
}

fn handle(app: &mut App, msg: Message, battery: &Battery) {
    match msg {
        Message::Refresh => app.refresh(battery),
        Message::NextTab => app.next_tab(),
        Message::TogglePatternAxis => app.toggle_pattern_axis(),
        Message::Quit => app.should_quit = true,
    }
}

fn install_panic_hook() {
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        ratatui::restore();
        original_hook(panic_info);
    }));
}
