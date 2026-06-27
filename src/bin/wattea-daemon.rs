//! wattea-daemon — arka plan batarya toplama servisi.
//!
//! sysfs'i her N saniyede bir okur ve SQLite'a kaydeder. systemd user
//! service olarak çalışmaya uygun: root gerektirmez, kendi veri dizinine
//! (`~/.local/share/wattea/db.sqlite`) yazar. SIGINT/SIGTERM ile temiz kapanır.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use color_eyre::eyre::{Context, Result};
use wattea::battery::Battery;
use wattea::storage::{Sample, Store};

/// Varsayılan örnekleme aralığı (saniye).
// 60 saniye: güç ölçümleri birkaç sn smoothing'li; daha sık örneklemek fazla
// bilgi getirmez, oysa disk yazışını ve CPU'yu düşük tutar.
const DEFAULT_INTERVAL_SECS: u64 = 60;

fn main() -> Result<()> {
    color_eyre::install()?;
    let interval = std::env::var("WATTEA_INTERVAL")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_INTERVAL_SECS);

    let db = wattea::db_path()
        .ok_or_else(|| color_eyre::eyre::eyre!("veri dizini bulunamadı (XDG_DATA_HOME?)"))?;

    eprintln!("wattea-daemon: db = {}", db.display());
    eprintln!("wattea-daemon: interval = {interval}s");

    let battery = Battery::detect()?;
    let store = Store::open(&db)?;

    // İlk örnekleme hemen (servis bir an önce veri toplamaya başlasın).
    collect_once(&battery, &store)?;

    run_loop(battery, store, Duration::from_secs(interval))
}

/// Örnek bir döngü: sinyal gelene dek her `interval`'de bir örnekler.
fn run_loop(battery: Battery, store: Store, interval: Duration) -> Result<()> {
    let stop = install_signal_handlers();

    while !stop.load(Ordering::SeqCst) {
        // Daemon async değil: sade bir thread::sleep döngüsü.
        // Hassas uyku: her 500ms kontrol ederek sinyale hızlı tepki ver.
        let mut waited = Duration::ZERO;
        let step = Duration::from_millis(500);
        while waited < interval && !stop.load(Ordering::SeqCst) {
            std::thread::sleep(step);
            waited += step;
        }
        if stop.load(Ordering::SeqCst) {
            break;
        }
        collect_once(&battery, &store)?;
    }

    eprintln!("wattea-daemon: kapatılıyor");
    Ok(())
}

fn collect_once(battery: &Battery, store: &Store) -> Result<()> {
    let sample = battery.read().context("batarya okuma")?;
    let now = std::time::SystemTime::now();
    let record = Sample::from((&now, &sample));
    store.insert(&record)?;
    Ok(())
}

/// SIGINT/SIGTERM için atomik bayrak kur. systemd `KillSignal` ile uyumlu.
fn install_signal_handlers() -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));

    #[cfg(unix)]
    {
        use signal_hook::consts::{SIGINT, SIGTERM};
        use signal_hook::flag;
        // Sinyal geldikçe `stop`'u `true` yapan güvenli bayrak handler'ları.
        // `register` bir `Arc<AtomicBool>` ister; her sinyale bir klon ver.
        for sig in [SIGINT, SIGTERM] {
            let _ = flag::register(sig, Arc::clone(&stop));
        }
    }

    stop
}
