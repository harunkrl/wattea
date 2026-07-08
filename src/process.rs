//! Per-process **estimated** power consumption.
//!
//! The Linux kernel does not provide per-process watts. This module produces
//! an estimate (attribution): it measures each process's share of CPU time
//! (from `/proc/[pid]/stat` → utime+stime delta) and apportions the total CPU
//! package power (RAPL `energy_uj` delta) by that share. When RAPL cannot be
//! read, only CPU% is shown (est_w = 0).
//!
//! This is an estimate — not an exact watt value. It is labeled "est." in the UI.

use std::collections::HashMap;
use std::fs;
use std::time::Instant;

/// On most Linux systems CLK_TCK = 100 (sysconf(_SC_CLK_TCK)).
const CLK_TCK: f64 = 100.0;

/// A single process's estimated power contribution.
#[derive(Debug, Clone)]
pub struct ProcessPower {
    pub pid: u32,
    pub name: String,
    /// Percentage of total CPU capacity (may exceed 100% on multicore).
    pub cpu_pct: f64,
    /// Estimated power contribution (W). 0 when RAPL is unavailable.
    pub est_w: f64,
}

/// A single process reading (internal state).
#[derive(Debug)]
struct ProcEntry {
    comm: String,
    ticks: u64, // utime + stime (clock ticks)
}

/// Stateful process reader. Computes CPU%/W from the delta between two samples.
#[derive(Debug, Default)]
pub struct ProcessReader {
    prev: Option<(Instant, HashMap<u32, ProcEntry>, Option<u64>)>,
}

