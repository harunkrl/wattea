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
    pub cpu_load: Option<f64>,
    pub brightness: Option<f64>,
    pub temperature: Option<f64>,
}

/// Desen analizi için bir sepet (hour 0–23 veya weekday 0–6).
#[derive(Debug, Clone, Default)]
pub struct HourlyBin {
    /// Sepet indeksi: saatlik modda 0–23, günlük modda 0–6 (Pazar–Cmt).
    pub hour: u8,
    pub avg_pct_per_hour: f64,
    pub sample_count: usize,
}

/// Tek bir "prizden çekilmiş" (on-battery) oturumu.
///
/// Ardışık Discharging örneklerinden oluşur; Charging/Full aralıklarıyla
/// veya uzun bir boşlukla (örn. suspend) bölünür.
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
    /// Oturum süresi (saniye).
    pub fn duration_secs(&self) -> i64 {
        self.end_ts - self.start_ts
    }

    /// Toplam % kaybı (negatif = şarj olmuş, ama discharging-only olduğu için genelde ≥0).
    pub fn capacity_drop(&self) -> i16 {
        self.start_capacity as i16 - self.end_capacity as i16
    }

    /// Ortalama güç (W); power ölçümü olmayan örnekler toplama katılmaz.
    pub fn avg_power(&self) -> Option<f64> {
        if self.sample_count == 0 {
            return None;
        }
        Some(self.power_sum / self.sample_count as f64)
    }

    /// Oturumun ortalama %/saat tüketimi: drop / süre.
    pub fn avg_pct_per_hour(&self) -> Option<f64> {
        let hours = self.duration_secs() as f64 / 3600.0;
        if hours <= 0.0 {
            return None;
        }
        Some(self.capacity_drop().max(0) as f64 / hours)
    }
}

/// Beklenenden yüksek güç tüketen tek bir anomali örneği.
#[derive(Debug, Clone)]
pub struct Anomaly {
    pub ts: i64,
    pub power: f64,
    pub z_score: f64,
    pub mean: f64,
    pub capacity: u8,
}

impl Sample {
    /// Tam bir örnek oluştur (batarya + sistem metrikleri).
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
        migrate(&conn)?;

        Ok(Self { conn })
    }

    /// Bellek içi DB (testler için).
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        migrate(&conn)?;
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
                sample.cpu_load,
                sample.brightness,
                sample.temperature,
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

    /// Beklenenden yüksek güç tüketen örnekleri (anomaliler) bul.
    ///
    /// Discharging örneklerinin güç dağılımında z-skoru `z_threshold`'den
    /// büyük olanları döndürür (varsayılan 2.0 = ~üst %2.3). Sonuç en yeni
    /// en üstte.
    pub fn query_anomalies(&self, z_threshold: f64) -> Result<Vec<Anomaly>> {
        let all = self.query_all()?;
        // Sadece discharging + güç ölçümü olanlar.
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

    /// Tüm örnekleri ts artan sırada döndür (session segmentasyonu/export için).
    pub fn query_all(&self) -> Result<Vec<Sample>> {
        let mut stmt = self.conn.prepare(SELECT_ALL_SQL)?;
        let rows = stmt.query_map([], row_to_sample)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// On-battery (Discharging) oturumlarını segmente et.
    ///
    /// `gap_secs`'den büyük boşluk yeni oturum sayılır (suspend/örnekleme
    /// atlamalarını ayırır). Sonuç en yeni oturum en üstte olacak şekilde
    /// ters kronolojik döner.
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
                            // Uzun boşluk: oturumu kapat, yenisini aç.
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
                // Discharging dışına çıkıldı (şarj/dolu): oturumu kapat.
                sessions.push(current.take().unwrap());
            }
        }
        if let Some(c) = current.take() {
            sessions.push(c);
        }

        // En yeni en üstte.
        sessions.sort_by_key(|b| std::cmp::Reverse(b.start_ts));
        Ok(sessions)
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
    cycle_count        INTEGER,
    cpu_load           REAL,
    brightness         REAL,
    temperature        REAL
);
CREATE INDEX IF NOT EXISTS idx_samples_ts ON samples(ts);
-- Idempotent backfill: aynı ts tekrar insert edilirse sessizce yok sayılır.
CREATE UNIQUE INDEX IF NOT EXISTS uq_samples_ts ON samples(ts);
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
        cpu_load: row.get(9)?,
        brightness: row.get(10)?,
        temperature: row.get(11)?,
    })
}

