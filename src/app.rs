//! Application state — Elm Architecture (Model → Message → Update).

use std::collections::VecDeque;
use std::time::Instant;

use crate::battery::{Battery, BatteryInfo, BatterySample};
use crate::process::{ProcessPower, ProcessReader};
use crate::storage::{Anomaly, HourlyBin, ProcessAgg, Sample, Session, Store};
use crate::system::{SystemMetrics, SystemReader};

/// Number of live samples kept for the sparkline (~5 min at 1s sampling).
const HISTORY_LEN: usize = 300;
/// Window shown on the Trend tab: the last 24 hours.
pub const TREND_WINDOW_SECS: u64 = 24 * 3600;

/// Pattern tab axis: hour of day or day of week.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatternAxis {
    Hourly,
    Weekday,
}

impl PatternAxis {
    pub fn toggle(self) -> Self {
        match self {
            Self::Hourly => Self::Weekday,
            Self::Weekday => Self::Hourly,
        }
    }
}

/// Process tab view: live or last-hour aggregate (history).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessView {
    Live,
    LastHour,
}

/// Active view tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// Live sysfs panel.
    Live,
    /// 24-hour history trend from SQLite.
    Trend,
    /// Usage pattern by hour-of-day / day-of-week.
    Pattern,
    /// On-battery sessions (unplug → plug cycles).
    Sessions,
    /// Per-process **estimated** power consumption (live).
    Processes,
}

impl Tab {
    pub fn next(self) -> Self {
        match self {
            Self::Live => Self::Trend,
            Self::Trend => Self::Pattern,
            Self::Pattern => Self::Sessions,
            Self::Sessions => Self::Processes,
            Self::Processes => Self::Live,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Live => "Live",
            Self::Trend => "Trend · 24h",
            Self::Pattern => "Pattern",
            Self::Sessions => "Sessions",
            Self::Processes => "Processes",
        }
    }
}

#[derive(Debug)]
pub struct App {
    pub info: BatteryInfo,
    pub sample: Option<BatterySample>,
    /// Live system metrics (CPU/brightness/temperature).
    pub sys: Option<SystemMetrics>,
    sys_reader: SystemReader,
    /// Per-process estimated power (live). Sorted high → low.
    pub processes: Vec<ProcessPower>,
    proc_reader: ProcessReader,
    /// Process tab view: live ↔ last-hour.
    pub process_view: ProcessView,
    /// Process history: per-name aggregate estimated power over the last hour.
    pub process_history: Vec<ProcessAgg>,
    /// Power (W) of the last N samples — for the live sparkline.
    pub power_history: VecDeque<f64>,
    /// SQLite history store (if available). Used by the Trend tab.
    pub store: Option<Store>,
    /// Last 24h of SQLite samples — for the trend charts.
    pub trend: Vec<Sample>,
    /// Pattern-analysis bins (hourly or weekday).
    pub pattern: Vec<HourlyBin>,
    pub pattern_axis: PatternAxis,
    /// On-battery sessions (newest first).
    pub sessions: Vec<Session>,
    /// Anomalies with unexpectedly high power draw (z ≥ 2).
    pub anomalies: Vec<Anomaly>,
    pub tab: Tab,
    /// Compact mode (btop-style dense single-screen). Default on at startup.
    pub compact: bool,
    pub last_update: Option<Instant>,
    pub last_error: Option<String>,
    pub should_quit: bool,
}

impl App {
    pub fn new(battery: &Battery, store: Option<Store>) -> Self {
        Self {
            info: battery.info.clone(),
            sample: None,
            sys: None,
            sys_reader: SystemReader::new(),
            processes: Vec::new(),
            proc_reader: ProcessReader::new(),
            process_view: ProcessView::Live,
            process_history: Vec::new(),
            power_history: VecDeque::with_capacity(HISTORY_LEN),
            store,
            trend: Vec::new(),
            pattern: Vec::new(),
            pattern_axis: PatternAxis::Hourly,
            sessions: Vec::new(),
            anomalies: Vec::new(),
            tab: Tab::Live,
            compact: true,
            last_update: None,
            last_error: None,
            should_quit: false,
        }
    }

