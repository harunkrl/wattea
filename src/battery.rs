//! sysfs battery reading.
//!
//! Reads all metrics from the files under `/sys/class/power_supply/<BAT*>/`.
//! Files may be missing, so every field is `Option` and the `read_*` helpers
//! silently return `None` on failure.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Charging state read from sysfs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Charging,
    Discharging,
    Full,
    NotCharging,
    Unknown,
}

impl Status {
    fn from_str(s: &str) -> Self {
        match s.trim() {
            "Charging" => Self::Charging,
            "Discharging" => Self::Discharging,
            "Full" => Self::Full,
            "Not charging" => Self::NotCharging,
            _ => Self::Unknown,
        }
    }

    /// Single-glyph badge (for header/badge display).
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Charging => "⚡",
            Self::Discharging => "🔋",
            Self::Full => "🔌",
            Self::NotCharging => "⏸",
            Self::Unknown => "❓",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Charging => "Charging",
            Self::Discharging => "Discharging",
            Self::Full => "Full",
            Self::NotCharging => "Not charging",
            Self::Unknown => "Unknown",
        }
    }
}

/// Static (unchanging) battery identity — read once at startup.
#[derive(Debug, Clone)]
pub struct BatteryInfo {
    pub name: String,
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub technology: Option<String>,
}

/// A single sample (the result of a sysfs read).
#[derive(Debug, Clone)]
pub struct BatterySample {
    pub capacity: u8, // % (0–100)
    pub status: Status,
    pub power_now: Option<f64>, // W (instantaneous power; positive whether charging or discharging)
    pub voltage: Option<f64>,   // V
    pub energy_now: Option<f64>, // Wh (remaining)
    pub energy_full: f64,       // Wh (current full capacity)
    pub energy_full_design: f64, // Wh (factory design capacity)
    pub cycle_count: Option<u32>, // cycle count
}

impl BatterySample {
    /// Percent flow per hour: power(W) / energy_full(Wh) * 100.
    pub fn pct_per_hour(&self) -> Option<f64> {
        let p = self.power_now?;
        if p <= 0.0 || self.energy_full <= 0.0 {
            return None;
        }
        Some(p / self.energy_full * 100.0)
    }

    /// Estimated time remaining. Discharging → time to empty; charging → time to full.
    pub fn time_remaining(&self) -> Option<Duration> {
        let p = self.power_now?;
        if p <= 0.0 {
            return None;
        }
        let hours = match self.status {
            Status::Discharging => self.energy_now? / p,
            Status::Charging => ((self.energy_full - self.energy_now?).max(0.0)) / p,
            _ => return None,
        };
        Some(Duration::from_secs_f64(hours * 3600.0))
    }

    /// Health: current full capacity / design capacity.
    pub fn health(&self) -> f64 {
        if self.energy_full_design <= 0.0 {
            return 0.0;
        }
        self.energy_full / self.energy_full_design * 100.0
    }
}

/// sysfs battery resource handle. Auto-detects the first `Battery` type.
pub struct Battery {
    pub(crate) dir: PathBuf,
    pub info: BatteryInfo,
}

impl Battery {
    /// Find the first battery under `/sys/class/power_supply/`.
    pub fn detect() -> color_eyre::Result<Self> {
        let base = Path::new("/sys/class/power_supply");
        for entry in std::fs::read_dir(base)? {
            let entry = entry?;
            let path = entry.path();
            if read_str(&path.join("type")).trim() == "Battery" {
                let info = BatteryInfo {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    manufacturer: read_str_opt(&path.join("manufacturer")),
                    model: read_str_opt(&path.join("model_name")),
                    technology: read_str_opt(&path.join("technology")),
                };
                return Ok(Self { dir: path, info });
            }
        }
        color_eyre::eyre::bail!("no battery found under /sys/class/power_supply/")
    }

    /// Read all live metrics once.
    pub fn read(&self) -> color_eyre::Result<BatterySample> {
        let capacity = read_u(&self.dir.join("capacity"), 1000).min(100) as u8;
        let status = read_str(&self.dir.join("status"));
        let status = Status::from_str(&status);

        // Power: prefer power_now (µW); fall back to current_now (µA) * voltage (V).
        let power_now = read_micro(&self.dir.join("power_now")).or_else(|| {
            let current = read_micro(&self.dir.join("current_now"))?;
            let voltage = read_micro(&self.dir.join("voltage_now"))?;
            Some(current * voltage / 1_000_000.0) // (µA/1e6)*(V) = A*V = W  ->  µA*V/1e6
        });

        let voltage = read_micro(&self.dir.join("voltage_now"));

        // Energy: prefer energy_* (µWh); fall back to charge_* (µAh) * V for Wh.
        let energy_now = read_micro(&self.dir.join("energy_now")).or_else(|| {
            let charge = read_micro(&self.dir.join("charge_now"))?;
            Some(charge * voltage.unwrap_or(0.0) / 1_000_000.0)
        });
        let energy_full = read_micro(&self.dir.join("energy_full"))
            .or_else(|| {
                let charge = read_micro(&self.dir.join("charge_full"))?;
                Some(charge * voltage.unwrap_or(0.0) / 1_000_000.0)
            })
            .unwrap_or(0.0);
        let energy_full_design = read_micro(&self.dir.join("energy_full_design"))
            .or_else(|| {
                let charge = read_micro(&self.dir.join("charge_full_design"))?;
                Some(charge * voltage.unwrap_or(0.0) / 1_000_000.0)
            })
            .unwrap_or(0.0);

        let cycle_count = read_u_opt(&self.dir.join("cycle_count"));

        Ok(BatterySample {
            capacity,
            status,
            power_now,
            voltage,
            energy_now,
            energy_full,
            energy_full_design,
            cycle_count,
        })
    }
}

// --- file reader helpers ------------------------------------------------------

fn read_str(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn read_str_opt(path: &Path) -> Option<String> {
    let s = std::fs::read_to_string(path).ok()?;
    let s = s.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

/// Convert a micro-unit file (µW, µV, µWh, µA) to its real unit (÷1e6).
fn read_micro(path: &Path) -> Option<f64> {
    let raw = std::fs::read_to_string(path).ok()?.trim().to_string();
    let v: f64 = raw.parse().ok()?;
    Some(v / 1_000_000.0)
}

fn read_u(path: &Path, default: u64) -> u64 {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(default)
}

fn read_u_opt(path: &Path) -> Option<u32> {
    let v: u64 = std::fs::read_to_string(path).ok()?.trim().parse().ok()?;
    Some(v as u32)
}