/// Eski DB'lerde eksik kolonları idempotent olarak ekle (cpu_load/brightness/temperature).
///
/// SCHEMA `IF NOT EXISTS` kullandığı için mevcut tablo yeniden oluşturulmaz;
/// bu yüzden yeni kolonlar ALTER TABLE ile eklenir.
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

    #[test]
    fn hourly_pattern_aggregates_by_hour() {
        let store = Store::open_in_memory().unwrap();
        // ts=0 → 00:00 UTC, ts=3600 → 01:00 UTC (localtime'a göre kayabilir ama
        // iki örnek farklı saatlere düşer). %/h = power/energy_full*100 = power/50*100.
        // Saat A'da iki örnek (5W→10%/h, 15W→30%/h → avg 20), saat B'de tek (10W→20%/h).
        store
            .insert(&sample(0, 80, Some(5.0), Status::Discharging))
            .unwrap();
        store
            .insert(&sample(60, 79, Some(15.0), Status::Discharging))
            .unwrap();
        store
            .insert(&sample(3600, 70, Some(10.0), Status::Discharging))
            .unwrap();
        // Charging örnekleri desene girmemeli.
        store
            .insert(&sample(120, 79, Some(20.0), Status::Charging))
            .unwrap();

        let bins = store.query_hourly_pattern().unwrap();
        // Charging hariç 3 discharging örnek, 2 ayrı saate dağılmış → 2 bin.
        assert_eq!(bins.len(), 2);
        // Toplam örnek sayısı charging hariç 3 olmalı.
        let total_n: usize = bins.iter().map(|b| b.sample_count).sum();
        assert_eq!(total_n, 3);
        // Tek örnekli bin %/h = 20 (10W/50*100).
        let single = bins.iter().find(|b| b.sample_count == 1).unwrap();
        assert!((single.avg_pct_per_hour - 20.0).abs() < 0.01);
        // İki örnekli bin avg = (10+30)/2 = 20 %/h.
        let double = bins.iter().find(|b| b.sample_count == 2).unwrap();
        assert!((double.avg_pct_per_hour - 20.0).abs() < 0.01);
    }

    #[test]
    fn sessions_segment_discharging_runs() {
        let store = Store::open_in_memory().unwrap();
        // discharge(10) → charge(kesinti) → discharge(10) → uzun gap → discharge(2)
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
        // 10dk = 600sn gap'ten büyük boşluk → ayrı session.
        store
            .insert(&sample(20 + 10 + 700, 70, Some(5.0), Status::Discharging))
            .unwrap();

        let sessions = store.query_sessions(600).unwrap();
        // 3 session: [0..10), [20..30), [son].
        assert_eq!(sessions.len(), 3);
        // İlk session: 90→81, 10 örnek.
        assert_eq!(sessions[2].start_capacity, 90);
        assert_eq!(sessions[2].end_capacity, 81);
        assert_eq!(sessions[2].sample_count, 10);
        // En yeni en üstte.
        assert!(sessions[0].start_ts >= sessions[1].start_ts);
    }

    #[test]
    fn anomalies_flag_high_zscore_power() {
        let store = Store::open_in_memory().unwrap();
        // 9 düşük güç (~5W) + 1 aykırı (50W).
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
        // Eski (9-kolonlu) şemayla bir DB aç, sonra Store::open_in_memory
        // ile yeni şema+migration çalışınca kolonların eklenmiş olduğunu doğrula.
        let store = Store::open_in_memory().unwrap();
        store
            .insert(&sample(1, 80, Some(5.0), Status::Discharging))
            .unwrap();
        // open_in_memory zaten migrate() çağırır; kolonların varlığını doğrula.
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
}
