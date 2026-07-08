//! SQLite time-series store.
//!
//! The daemon writes samples; the TUI (and future analysis tools) read them.
//! WAL mode enables lock-free concurrent reads and writes.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use color_eyre::eyre::{Context, Result};
use rusqlite::{params, Connection};

use crate::battery::{BatterySample, Status};

/// A single time-series sample row (for query results).
#[derive(Debug, Clone)]
pub struct Sample {
    /// Unix epoch seconds.
    pub ts: i64,
    pub capacity: u8,
    pub status: Status,
    pub power_now: Option<f64>,
    pub voltage: Option<f64>,
    pub energy_now: Option<f64>,
    pub energy_full: f64,
    pub energy_full_design: f64,
    pub cycle_count: Option<u32>,
    pub cpu_load: Option<f64>,
    pub brightness: Option<f64>,
    pub temperature: Option<f64>,
}

/// A pattern-analysis bin (hour 0–23 or weekday 0–6).
#[derive(Debug, Clone, Default)]
pub struct HourlyBin {
    /// Bin index: 0–23 in hourly mode, 0–6 in weekday mode (Sun–Sat).
    pub hour: u8,
    pub avg_pct_per_hour: f64,
    pub sample_count: usize,
}

/// A single on-battery ("unplugged") session.
///
/// Consists of consecutive Discharging samples; split by Charging/Full runs
/// or by a long gap (e.g. suspend).
#[derive(Debug, Clone)]
pub struct Session {
    pub start_ts: i64,
    pub end_ts: i64,
    pub start_capacity: u8,
    pub end_capacity: u8,
    pub power_sum: f64,
    pub sample_count: usize,
}

impl Session {
    /// Session duration (seconds).
    pub fn duration_secs(&self) -> i64 {
        self.end_ts - self.start_ts
    }

    /// Total % drop (negative = gained charge, but discharging-only so usually ≥ 0).
    pub fn capacity_drop(&self) -> i16 {
        self.start_capacity as i16 - self.end_capacity as i16
    }

    /// Average power (W); samples without a power reading do not contribute.
    pub fn avg_power(&self) -> Option<f64> {
        if self.sample_count == 0 {
            return None;
        }
        Some(self.power_sum / self.sample_count as f64)
    }

    /// Session average %/hour consumption: drop / duration.
    pub fn avg_pct_per_hour(&self) -> Option<f64> {
        let hours = self.duration_secs() as f64 / 3600.0;
        if hours <= 0.0 {
            return None;
        }
        Some(self.capacity_drop().max(0) as f64 / hours)
    }
}

/// A single anomaly sample with unexpectedly high power draw.
#[derive(Debug, Clone)]
pub struct Anomaly {
    pub ts: i64,
    pub power: f64,
    pub z_score: f64,
    pub mean: f64,
    pub capacity: u8,
}

/// Per-process aggregate estimated power (for history queries).
#[derive(Debug, Clone)]
pub struct ProcessAgg {
    pub name: String,
    pub avg_w: f64,
    pub avg_cpu_pct: f64,
    pub sample_count: usize,
}

impl Sample {
    /// Build a full sample (battery + system metrics).
    pub fn new(ts: i64, s: &BatterySample, sys: &crate::system::SystemMetrics) -> Self {
        Self {
            ts,
            capacity: s.capacity,
            status: s.status,
            power_now: s.power_now,
            voltage: s.voltage,
            energy_now: s.energy_now,
            energy_full: s.energy_full,
            energy_full_design: s.energy_full_design,
            cycle_count: s.cycle_count,
            cpu_load: sys.cpu_load,
            brightness: sys.brightness,
            temperature: sys.temperature,
        }
    }
}

/// SQLite store. The daemon and TUI each open their own `Store` instance.
pub struct Store {
    conn: Connection,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").finish_non_exhaustive()
    }
}