impl ProcessReader {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sample and return the top `n` processes by estimated power.
    /// On the first sample there is no delta → empty vector.
    pub fn top(&mut self, n: usize) -> Vec<ProcessPower> {
        let now = Instant::now();
        let cur = read_all_procs();
        let rapl = read_rapl_energy();

        let result = if let Some((ptime, pmap, prapl)) = self.prev.take() {
            let dt = now.duration_since(ptime).as_secs_f64().max(1e-6);
            // Total CPU power (W) from the RAPL delta.
            let total_w = match (rapl, prapl) {
                (Some(c), Some(p)) => Some((c.saturating_sub(p)) as f64 / 1e6 / dt),
                _ => None,
            };
            let cpus = available_cpus() as f64;

            let mut out: Vec<ProcessPower> = cur
                .iter()
                .filter_map(|(pid, e)| {
                    let p = pmap.get(pid)?;
                    let dticks = e.ticks.saturating_sub(p.ticks) as f64;
                    // Ratio of CPU time to total CPU capacity.
                    let cpu_frac = dticks / (dt * CLK_TCK * cpus);
                    let cpu_pct = cpu_frac * 100.0;
                    let est_w = total_w.map(|w| cpu_frac * w).unwrap_or(0.0);
                    Some(ProcessPower {
                        pid: *pid,
                        name: e.comm.clone(),
                        cpu_pct,
                        est_w,
                    })
                })
                .collect();
            // Sort by estimated power first, then by CPU% on ties (high → low).
            out.sort_by(|a, b| {
                b.est_w
                    .partial_cmp(&a.est_w)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| {
                        b.cpu_pct
                            .partial_cmp(&a.cpu_pct)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
            });
            out.truncate(n);
            out
        } else {
            Vec::new()
        };
        self.prev = Some((now, cur, rapl));
        result
    }
}

/// Total CPU core count (fallback 1).
fn available_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// Read all numeric PIDs under `/proc` → pid, comm, utime+stime.
fn read_all_procs() -> HashMap<u32, ProcEntry> {
    let mut map = HashMap::new();
    if let Ok(entries) = fs::read_dir("/proc") {
        for e in entries.flatten() {
            if let Some(name) = e.file_name().to_str()
                && let Ok(pid) = name.parse::<u32>()
                && let Ok(stat) = fs::read_to_string(e.path().join("stat"))
                && let Some((_, comm, ticks)) = parse_proc_stat(&stat)
            {
                map.insert(pid, ProcEntry { comm, ticks });
            }
        }
    }
    map
}

/// Parse a `/proc/[pid]/stat` line → (pid, comm, utime+stime ticks).
///
/// `comm` is wrapped in parentheses and may contain spaces/parentheses. We
/// therefore take everything between the first `(` and the last `)` as comm.
pub fn parse_proc_stat(line: &str) -> Option<(u32, String, u64)> {
    let lparen = line.find('(')?;
    let rparen = line.rfind(')')?;
    if rparen <= lparen {
        return None;
    }
    let pid: u32 = line[..lparen].trim().parse().ok()?;
    let comm = line[lparen + 1..rparen].to_string();
    // After the parens: state ppid ... utime stime ...
    // utime = field 14, stime = field 15 (1-based). After the parens, field 3 = state
    // → utime index 14-3 = 11, stime index 12.
    let rest: Vec<&str> = line[rparen + 1..].split_whitespace().collect();
    let utime: u64 = rest.get(11)?.parse().ok()?;
    let stime: u64 = rest.get(12)?.parse().ok()?;
    Some((pid, comm, utime + stime))
}

/// Read the total CPU package energy from RAPL (microjoules).
///
/// `/sys/class/powercap/intel-rapl-0/energy_uj` (Intel). Returns None on
/// AMD/unavailable systems. Only the "package0" node is used (looks for a
/// name starting with "package").
pub fn read_rapl_energy() -> Option<u64> {
    let base = std::path::Path::new("/sys/class/powercap");
    let entries = fs::read_dir(base).ok()?;
    for e in entries.flatten() {
        let dir = e.path();
        // Check whether this is a sub-node or the package (package_X name).
        let name = fs::read_to_string(dir.join("name")).ok()?;
        let name = name.trim();
        if name.starts_with("package") {
            let energy = fs::read_to_string(dir.join("energy_uj")).ok()?;
            if let Ok(uj) = energy.trim().parse::<u64>() {
                return Some(uj);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_comm() {
        // After the parens: state(0) ppid(1) pgrp(2) session(3) tty_nr(4) tpgid(5)
        // flags(6) minflt(7) cminflt(8) majflt(9) cmajflt(10) utime(11) stime(12).
        // → state + 10 placeholders, then utime stime.
        let mut rest = String::from("R ");
        for _ in 0..10 {
            rest.push_str("0 ");
        }
        rest.push_str("500 600 "); // idx 11=utime, 12=stime
        let line = format!("1234 (chrome) {rest}0 0 0");
        let (pid, comm, ticks) = parse_proc_stat(&line).unwrap();
        assert_eq!(pid, 1234);
        assert_eq!(comm, "chrome");
        assert_eq!(ticks, 1100);
    }

    #[test]
    fn parse_comm_with_spaces() {
        let mut rest = String::from("R ");
        for _ in 0..10 {
            rest.push_str("0 ");
        }
        rest.push_str("100 200 ");
        let line = format!("5678 (my cool app) {rest}0");
        let (pid, comm, ticks) = parse_proc_stat(&line).unwrap();
        assert_eq!(pid, 5678);
        assert_eq!(comm, "my cool app");
        assert_eq!(ticks, 300);
    }

    #[test]
    fn parse_comm_with_parens() {
        // When comm contains parens, everything between the first ( and last )
        // must be taken correctly.
        // rest: state + 10 placeholders + utime(11) + stime(12).
        let line = "9 (foo (bar) baz) S 0 0 0 0 0 0 0 0 0 0 7 8 0".to_string();
        // rest = "S 0 0 0 0 0 0 0 0 0 0 7 8 0"; idx0=S, idx11=7, idx12=8
        let (pid, comm, ticks) = parse_proc_stat(&line).unwrap();
        assert_eq!(pid, 9);
        assert_eq!(comm, "foo (bar) baz");
        assert_eq!(ticks, 15);
    }

    #[test]
    fn parse_malformed_returns_none() {
        assert_eq!(parse_proc_stat("no parens here"), None);
        assert_eq!(parse_proc_stat("(no pid) R"), None);
        assert_eq!(parse_proc_stat("abc (x) R"), None); // pid cannot be parsed
    }

    #[test]
    fn process_reader_first_sample_empty() {
        // On the first sample /proc is actually read but there is no delta → empty
        // (also empty when /proc is unavailable).
        let mut r = ProcessReader::new();
        let out = r.top(5);
        assert!(out.is_empty(), "first sample must be empty (no delta yet)");
    }
}
