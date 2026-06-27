//! Uygulama durumu — Elm Architecture (Model → Message → Update).

use std::collections::VecDeque;
use std::time::Instant;

use crate::battery::{Battery, BatteryInfo, BatterySample};
use crate::storage::{Sample, Store};

/// Sparkline'da tutulan canlı örnek sayısı (≈ 5 dk @ 1 sn örnekleme).
const HISTORY_LEN: usize = 300;
/// Trend sekmesinde gösterilen pencere: son 24 saat.
pub const TREND_WINDOW_SECS: u64 = 24 * 3600;

/// Aktif görünüm sekmesi.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// Canlı sysfs paneli.
    Live,
    /// SQLite'ten 24 saatlik history trendi.
    Trend,
}

impl Tab {
    pub fn next(self) -> Self {
        match self {
            Self::Live => Self::Trend,
            Self::Trend => Self::Live,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Live => "Live",
            Self::Trend => "Trend · 24h",
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

    /// Sonraki sekmeye geç.
    pub fn next_tab(&mut self) {
        self.tab = self.tab.next();
        self.refresh_trend();
    }
}

/// Klavye/tick olaylarının uygulamaya ilettiği mesaj.
#[derive(Debug)]
pub enum Message {
    /// Veriyi yenile (manuel 'r' veya otomatik tick).
    Refresh,
    /// Sekme değiştir (Tab/2).
    NextTab,
    Quit,
}
