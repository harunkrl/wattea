//! UPower history (`.dat`) backfill import.
//!
//! UPower keeps a chronological TSV log in
//! `/var/lib/upower/history-{charge,rate,voltage}-<battery>.dat`
//! (`ts \t value \t state`). This module parses those files, joins rate/voltage
//! onto the charge axis, and writes them into the Wattea SQLite store — so the
//! dashboard is populated with days of data even on first launch.
//!
//! Idempotent: the unique index on `samples(ts)` plus `INSERT OR IGNORE` means
//! re-running never creates duplicate rows.

use std::path::Path;

use color_eyre::eyre::{Context, Result};

use crate::battery::Status;
use crate::storage::{Sample, Store};

/// UPower history directory.
const UPOWER_DIR: &str = "/var/lib/upower";

/// Returns the number of imported samples.
///
/// `energy_full` / `energy_full_design` are taken from the current live sysfs;
/// since the past full capacity is not exactly known, the (current) fixed value
/// is used — a rough approximation for the health trend, but sufficient for
/// %/hour and trend charts.
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

    // Must be sorted by ts for binary search (usually already sorted).
    rate.sort_by_key(|r| r.0);
    voltage.sort_by_key(|r| r.0);

    let mut count = 0usize;
    let mut ri = 0usize; // rate cursor
    let mut vi = 0usize; // voltage cursor

    for (ts, cap, state) in &charge {
        // Noise: skip the unknown + 0.000 rows written at boot/suspend.
        if state == "unknown" || *cap <= 0.0 {
            continue;
        }
        let status = parse_state(state);

        // "Last known" join: take the closest rate/voltage value whose ts is
        // not greater than the charge ts. Advance the cursors (files are chronological).
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
            cpu_load: None,
            brightness: None,
            temperature: None,
        };
        store.insert(&sample)?;
        count += 1;
    }

    Ok(count)
}

/// Pick the filename suffix (e.g. `L21M4PD0-56-1044`).
///
/// If a model is given, pick the file whose name contains it; otherwise take
/// the largest charge file (the primary battery usually produces the most data;
/// noise/secondary devices like generic_id/XZ10 are smaller).
fn pick_suffix(model: Option<&str>) -> Result<String> {
    let entries =
        std::fs::read_dir(UPOWER_DIR).wrap_err_with(|| format!("cannot read {UPOWER_DIR}"))?;

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
                "UPower history not found: no history-charge-*.dat under {UPOWER_DIR}"
            )
        })
}

/// Parse `ts \t value \t state` lines. Malformed lines are skipped.
fn parse_dat(path: &Path) -> Result<Vec<(i64, f64, String)>> {
    let content =
        std::fs::read_to_string(path).wrap_err_with(|| format!("read: {}", path.display()))?;
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

/// Advance the `cursor` up to the last record whose ts is not greater than `target_ts`.
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

    /// Create a temporary directory for tests (without a tempfile crate dependency).
    fn tempdir_for(prefix: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

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
        assert_eq!(data[c].0, 200); // largest ≤ 250 is 200
        advance(&data, &mut c, 300);
        assert_eq!(data[c].0, 300);
        // When the target is in the past the cursor does not go back (chronological assumption).
        advance(&data, &mut c, 150);
        assert_eq!(data[c].0, 300);
    }

    #[test]
    fn parse_dat_handles_realistic_upower_format() {
        // Realistic /var/lib/upower/history-*.dat format: ts\tvalue\tstate.
        let dir = tempdir_for("wattea_parse_dat");
        let path = dir.join("history-charge-test.dat");
        std::fs::write(
            &path,
            "1781939602\t79.000\tdischarging\n1781939700\t78.000\tdischarging\n1781939800\t0.000\tunknown\n",
        )
        .unwrap();
        let rows = parse_dat(&path).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].0, 1781939602);
        assert!((rows[0].1 - 79.0).abs() < 1e-9);
        assert_eq!(rows[0].2, "discharging");
        // 0.000 + unknown is also parsed (filtering happens on the import side).
        assert_eq!(rows[2].2, "unknown");
    }

    #[test]
    fn parse_dat_skips_malformed_lines() {
        let dir = tempdir_for("wattea_parse_bad");
        let path = dir.join("bad.dat");
        std::fs::write(
            &path,
            "100\t50.0\tdischarging\ngarbage line\n200\tnotanumber\tx\n\n",
        )
        .unwrap();
        let rows = parse_dat(&path).unwrap();
        // Only the first valid line remains.
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, 100);
    }
}
