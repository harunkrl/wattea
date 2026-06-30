//! Process başına **tahmini** güç tüketimi.
//!
//! Linux çekirdeği process başına watt vermez. Bu modül bir tahmin (attribution)
//! yapar: her process'in CPU zamanı payını (`/proc/[pid]/stat` → utime+stime
//! deltası) ölçer, toplam CPU paket gücünü (RAPL `energy_uj` deltası) bu paya
//! orantılar. RAPL okunamazsa yalnızca CPU% gösterilir (est_w = 0).
//!
//! Bu tahmindir — kesin watt değildir. UI'da "est." olarak etiketlenir.

use std::collections::HashMap;
use std::fs;
use std::time::Instant;

/// Çoğu Linux'ta CLK_TCK = 100 (sysconf(_SC_CLK_TCK)).
const CLK_TCK: f64 = 100.0;

/// Tek process'in tahmini güç katkısı.
#[derive(Debug, Clone)]
pub struct ProcessPower {
    pub pid: u32,
    pub name: String,
    /// Toplam CPU kapasitesinin yüzdesi (çok çekirdekli > 100 olabilir).
    pub cpu_pct: f64,
    /// Tahmini güç katkısı (W). RAPL yoksa 0.
    pub est_w: f64,
}

/// Bir process okuması (iç state).
#[derive(Debug)]
struct ProcEntry {
    comm: String,
    ticks: u64, // utime + stime (clock ticks)
}

/// Stateful process okuyucu. İki örnek arasındaki delta ile CPU%/W hesaplar.
#[derive(Debug, Default)]
pub struct ProcessReader {
    prev: Option<(Instant, HashMap<u32, ProcEntry>, Option<u64>)>,
}


impl ProcessReader {
    pub fn new() -> Self {
        Self::default()
    }

    /// Örnekle ve en yüksek tahmini güçlü `n` process'i döndür.
    /// İlk örnekte delta yoktur → boş vektör.
    pub fn top(&mut self, n: usize) -> Vec<ProcessPower> {
        let now = Instant::now();
        let cur = read_all_procs();
        let rapl = read_rapl_energy();

        let result = if let Some((ptime, pmap, prapl)) = self.prev.take() {
            let dt = now.duration_since(ptime).as_secs_f64().max(1e-6);
            // RAPL deltasından toplam CPU gücü (W).
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
                    // CPU zamanının toplam CPU kapasitesine oranı.
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
            // Önce tahmini güce, eşitse CPU%'ye göre sırala (yüksek→düşük).
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

/// Toplam CPU çekirdek sayısı (fallback 1).
fn available_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// `/proc` altındaki tüm numeric PID'leri okur → pid, comm, utime+stime.
fn read_all_procs() -> HashMap<u32, ProcEntry> {
    let mut map = HashMap::new();
    if let Ok(entries) = fs::read_dir("/proc") {
        for e in entries.flatten() {
            if let Some(name) = e.file_name().to_str()
                && let Ok(pid) = name.parse::<u32>()
                    && let Ok(stat) = fs::read_to_string(e.path().join("stat"))
                        && let Some((_, comm, ticks)) = parse_proc_stat(&stat) {
                            map.insert(pid, ProcEntry { comm, ticks });
                        }
        }
    }
    map
}

/// `/proc/[pid]/stat` satırını ayrıştırır → (pid, comm, utime+stime ticks).
///
/// `comm` parantez içinde olabilir, boşluk/parantez içerebilir. Bu yüzden
/// ilk `(` ile son `)` arası comm olarak alınır.
pub fn parse_proc_stat(line: &str) -> Option<(u32, String, u64)> {
    let lparen = line.find('(')?;
    let rparen = line.rfind(')')?;
    if rparen <= lparen {
        return None;
    }
    let pid: u32 = line[..lparen].trim().parse().ok()?;
    let comm = line[lparen + 1..rparen].to_string();
    // Parantez sonrası: state ppid ... utime stime ...
    // utime = alan 14, stime = alan 15 (1-tabanlı). Parantez sonrası alan 3 = state
    // → utime index 14-3 = 11, stime index 12.
    let rest: Vec<&str> = line[rparen + 1..].split_whitespace().collect();
    let utime: u64 = rest.get(11)?.parse().ok()?;
    let stime: u64 = rest.get(12)?.parse().ok()?;
    Some((pid, comm, utime + stime))
}

/// RAPL toplam CPU paket enerjisini okur (microjoule).
///
/// `/sys/class/powercap/intel-rapl-0/energy_uj` (Intel). AMD/okunamazsa None.
/// Sadece "package0" düğümünü alır (name "package-X" arar).
pub fn read_rapl_energy() -> Option<u64> {
    let base = std::path::Path::new("/sys/class/powercap");
    let entries = fs::read_dir(base).ok()?;
    for e in entries.flatten() {
        let dir = e.path();
        // Alt düğüm mü yoksa paket mi kontrol et (package_X name).
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
        // Parantez sonrası: state(0) ppid(1) pgrp(2) session(3) tty_nr(4) tpgid(5)
        // flags(6) minflt(7) cminflt(8) majflt(9) cmajflt(10) utime(11) stime(12).
        // → state + 10 yer tutucu, sonra utime stime.
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
        // comm içinde parantez varsa: ilk ( ... son ) arası doğru alınmalı.
        // rest: state + 10 yer tutucu + utime(11) + stime(12).
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
        assert_eq!(parse_proc_stat("abc (x) R"), None); // pid parse edilemez
    }

    #[test]
    fn process_reader_first_sample_empty() {
        // İlk örnekte /proc gerçekten okunur ama delta yok → boş ( veya /proc yoksa da boş).
        let mut r = ProcessReader::new();
        let out = r.top(5);
        assert!(out.is_empty(), "ilk örnekte delta olmadığından boş olmalı");
    }
}
