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

    // `wattea import` → UPower history backfill. `wattea` → TUI.
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && (args[1] == "import" || args[1] == "backfill") {
        return run_import();
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