    /// Read sysfs and update state. On failure, store the error (UI must not crash).
    pub fn refresh(&mut self, battery: &Battery) {
        match battery.read() {
            Ok(sample) => {
                if self.power_history.len() >= HISTORY_LEN {
                    self.power_history.pop_front();
                }
                if let Some(p) = sample.power_now {
                    self.power_history.push_back(p);
                }
                self.last_update = Some(Instant::now());
                self.last_error = None;
                self.sample = Some(sample);
            }
            Err(e) => self.last_error = Some(format!("{e:#}")),
        }
        // Also read system metrics (stateful reader for the CPU delta).
        self.sys = Some(self.sys_reader.read());
        // Per-process estimated power (for the live tab).
        self.processes = self.proc_reader.top(15);
        if self.process_view == ProcessView::LastHour {
            self.refresh_process_history();
        }
        // Refresh trend data too (from SQLite).
        self.refresh_trend();
        self.refresh_pattern();
        self.refresh_sessions();
    }

    /// Load the last 24 hours from the SQLite history.
    pub fn refresh_trend(&mut self) {
        if let Some(store) = &self.store {
            match store.query_since(TREND_WINDOW_SECS) {
                Ok(samples) => self.trend = samples,
                Err(e) => self.last_error = Some(format!("history: {e:#}")),
            }
        }
    }

    /// Load pattern bins from the SQLite history.
    pub fn refresh_pattern(&mut self) {
        if let Some(store) = &self.store {
            let res = match self.pattern_axis {
                PatternAxis::Hourly => store.query_hourly_pattern(),
                PatternAxis::Weekday => store.query_weekday_pattern(),
            };
            match res {
                Ok(bins) => self.pattern = bins,
                Err(e) => self.last_error = Some(format!("pattern: {e:#}")),
            }
        }
    }

    /// Advance to the next tab.
    pub fn next_tab(&mut self) {
        self.tab = self.tab.next();
        self.refresh_trend();
        self.refresh_pattern();
        self.refresh_sessions();
        self.refresh_anomalies();
    }

    /// Switch the pattern axis (hourly ↔ weekday).
    pub fn toggle_pattern_axis(&mut self) {
        self.pattern_axis = self.pattern_axis.toggle();
        self.refresh_pattern();
    }

    /// Toggle compact mode ('c').
    pub fn toggle_compact(&mut self) {
        self.compact = !self.compact;
    }

    /// Switch the process tab view: live ↔ last-hour ('d').
    pub fn toggle_process_view(&mut self) {
        self.process_view = match self.process_view {
            ProcessView::Live => ProcessView::LastHour,
            ProcessView::LastHour => ProcessView::Live,
        };
        if self.process_view == ProcessView::LastHour {
            self.refresh_process_history();
        }
    }

    /// Load per-name aggregate process power for the last hour from SQLite.
    pub fn refresh_process_history(&mut self) {
        if let Some(store) = &self.store {
            match store.query_top_processes(3600) {
                Ok(p) => self.process_history = p,
                Err(e) => self.last_error = Some(format!("process history: {e:#}")),
            }
        }
    }

    /// Go to a specific tab (no-op if already there).
    pub fn goto_tab(&mut self, tab: Tab) {
        if self.tab != tab {
            self.tab = tab;
            self.refresh_trend();
            self.refresh_pattern();
            self.refresh_sessions();
            self.refresh_anomalies();
        }
    }

    /// Load on-battery sessions.
    pub fn refresh_sessions(&mut self) {
        if let Some(store) = &self.store {
            // A gap larger than 10 minutes starts a new session.
            match store.query_sessions(600) {
                Ok(s) => self.sessions = s,
                Err(e) => self.last_error = Some(format!("sessions: {e:#}")),
            }
        }
    }

    /// Anomaly detection: power spikes with z-score ≥ 2.
    pub fn refresh_anomalies(&mut self) {
        if let Some(store) = &self.store {
            match store.query_anomalies(2.0) {
                Ok(a) => self.anomalies = a,
                Err(e) => self.last_error = Some(format!("anomalies: {e:#}")),
            }
        }
    }
}

/// A message delivered to the app by keyboard/tick events.
#[derive(Debug)]
pub enum Message {
    /// Refresh data (manual 'r' or automatic tick).
    Refresh,
    /// Change tab (Tab).
    NextTab,
    /// Switch pattern axis (hourly ↔ weekday, 'd').
    TogglePatternAxis,
    /// Switch process view (live ↔ last-hour, 'd').
    ToggleProcessView,
    /// Toggle compact mode ('c').
    ToggleCompact,
    Quit,
}
