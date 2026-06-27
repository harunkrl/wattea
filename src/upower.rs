//! UPower history (`.dat`) backfill import.
//!
//! UPower, `/var/lib/upower/history-{charge,rate,voltage}-<baterya>.dat`
//! dosyalarına kronolojik TSV kaydı tutar (`ts \t değer \t durum`). Bu modül
//! o dosyaları parse eder, charge ekseni üzerinden rate/voltage ile eşleştirir
//! ve Wattea SQLite deposuna yazar — böylece ilk açılışta bile günlerce veriyle
//! dolu bir dashboard mümkün olur.
//!
//! Idempotent'tir: `samples(ts)` unique index'i + `INSERT OR IGNORE` sayesinde
//! tekrar çalıştırınca çift kayıt oluşmaz.

use std::path::Path;

use color_eyre::eyre::{Context, Result};

use crate::battery::Status;
use crate::storage::{Sample, Store};

/// UPower history dizini.
const UPOWER_DIR: &str = "/var/lib/upower";

/// Import edilen örnek sayısını döndürür.
///
/// `energy_full` / `energy_full_design` anlık sysfs'ten alınır; geçmişte bu
/// tam bilinemediği için sabit (bugünkü) değer kullanılır — health trendi için
/// kaba bir yaklaşımdır, ama %/saat ve trend grafikleri için yeterli.
pub fn import(
    store: &Store,
    model: Option<&str>,
    energy_full: f64,
    energy_full_design: f64,
) -> Result<usize> {
    let suffix = pick_suffix(model)?;
    let dir = Path::new(UPOWER_DIR);

    let charge = parse_dat(&dir.join(format!("history-charge-{suffix}.dat")))?;
    let mut rate = parse_dat(&dir.join(format!("history-rate-{suffix}.dat")))?;
    let mut voltage = parse_dat(&dir.join(format!("history-voltage-{suffix}.dat")))?;

    // Binary-search için ts'ye göre sıralı olmalı (genelde zaten sıralıdır).
    rate.sort_by_key(|r| r.0);
    voltage.sort_by_key(|r| r.0);

    let mut count = 0usize;
    let mut ri = 0usize; // rate imlecisi
    let mut vi = 0usize; // voltage imlecisi

    for (ts, cap, state) in &charge {
        // Gürültü: boot/suspend'te yazılan unknown + 0.000 satırlarını atla.
        if state == "unknown" || *cap <= 0.0 {
            continue;
        }
        let status = parse_state(state);

        // "Last known" eşleştirme: charge ts'sinden büyük olmayan en yakın
        // rate/voltage değerini al. İmleçleri ilerlet (dosyalar kronolojik).
        advance(&rate, &mut ri, *ts);
        advance(&voltage, &mut vi, *ts);

        let power = rate
            .get(ri)
            .filter(|(rt, _, _)| rt <= ts)
            .and_then(|(_, v, _)| (*v > 0.0).then_some(*v));
        let volt = voltage
            .get(vi)
            .filter(|(vt, _, _)| vt <= ts)
            .map(|(_, v, _)| *v);
        let energy_now = cap / 100.0 * energy_full;

        let sample = Sample {
            ts: *ts,
            capacity: cap.round().clamp(0.0, 100.0) as u8,
            status,
            power_now: power,
            voltage: volt,
            energy_now: Some(energy_now),
            energy_full,
            energy_full_design,
            cycle_count: None,
        };
        store.insert(&sample)?;
        count += 1;
    }

    Ok(count)
}

/// Dosya adı suffix'ini seç (örn `L21M4PD0-56-1044`).
///
/// Model verildiyse adında geçen dosyayı seçer; yoksa en büyük charge dosyasını
/// alır (genelde asıl batarya en çok veri üretir; generic_id/XZ10 gibi
/// gürültü/sekonder cihazlar küçüktür).
fn pick_suffix(model: Option<&str>) -> Result<String> {
    let entries =
        std::fs::read_dir(UPOWER_DIR).wrap_err_with(|| format!("{UPOWER_DIR} okunamadı"))?;

    let mut candidates: Vec<(u64, String)> = Vec::new();
    for entry in entries {
        let name = entry?.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix("history-charge-") else {
            continue;
        };
        let Some(suffix) = rest.strip_suffix(".dat") else {
            continue;
        };
        let size = std::fs::metadata(Path::new(UPOWER_DIR).join(&name))
            .map(|m| m.len())
            .unwrap_or(0);
        if let Some(m) = model
            && suffix.contains(m)
        {
            return Ok(suffix.to_string());
        }
        candidates.push((size, suffix.to_string()));
    }

    candidates.sort_by_key(|(s, _)| *s);
    candidates
        .into_iter()
        .next_back()
        .map(|(_, s)| s)
        .ok_or_else(|| {
            color_eyre::eyre::eyre!(
                "UPower history bulunamadı: {UPOWER_DIR} altında history-charge-*.dat yok"
            )
        })
}

/// `ts \t değer \t durum` satırlarını parse et. Bozuk satırlar atlanır.
fn parse_dat(path: &Path) -> Result<Vec<(i64, f64, String)>> {
    let content =
        std::fs::read_to_string(path).wrap_err_with(|| format!("oku: {}", path.display()))?;
    let mut out = Vec::new();
    for line in content.lines() {
        let mut it = line.split('\t');
        let (Some(ts_s), Some(val_s), Some(state)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let (Ok(ts), Ok(val)) = (ts_s.trim().parse::<i64>(), val_s.trim().parse::<f64>()) else {
            continue;
        };
        out.push((ts, val, state.trim().to_string()));
    }
    Ok(out)
}

/// `imleci`, `target_ts`'den büyük olmayan son kayda kadar ilerlet.
fn advance(data: &[(i64, f64, String)], cursor: &mut usize, target_ts: i64) {
    while *cursor + 1 < data.len() && data[*cursor + 1].0 <= target_ts {
        *cursor += 1;
    }
}

fn parse_state(state: &str) -> Status {
    match state {
        "charging" => Status::Charging,
        "discharging" => Status::Discharging,
        "pending-charge" | "fully-charged" => Status::Full,
        _ => Status::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_parsing_covers_upower_labels() {
        assert_eq!(parse_state("charging"), Status::Charging);
        assert_eq!(parse_state("discharging"), Status::Discharging);
        assert_eq!(parse_state("pending-charge"), Status::Full);
        assert_eq!(parse_state("fully-charged"), Status::Full);
        assert_eq!(parse_state("unknown"), Status::Unknown);
        assert_eq!(parse_state("garbage"), Status::Unknown);
    }

    #[test]
    fn advance_finds_last_known_before_target() {
        let data = vec![
            (100, 1.0, "x".into()),
            (200, 2.0, "x".into()),
            (300, 3.0, "x".into()),
        ];
        let mut c = 0;
        advance(&data, &mut c, 250);
        assert_eq!(data[c].0, 200); // 250'den küçük en büyük = 200
        advance(&data, &mut c, 300);
        assert_eq!(data[c].0, 300);
        // target geçmişte ise imleç geri dönmez (kronolojik varsayım).
        advance(&data, &mut c, 150);
        assert_eq!(data[c].0, 300);
    }
}
