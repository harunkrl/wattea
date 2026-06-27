//! Uygulama durumu — Elm Architecture (Model → Message → Update).

use std::collections::VecDeque;
use std::time::Instant;

use crate::battery::{Battery, BatteryInfo, BatterySample};
use crate::storage::{HourlyBin, Sample, Store};

/// Sparkline'da tutulan canlı örnek sayısı (≈ 5 dk @ 1 sn örnekleme).
const HISTORY_LEN: usize = 300;
/// Trend sekmesinde gösterilen pencere: son 24 saat.
pub const TREND_WINDOW_SECS: u64 = 24 * 3600;

/// Pattern sekmesinin ekseni: günün saati veya haftanın günü.
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

/// Aktif görünüm sekmesi.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// Canlı sysfs paneli.
    Live,
    /// SQLite'ten 24 saatlik history trendi.
    Trend,
    /// Saat-bazına / gün-bazına kullanım deseni (killer feature).
    Pattern,
}

impl Tab {
    pub fn next(self) -> Self {
        match self {
            Self::Live => Self::Trend,
            Self::Trend => Self::Pattern,
            Self::Pattern => Self::Live,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Live => "Live",
            Self::Trend => "Trend · 24h",
            Self::Pattern => "Pattern",
        }
    }
}

#[derive(Debug)]
pub struct App {
    pub info: BatteryInfo,
    pub sample: Option<BatterySample>,
    /// Son N ölçümün gücü (W) — canlı sparkline için.
    pub power_history: VecDeque<f64>,
    /// SQLite history deposu (varsa). Trend sekmesi bunu kullanır.
    pub store: Option<Store>,
    /// Son 24 saatin SQLite örnekleri — trend grafiği için.
    pub trend: Vec<Sample>,
    /// Desen analizi sepetleri (saatlik veya günlük).
    pub pattern: Vec<HourlyBin>,
    pub pattern_axis: PatternAxis,
    pub tab: Tab,
    pub last_update: Option<Instant>,
    pub last_error: Option<String>,
    pub should_quit: bool,
}

impl App {
    pub fn new(battery: &Battery, store: Option<Store>) -> Self {
        Self {
            info: battery.info.clone(),
            sample: None,
            power_history: VecDeque::with_capacity(HISTORY_LEN),
            store,
            trend: Vec::new(),
            pattern: Vec::new(),
            pattern_axis: PatternAxis::Hourly,
            tab: Tab::Live,
            last_update: None,
            last_error: None,
            should_quit: false,
        }
    }

    /// sysfs'i oku, durumu güncelle. Başarısız olursa hatayı sakla (UI düşmesin).
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
        // Trend verisini de tazele (SQLite'ten).
        self.refresh_trend();
        self.refresh_pattern();
    }

    /// SQLite history'sinden son 24 saati yükle.
    pub fn refresh_trend(&mut self) {
        if let Some(store) = &self.store {
            match store.query_since(TREND_WINDOW_SECS) {
                Ok(samples) => self.trend = samples,
                Err(e) => self.last_error = Some(format!("history: {e:#}")),
            }
        }
    }

    /// SQLite history'sinden desen sepetlerini yükle.
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

    /// Sonraki sekmeye geç.
    pub fn next_tab(&mut self) {
        self.tab = self.tab.next();
        self.refresh_trend();
        self.refresh_pattern();
    }

    /// Pattern eksenini değiştir (saatlik ↔ günlük).
    pub fn toggle_pattern_axis(&mut self) {
        self.pattern_axis = self.pattern_axis.toggle();
        self.refresh_pattern();
    }

    /// Belirli bir sekmeye git (zaten oradaysa no-op).
    pub fn goto_tab(&mut self, tab: Tab) {
        if self.tab != tab {
            self.tab = tab;
            self.refresh_trend();
            self.refresh_pattern();
        }
    }
}

/// Klavye/tick olaylarının uygulamaya ilettiği mesaj.
#[derive(Debug)]
pub enum Message {
    /// Veriyi yenile (manuel 'r' veya otomatik tick).
    Refresh,
    /// Sekme değiştir (Tab).
    NextTab,
    /// Pattern eksenini değiştir (saatlik ↔ günlük, 'd').
    TogglePatternAxis,
    Quit,
}
