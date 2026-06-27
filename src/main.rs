//! Wattea — terminal batarya tüketim takipçisi.
//!
//! sysfs'ten her saniye örnekler, ratatui ile canlı bir panel çizer:
//! doluluk Gauge'i, güç sparkline'ı, %/saat tüketim, sağlık, döngü.

use std::time::Duration;

use color_eyre::eyre::Result;
use crossterm::event::{Event, EventStream, KeyCode};
use futures::StreamExt;
use ratatui::DefaultTerminal;
use tokio::{select, time::interval};

mod app;
mod battery;
#[cfg(test)]
mod tests;
mod ui;

use app::{App, Message};
use battery::Battery;

fn main() -> Result<()> {
    color_eyre::install()?;
    let battery = Battery::detect()?;

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, battery);
    ratatui::restore();

    result
}

/// Async ana döngü: her tick'te sysfs'i oku, klavye olaylarını dinle.
fn run(terminal: &mut DefaultTerminal, battery: Battery) -> Result<()> {
    // color_eyre + ratatui::init() panik'te terminal'i geri yükler;
    // ek olarak panik hook'u ile restore'u garanti altına al.
    install_panic_hook();

    let mut app = App::new(&battery);
    // İlk örnekleme hemen (UI'da boş ekran göstermemek için).
    app.refresh(&battery);

    // tokio runtime: async event stream + tick interval aynı anda.
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
