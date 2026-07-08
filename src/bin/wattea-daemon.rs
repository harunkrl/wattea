//! wattea-daemon — background battery collection service.
//!
//! Reads sysfs every N seconds and stores it in SQLite. Suitable for running
//! as a systemd user service: requires no root, writes to its own data
//! directory (`~/.local/share/wattea/db.sqlite`). Shuts down cleanly on
//! SIGINT/SIGTERM.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use color_eyre::eyre::{Context, Result};
use wattea::battery::Battery;
use wattea::process::ProcessReader;
use wattea::storage::{Sample, Store};

/// Default sampling interval (seconds).
// 60 seconds: power readings are smoothed over a few seconds; sampling more
// often adds little information, while keeping disk writes and CPU low.
const DEFAULT_INTERVAL_SECS: u64 = 60;

/// Process snapshot retention (7 days).
const PROCESS_RETENTION_SECS: u64 = 7 * 24 * 3600;
/// Number of processes kept per sample (top-N).
const PROCESS_TOP_N: usize = 15;

fn main() -> Result<()> {
    color_eyre::install()?;
    let interval = std::env::var("WATTEA_INTERVAL")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_INTERVAL_SECS);

    let db = wattea::db_path()
        .ok_or_else(|| color_eyre::eyre::eyre!("data directory not found (XDG_DATA_HOME?)"))?;

    eprintln!("wattea-daemon: db = {}", db.display());
    eprintln!("wattea-daemon: interval = {interval}s");

    let battery = Battery::detect()?;
    let store = Store::open(&db)?;
    let mut sys = wattea::system::SystemReader::new();
    let mut procs = ProcessReader::new();

    // First sample immediately (so the service starts collecting data right away).
    collect_once(&battery, &store, &mut sys, &mut procs)?;

    run_loop(battery, store, Duration::from_secs(interval), sys, procs)
}

/// Sampling loop: samples every `interval` until a signal is received.
fn run_loop(
    battery: Battery,
    store: Store,
    interval: Duration,
    mut sys: wattea::system::SystemReader,
    mut procs: ProcessReader,
) -> Result<()> {
    let stop = install_signal_handlers();

    while !stop.load(Ordering::SeqCst) {
        // The daemon is not async: a plain thread::sleep loop.
        // Responsive sleep: check every 500ms to react quickly to a signal.
        let mut waited = Duration::ZERO;
        let step = Duration::from_millis(500);
        while waited < interval && !stop.load(Ordering::SeqCst) {
            std::thread::sleep(step);
            waited += step;
        }
        if stop.load(Ordering::SeqCst) {
            break;
        }
        collect_once(&battery, &store, &mut sys, &mut procs)?;
    }

    eprintln!("wattea-daemon: shutting down");
    Ok(())
}

fn collect_once(
    battery: &Battery,
    store: &Store,
    sys: &mut wattea::system::SystemReader,
    procs: &mut ProcessReader,
) -> Result<()> {
    let sample = battery.read().context("battery read")?;
    let metrics = sys.read();
    let now = std::time::SystemTime::now();
    let ts = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let record = Sample::new(ts, &sample, &metrics);
    store.insert(&record)?;

    // Process snapshots: top-N estimated power + retention cleanup.
    let top = procs.top(PROCESS_TOP_N);
    if !top.is_empty()
        && let Err(e) = store.insert_process_snapshots(ts, &top)
    {
        eprintln!("wattea-daemon: failed to write process snapshot: {e:#}");
    }
    if let Err(e) = store.prune_process_snapshots(PROCESS_RETENTION_SECS) {
        eprintln!("wattea-daemon: process prune error: {e:#}");
    }
    Ok(())
}

/// Install an atomic flag for SIGINT/SIGTERM. Compatible with systemd `KillSignal`.
fn install_signal_handlers() -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));

    #[cfg(unix)]
    {
        use signal_hook::consts::{SIGINT, SIGTERM};
        use signal_hook::flag;
        // Safe flag handlers that set `stop` to `true` on each signal.
        // `register` wants an `Arc<AtomicBool>`; give a clone to each signal.
        for sig in [SIGINT, SIGTERM] {
            let _ = flag::register(sig, Arc::clone(&stop));
        }
    }

    stop
}