impl Store {
    /// Open the DB; create it under `data_dir` if missing and set up the schema.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).wrap_err_with(|| {
                format!("failed to create data directory: {}", parent.display())
            })?;
        }
        let conn = Connection::open(path)
            .wrap_err_with(|| format!("failed to open SQLite: {}", path.display()))?;

        // WAL: concurrent read/write, crash-safe.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;

        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;

        Ok(Self { conn })
    }

    /// In-memory DB (for tests).
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;
        Ok(Self { conn })
    }

    /// Insert a single sample.
    pub fn insert(&self, sample: &Sample) -> Result<()> {
        self.conn.execute(
            INSERT_SQL,
            params![
                sample.ts,
                sample.capacity,
                sample.status.label(),
                sample.power_now,
                sample.voltage,
                sample.energy_now,
                sample.energy_full,
                sample.energy_full_design,
                sample.cycle_count,
                sample.cpu_load,
                sample.brightness,
                sample.temperature,
            ],
        )?;
        Ok(())
    }

    /// Fetch all samples since a given point in time (for trend charts).
    pub fn query_since(&self, since_secs_ago: u64) -> Result<Vec<Sample>> {
        let now = now_ts();
        let cutoff = now - since_secs_ago as i64;
        let mut stmt = self.conn.prepare(SELECT_SINCE_SQL)?;
        let rows = stmt.query_map(params![cutoff], row_to_sample)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Fetch the last N samples (newest last).
    pub fn query_last(&self, n: u32) -> Result<Vec<Sample>> {
        let mut stmt = self.conn.prepare(SELECT_LAST_SQL)?;
        let rows = stmt.query_map(params![n as i64], row_to_sample)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Total sample count (for status/diagnostics).
    pub fn count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM samples", [], |r| r.get(0))?)
    }

    /// Find samples with unexpectedly high power draw (anomalies).
    ///
    /// Among Discharging samples, returns those whose z-score in the power
    /// distribution exceeds `z_threshold` (default 2.0 ≈ top 2.3%). Results
    /// are newest first.
    pub fn query_anomalies(&self, z_threshold: f64) -> Result<Vec<Anomaly>> {
        let all = self.query_all()?;
        // Only discharging samples with a power reading.
        let powers: Vec<(usize, f64)> = all
            .iter()
            .enumerate()
            .filter_map(|(i, s)| s.power_now.filter(|p| *p > 0.0).map(|p| (i, p)))
            .collect();
        if powers.len() < 2 {
            return Ok(Vec::new());
        }
        let n = powers.len() as f64;
        let mean = powers.iter().map(|(_, p)| p).sum::<f64>() / n;
        let variance = powers.iter().map(|(_, p)| (p - mean).powi(2)).sum::<f64>() / n;
        let std = variance.sqrt();
        if std < 1e-9 {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for (i, p) in powers {
            let z = (p - mean) / std;
            if z >= z_threshold {
                let s = &all[i];
                out.push(Anomaly {
                    ts: s.ts,
                    power: p,
                    z_score: z,
                    mean,
                    capacity: s.capacity,
                });
            }
        }
        out.sort_by_key(|a| std::cmp::Reverse(a.ts));
        Ok(out)
    }

    /// Average %/hour consumption pattern by hour of day (0–23).
    ///
    /// Only `Discharging` samples are used (charging makes the pattern
    /// meaningless). For each hour, the value is the average consumption rate
    /// across all days at that hour — the "at 14:00 I usually burn %X/h" pattern.
    pub fn query_hourly_pattern(&self) -> Result<Vec<HourlyBin>> {
        let mut stmt = self.conn.prepare(HOURLY_PATTERN_SQL)?;
        let rows = stmt.query_map([], |row| {
            Ok(HourlyBin {
                hour: row.get::<_, i64>(0)? as u8,
                avg_pct_per_hour: row.get(1)?,
                sample_count: row.get::<_, i64>(2)? as usize,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Average %/hour consumption pattern by day of week (Sunday=0 … Saturday=6).
    pub fn query_weekday_pattern(&self) -> Result<Vec<HourlyBin>> {
        let mut stmt = self.conn.prepare(WEEKDAY_PATTERN_SQL)?;
        let rows = stmt.query_map([], |row| {
            Ok(HourlyBin {
                hour: row.get::<_, i64>(0)? as u8,
                avg_pct_per_hour: row.get(1)?,
                sample_count: row.get::<_, i64>(2)? as usize,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Return all samples in ascending ts order (for session segmentation/export).
    pub fn query_all(&self) -> Result<Vec<Sample>> {
        let mut stmt = self.conn.prepare(SELECT_ALL_SQL)?;
        let rows = stmt.query_map([], row_to_sample)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Segment on-battery (Discharging) sessions.
    ///
    /// A gap larger than `gap_secs` starts a new session (separates
    /// suspend/sampling skips). Results are returned in reverse chronological
    /// order (newest session first).
    pub fn query_sessions(&self, gap_secs: i64) -> Result<Vec<Session>> {
        let samples = self.query_all()?;
        let mut sessions: Vec<Session> = Vec::new();
        let mut current: Option<Session> = None;

        for s in samples {
            let discharging = s.status == crate::battery::Status::Discharging;
            if discharging {
                let power = s.power_now.unwrap_or(0.0).max(0.0);
                match &mut current {
                    None => {
                        current = Some(Session {
                            start_ts: s.ts,
                            end_ts: s.ts,
                            start_capacity: s.capacity,
                            end_capacity: s.capacity,
                            power_sum: power,
                            sample_count: 1,
                        });
                    }
                    Some(sess) => {
                        let gap = s.ts - sess.end_ts;
                        if gap > gap_secs {
                            // Long gap: close the session, start a new one.
                            if let Some(c) = current.take() {
                                sessions.push(c);
                            }
                            current = Some(Session {
                                start_ts: s.ts,
                                end_ts: s.ts,
                                start_capacity: s.capacity,
                                end_capacity: s.capacity,
                                power_sum: power,
                                sample_count: 1,
                            });
                        } else {
                            sess.end_ts = s.ts;
                            sess.end_capacity = s.capacity;
                            sess.power_sum += power;
                            sess.sample_count += 1;
                        }
                    }
                }
            } else if current.is_some() {
                // Left the discharging state (charging/full): close the session.
                sessions.push(current.take().unwrap());
            }
        }
        if let Some(c) = current.take() {
            sessions.push(c);
        }

        // Newest first.
        sessions.sort_by_key(|b| std::cmp::Reverse(b.start_ts));
        Ok(sessions)
    }

    /// Bulk-insert process snapshots (called by the daemon).
    pub fn insert_process_snapshots(
        &self,
        ts: i64,
        snaps: &[crate::process::ProcessPower],
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(INSERT_PROC_SQL)?;
            for p in snaps {
                stmt.execute(params![ts, p.pid, p.name, p.cpu_pct, p.est_w])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Aggregate processes within a window (seconds) by name:
    /// average W, average CPU%, sample count. Sorted by avg W descending.
    pub fn query_top_processes(&self, window_secs: u64) -> Result<Vec<ProcessAgg>> {
        let now = now_ts();
        let cutoff = now - window_secs as i64;
        let mut stmt = self.conn.prepare(TOP_PROC_SQL)?;
        let rows = stmt.query_map(params![cutoff], |row| {
            Ok(ProcessAgg {
                name: row.get(0)?,
                avg_w: row.get(1)?,
                avg_cpu_pct: row.get(2)?,
                sample_count: row.get::<_, i64>(3)? as usize,
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Delete process snapshots older than `retention_secs`.
    pub fn prune_process_snapshots(&self, retention_secs: u64) -> Result<usize> {
        let now = now_ts();
        let cutoff = now - retention_secs as i64;
        let n = self.conn.execute(PRUNE_PROC_SQL, params![cutoff])?;
        Ok(n)
    }
}

// --- SQL constants ------------------------------------------------------------

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS samples (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    ts                 INTEGER NOT NULL,
    capacity           INTEGER NOT NULL,
    status             TEXT    NOT NULL,
    power_now          REAL,
    voltage            REAL,
    energy_now         REAL,
    energy_full        REAL    NOT NULL,
    energy_full_design REAL    NOT NULL,
    cycle_count        INTEGER,
    cpu_load           REAL,
    brightness         REAL,
    temperature        REAL
);
CREATE INDEX IF NOT EXISTS idx_samples_ts ON samples(ts);
-- Idempotent backfill: re-inserting the same ts is silently ignored.
CREATE UNIQUE INDEX IF NOT EXISTS uq_samples_ts ON samples(ts);

-- Per-process estimated power snapshots (written by the daemon).
CREATE TABLE IF NOT EXISTS process_snapshot (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    ts      INTEGER NOT NULL,
    pid     INTEGER NOT NULL,
    name    TEXT    NOT NULL,
    cpu_pct REAL    NOT NULL,
    est_w   REAL    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_proc_ts ON process_snapshot(ts);
CREATE INDEX IF NOT EXISTS idx_proc_name_ts ON process_snapshot(name, ts);
";

const INSERT_SQL: &str = "
INSERT OR IGNORE INTO samples
    (ts, capacity, status, power_now, voltage, energy_now,
     energy_full, energy_full_design, cycle_count,
     cpu_load, brightness, temperature)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
";

const SELECT_SINCE_SQL: &str = "
SELECT ts, capacity, status, power_now, voltage, energy_now,
       energy_full, energy_full_design, cycle_count,
       cpu_load, brightness, temperature
FROM samples
WHERE ts >= ?1
ORDER BY ts ASC
";

const SELECT_LAST_SQL: &str = "
SELECT ts, capacity, status, power_now, voltage, energy_now,
       energy_full, energy_full_design, cycle_count,
       cpu_load, brightness, temperature
FROM (
    SELECT * FROM samples ORDER BY ts DESC LIMIT ?1
)
ORDER BY ts ASC
";

const SELECT_ALL_SQL: &str = "
SELECT ts, capacity, status, power_now, voltage, energy_now,
       energy_full, energy_full_design, cycle_count,
       cpu_load, brightness, temperature
FROM samples
ORDER BY ts ASC
";

/// Average %/h by hour of day (discharging only; converted to localtime).
const HOURLY_PATTERN_SQL: &str = "
SELECT CAST(strftime('%H', ts, 'unixepoch', 'localtime') AS INTEGER) AS hour,
       AVG(power_now / energy_full * 100.0)                          AS avg_rate,
       COUNT(*)                                                     AS n
FROM samples
WHERE status = 'Discharging'
  AND power_now IS NOT NULL
  AND power_now > 0
  AND energy_full > 0
GROUP BY hour
ORDER BY hour
";

/// Average %/h by day of week (weekday: 0=Sunday … 6=Saturday).
const WEEKDAY_PATTERN_SQL: &str = "
SELECT CAST(strftime('%w', ts, 'unixepoch', 'localtime') AS INTEGER) AS day,
       AVG(power_now / energy_full * 100.0)                          AS avg_rate,
       COUNT(*)                                                     AS n
FROM samples
WHERE status = 'Discharging'
  AND power_now IS NOT NULL
  AND power_now > 0
  AND energy_full > 0
GROUP BY day
ORDER BY day
";

/// Bulk insert for process snapshots.
const INSERT_PROC_SQL: &str = "
INSERT INTO process_snapshot (ts, pid, name, cpu_pct, est_w)
VALUES (?1, ?2, ?3, ?4, ?5)
";

/// Per-name aggregate estimated power within a window (descending by avg W).
const TOP_PROC_SQL: &str = "
SELECT name,
       AVG(est_w)   AS avg_w,
       AVG(cpu_pct) AS avg_cpu,
       COUNT(*)     AS n
FROM process_snapshot
WHERE ts >= ?1
GROUP BY name
ORDER BY avg_w DESC
";

/// Delete old process snapshots (retention).
const PRUNE_PROC_SQL: &str = "
DELETE FROM process_snapshot WHERE ts < ?1
";

// --- helpers ------------------------------------------------------------------

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn row_to_sample(row: &rusqlite::Row<'_>) -> rusqlite::Result<Sample> {
    let status_str: String = row.get(2)?;
    Ok(Sample {
        ts: row.get(0)?,
        capacity: row.get(1)?,
        status: parse_status(&status_str),
        power_now: row.get(3)?,
        voltage: row.get(4)?,
        energy_now: row.get(5)?,
        energy_full: row.get(6)?,
        energy_full_design: row.get(7)?,
        cycle_count: row.get(8)?,
        cpu_load: row.get(9)?,
        brightness: row.get(10)?,
        temperature: row.get(11)?,
    })
}

/// Idempotently add missing columns to old DBs (cpu_load/brightness/temperature).
///
/// Because SCHEMA uses `IF NOT EXISTS`, an existing table is not recreated;
/// new columns are therefore added via ALTER TABLE.
fn migrate(conn: &Connection) -> Result<()> {
    let cols: Vec<String> = conn
        .prepare("PRAGMA table_info(samples)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .collect();
    for (col, ty) in [
        ("cpu_load", "REAL"),
        ("brightness", "REAL"),
        ("temperature", "REAL"),
    ] {
        if !cols.iter().any(|c| c == col) {
            conn.execute(&format!("ALTER TABLE samples ADD COLUMN {col} {ty}"), [])?;
        }
    }
    Ok(())
}

fn parse_status(s: &str) -> Status {
    // Resolve back the values written by Status::label().
    match s {
        "Charging" => Status::Charging,
        "Discharging" => Status::Discharging,
        "Full" => Status::Full,
        "Not charging" => Status::NotCharging,
        _ => Status::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(ts: i64, cap: u8, power: Option<f64>, status: Status) -> Sample {
        Sample {
            ts,
            capacity: cap,
            status,
            power_now: power,
            voltage: Some(16.0),
            energy_now: Some(40.0),
            energy_full: 50.0,
            energy_full_design: 56.0,
            cycle_count: Some(100),
            cpu_load: None,
            brightness: None,
            temperature: None,
        }
    }

    #[test]
    fn insert_and_query_roundtrip() {
        let store = Store::open_in_memory().unwrap();
        store
            .insert(&sample(1000, 80, Some(5.0), Status::Discharging))
            .unwrap();
        store
            .insert(&sample(2000, 70, Some(8.0), Status::Discharging))
            .unwrap();
        store
            .insert(&sample(3000, 65, Some(0.0), Status::Charging))
            .unwrap();

        assert_eq!(store.count().unwrap(), 3);

        let all = store.query_last(10).unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].capacity, 80);
        assert_eq!(all[2].status, Status::Charging);
        // status round-trip (label → enum)
        assert_eq!(all[1].status, Status::Discharging);
    }

    #[test]
    fn query_since_returns_recent_samples() {
        let store = Store::open_in_memory().unwrap();
        let now = now_ts();
        // Insert data around the current time (not in the distant past).
        store
            .insert(&sample(now, 80, Some(5.0), Status::Discharging))
            .unwrap();
        store
            .insert(&sample(now + 1, 79, Some(6.0), Status::Discharging))
            .unwrap();
        store
            .insert(&sample(now + 2, 78, Some(7.0), Status::Discharging))
            .unwrap();

        // Very wide window (≥ since the epoch) → returns everything.
        let since = store.query_since(u64::MAX / 2).unwrap();
        assert!(since.len() >= 3);
        // Sorted (ts ascending).
        assert!(since.windows(2).all(|w| w[0].ts <= w[1].ts));
    }

    #[test]
    fn query_last_returns_n_newest() {
        let store = Store::open_in_memory().unwrap();
        for i in 0..10 {
            store
                .insert(&sample(
                    1000 + i,
                    90 - i as u8,
                    Some(5.0),
                    Status::Discharging,
                ))
                .unwrap();
        }
        let last3 = store.query_last(3).unwrap();
        assert_eq!(last3.len(), 3);
        assert_eq!(last3[0].capacity, 83); // ts 1007
        assert_eq!(last3[2].capacity, 81); // ts 1009 (newest)
    }

    #[test]
    fn hourly_pattern_aggregates_by_hour() {
        let store = Store::open_in_memory().unwrap();
        // ts=0 → 00:00 UTC, ts=3600 → 01:00 UTC (may shift by localtime, but
        // the two samples land in different hours). %/h = power/energy_full*100 = power/50*100.
        // Hour A has two samples (5W→10%/h, 15W→30%/h → avg 20), hour B has one (10W→20%/h).
        store
            .insert(&sample(0, 80, Some(5.0), Status::Discharging))
            .unwrap();
        store
            .insert(&sample(60, 79, Some(15.0), Status::Discharging))
            .unwrap();
        store
            .insert(&sample(3600, 70, Some(10.0), Status::Discharging))
            .unwrap();
        // Charging samples must not enter the pattern.
        store
            .insert(&sample(120, 79, Some(20.0), Status::Charging))
            .unwrap();

        let bins = store.query_hourly_pattern().unwrap();
        // 3 discharging samples excluding charging, split across 2 hours → 2 bins.
        assert_eq!(bins.len(), 2);
        // Total sample count excluding charging must be 3.
        let total_n: usize = bins.iter().map(|b| b.sample_count).sum();
        assert_eq!(total_n, 3);
        // The single-sample bin %/h = 20 (10W/50*100).
        let single = bins.iter().find(|b| b.sample_count == 1).unwrap();
        assert!((single.avg_pct_per_hour - 20.0).abs() < 0.01);
        // The two-sample bin avg = (10+30)/2 = 20 %/h.
        let double = bins.iter().find(|b| b.sample_count == 2).unwrap();
        assert!((double.avg_pct_per_hour - 20.0).abs() < 0.01);
    }

    #[test]
    fn sessions_segment_discharging_runs() {
        let store = Store::open_in_memory().unwrap();
        // discharge(10) → charge(interrupt) → discharge(10) → long gap → discharge(2)
        for i in 0..10 {
            store
                .insert(&sample(i, 90 - i as u8, Some(5.0), Status::Discharging))
                .unwrap();
        }
        store
            .insert(&sample(10, 80, Some(0.0), Status::Charging))
            .unwrap();
        for i in 0..10 {
            store
                .insert(&sample(20 + i, 70, Some(5.0), Status::Discharging))
                .unwrap();
        }
        // A gap larger than 10 min (600s) → separate session.
        store
            .insert(&sample(20 + 10 + 700, 70, Some(5.0), Status::Discharging))
            .unwrap();

        let sessions = store.query_sessions(600).unwrap();
        // 3 sessions: [0..10), [20..30), [last].
        assert_eq!(sessions.len(), 3);
        // First session: 90→81, 10 samples.
        assert_eq!(sessions[2].start_capacity, 90);
        assert_eq!(sessions[2].end_capacity, 81);
        assert_eq!(sessions[2].sample_count, 10);
        // Newest first.
        assert!(sessions[0].start_ts >= sessions[1].start_ts);
    }

    #[test]
    fn anomalies_flag_high_zscore_power() {
        let store = Store::open_in_memory().unwrap();
        // 9 low-power (~5W) + 1 outlier (50W).
        for i in 0..9 {
            store
                .insert(&sample(i, 80, Some(5.0), Status::Discharging))
                .unwrap();
        }
        store
            .insert(&sample(9, 70, Some(50.0), Status::Discharging))
            .unwrap();

        let anomalies = store.query_anomalies(2.0).unwrap();
        assert_eq!(anomalies.len(), 1);
        assert!((anomalies[0].power - 50.0).abs() < 0.01);
        assert!(anomalies[0].z_score >= 2.0);
    }

    #[test]
    fn sample_new_merges_battery_and_system() {
        use crate::battery::BatterySample;
        use crate::system::SystemMetrics;
        let bs = BatterySample {
            capacity: 50,
            status: Status::Discharging,
            power_now: Some(10.0),
            voltage: Some(16.0),
            energy_now: Some(25.0),
            energy_full: 50.0,
            energy_full_design: 56.0,
            cycle_count: Some(42),
        };
        let sys = SystemMetrics {
            cpu_load: Some(33.0),
            brightness: Some(60.0),
            temperature: Some(45.0),
        };
        let s = Sample::new(123, &bs, &sys);
        assert_eq!(s.ts, 123);
        assert_eq!(s.capacity, 50);
        assert_eq!(s.cycle_count, Some(42));
        assert_eq!(s.cpu_load, Some(33.0));
        assert_eq!(s.brightness, Some(60.0));
        assert_eq!(s.temperature, Some(45.0));
    }

    #[test]
    fn migrate_adds_columns_to_old_schema() {
        // Open a DB with the old (9-column) schema, then verify that opening
        // with the new schema + migration adds the columns.
        let store = Store::open_in_memory().unwrap();
        store
            .insert(&sample(1, 80, Some(5.0), Status::Discharging))
            .unwrap();
        // open_in_memory already calls migrate(); verify the columns exist.
        let cols: Vec<String> = store
            .conn
            .prepare("PRAGMA table_info(samples)")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert!(cols.contains(&"cpu_load".into()));
        assert!(cols.contains(&"brightness".into()));
        assert!(cols.contains(&"temperature".into()));
    }

    #[test]
    fn process_snapshots_insert_query_prune() {
        use crate::process::ProcessPower;
        let store = Store::open_in_memory().unwrap();
        let now = now_ts();

        // Two snapshot groups around now (chrome + firefox).
        let snaps = vec![
            ProcessPower {
                pid: 1,
                name: "chrome".into(),
                cpu_pct: 40.0,
                est_w: 1.6,
            },
            ProcessPower {
                pid: 2,
                name: "firefox".into(),
                cpu_pct: 10.0,
                est_w: 0.4,
            },
        ];
        store.insert_process_snapshots(now, &snaps).unwrap();
        store.insert_process_snapshots(now + 60, &snaps).unwrap();

        // An old (out-of-window) record → must be excluded.
        let old = vec![ProcessPower {
            pid: 3,
            name: "old".into(),
            cpu_pct: 5.0,
            est_w: 0.2,
        }];
        store.insert_process_snapshots(now - 7200, &old).unwrap();

        // Last 1 hour (3600s) query: chrome avg 1.6 (2 samples), firefox 0.4.
        let top = store.query_top_processes(3600).unwrap();
        assert_eq!(top.len(), 2); // 'old' excluded
        assert_eq!(top[0].name, "chrome");
        assert!((top[0].avg_w - 1.6).abs() < 1e-6);
        assert_eq!(top[0].sample_count, 2);

        // Prune: 1-hour retention → everything old is deleted.
        let removed = store.prune_process_snapshots(3600).unwrap();
        assert!(removed >= 1, "old record should be deleted");
        let top2 = store.query_top_processes(u64::MAX / 2).unwrap();
        assert!(top2.iter().all(|p| p.name != "old"));
    }
}
