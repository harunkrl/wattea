//! SQLite zaman-serisi deposu.
//!
//! Daemon örnekleri yazar, TUI (ve gelecekteki analiz araçları) okur.
//! WAL modu sayesinde eşzamanlı okuma/yazma kilitlenmesiz çalışır.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use color_eyre::eyre::{Context, Result};
use rusqlite::{Connection, params};

use crate::battery::{BatterySample, Status};

/// Bir zaman-serisi örnek satırı (sorgu sonuçları için).
#[derive(Debug, Clone)]
pub struct Sample {
    /// Unix epoch saniyesi.
    pub ts: i64,
    pub capacity: u8,
    pub status: Status,
    pub power_now: Option<f64>,
    pub voltage: Option<f64>,
    pub energy_now: Option<f64>,
    pub energy_full: f64,
    pub energy_full_design: f64,
    pub cycle_count: Option<u32>,
}

/// Desen analizi için bir sepet (hour 0–23 veya weekday 0–6).
#[derive(Debug, Clone, Default)]
pub struct HourlyBin {
    /// Sepet indeksi: saatlik modda 0–23, günlük modda 0–6 (Pazar–Cmt).
    pub hour: u8,
    pub avg_pct_per_hour: f64,
    pub sample_count: usize,
}

impl From<(&SystemTime, &BatterySample)> for Sample {
    fn from((ts, s): (&SystemTime, &BatterySample)) -> Self {
        Self {
            ts: ts
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
            capacity: s.capacity,
            status: s.status,
            power_now: s.power_now,
            voltage: s.voltage,
            energy_now: s.energy_now,
            energy_full: s.energy_full,
            energy_full_design: s.energy_full_design,
            cycle_count: s.cycle_count,
        }
    }
}

/// SQLite deposu. Daemon ve TUI ayrı `Store` örneği açar.
pub struct Store {
    conn: Connection,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").finish_non_exhaustive()
    }
}

impl Store {
    /// DB'yi aç; yoksa `data_dir` altında oluştur ve şemayı kur.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .wrap_err_with(|| format!("veri dizini oluşturulamadı: {}", parent.display()))?;
        }
        let conn = Connection::open(path)
            .wrap_err_with(|| format!("SQLite açılamadı: {}", path.display()))?;

        // WAL: eşzamanlı okuma/yazma, çökme-güvenli.
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "busy_timeout", 5000)?;

        conn.execute_batch(SCHEMA)?;

        Ok(Self { conn })
    }

    /// Bellek içi DB (testler için).
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self { conn })
    }

    /// Tek bir örnek ekle.
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
            ],
        )?;
        Ok(())
    }

    /// Belirli bir andan itibaren tüm örnekleri getir (trend grafikleri için).
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

    /// En son N örneği getir (en yeni sonda).
    pub fn query_last(&self, n: u32) -> Result<Vec<Sample>> {
        let mut stmt = self.conn.prepare(SELECT_LAST_SQL)?;
        let rows = stmt.query_map(params![n as i64], row_to_sample)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Toplam örnek sayısı (sistem durumu/teşhis için).
    pub fn count(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM samples", [], |r| r.get(0))?)
    }

    /// Saat-bazına ortalama %/saat tüketim deseni (0–23).
    ///
    /// Yalnızca `Discharging` örneklerini alır (şarjdayken desen anlamsız).
    /// Her saat için o saatteki tüm günlerin ortalama tüketim hızıdır —
    /// "saat 14'te genelde %X/sa harcarım" deseni.
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

    /// Gün-bazına ortalama %/saat tüketim deseni (Pazar=0 … Cumartesi=6).
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
}

// --- SQL sabitleri ------------------------------------------------------------

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
    cycle_count        INTEGER
);
CREATE INDEX IF NOT EXISTS idx_samples_ts ON samples(ts);
-- Idempotent backfill: aynı ts tekrar insert edilirse sessizce yok sayılır.
CREATE UNIQUE INDEX IF NOT EXISTS uq_samples_ts ON samples(ts);
";

const INSERT_SQL: &str = "
INSERT OR IGNORE INTO samples
    (ts, capacity, status, power_now, voltage, energy_now,
     energy_full, energy_full_design, cycle_count)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
";

const SELECT_SINCE_SQL: &str = "
SELECT ts, capacity, status, power_now, voltage, energy_now,
       energy_full, energy_full_design, cycle_count
FROM samples
WHERE ts >= ?1
ORDER BY ts ASC
";

const SELECT_LAST_SQL: &str = "
SELECT ts, capacity, status, power_now, voltage, energy_now,
       energy_full, energy_full_design, cycle_count
FROM (
    SELECT * FROM samples ORDER BY ts DESC LIMIT ?1
)
ORDER BY ts ASC
";

/// Saat-bazına ortalama %/sa (yalnız boşalma; localtime dönüşümüyle).
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

/// Gün-bazına ortalama %/sa (weekday: 0=Pazar … 6=Cumartesi).
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

// --- yardımcılar --------------------------------------------------------------

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
    })
}

fn parse_status(s: &str) -> Status {
    // Status::label() ile yazılan değerleri geri çöz.
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
        // Şimdiki zaman civarına veri ekle (geçmiş/zamanın dışında değil).
        store
            .insert(&sample(now, 80, Some(5.0), Status::Discharging))
            .unwrap();
        store
            .insert(&sample(now + 1, 79, Some(6.0), Status::Discharging))
            .unwrap();
        store
            .insert(&sample(now + 2, 78, Some(7.0), Status::Discharging))
            .unwrap();

        // Çok geniş pencere (≥ epoch'tan beri) → tümünü getirir.
        let since = store.query_since(u64::MAX / 2).unwrap();
        assert!(since.len() >= 3);
        // Sıralı (ts artan).
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
        assert_eq!(last3[2].capacity, 81); // ts 1009 (en yeni)
    }
}
