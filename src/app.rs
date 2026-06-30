//! Uygulama durumu — Elm Architecture (Model → Message → Update).

use std::collections::VecDeque;
use std::time::Instant;

use crate::battery::{Battery, BatteryInfo, BatterySample};
use crate::process::{ProcessPower, ProcessReader};
use crate::storage::{Anomaly, HourlyBin, ProcessAgg, Sample, Session, Store};
use crate::system::{SystemMetrics, SystemReader};

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

/// Process sekmesinin görünümü: canlı veya son-1-saat-top (history).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessView {
    Live,
    LastHour,
}

/// Aktif görünüm sekmesi.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// Canlı sysfs paneli.
    Live,
    /// SQLite'ten 24 saatlik history trendi.
    Trend,
    /// Saat-bazına / gün-bazına kullanım deseni.
    Pattern,
    /// On-battery oturumları (prizden-çek → prize-tak döngüleri).
    Sessions,
    /// Process başına **tahmini** güç tüketimi (canlı).
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
    /// Canlı sistem metrikleri (CPU/parlaklık/sıcaklık).
    pub sys: Option<SystemMetrics>,
    sys_reader: SystemReader,
    /// Process başına tahmini güç (canlı). Sıralı: yüksek→düşük.
    pub processes: Vec<ProcessPower>,
    proc_reader: ProcessReader,
    /// Process sekmesi görünümü: canlı ↔ son-1-saat.
    pub process_view: ProcessView,
    /// Process history: son 1 saatte ada göre toplu tahmini güç.
    pub process_history: Vec<ProcessAgg>,
    /// Son N ölçümün gücü (W) — canlı sparkline için.
    pub power_history: VecDeque<f64>,
    /// SQLite history deposu (varsa). Trend sekmesi bunu kullanır.
    pub store: Option<Store>,
    /// Son 24 saatin SQLite örnekleri — trend grafiği için.
    pub trend: Vec<Sample>,
    /// Desen analizi sepetleri (saatlik veya günlük).
    pub pattern: Vec<HourlyBin>,
    pub pattern_axis: PatternAxis,
    /// On-battery oturumları (en yeni en üstte).
    pub sessions: Vec<Session>,
    /// Beklenenden yüksek güç tüketen anomaliler (z≥2).
    pub anomalies: Vec<Anomaly>,
    pub tab: Tab,
    /// Compact mod (btop tarzı yoğun tek-ekran). Açılışta default açık.
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
        // Sistem metriklerini de oku (CPU delta için state'li reader).
        self.sys = Some(self.sys_reader.read());
        // Process başına tahmini güç (canlı sekme için).
        self.processes = self.proc_reader.top(15);
        if self.process_view == ProcessView::LastHour {
            self.refresh_process_history();
        }
        // Trend verisini de tazele (SQLite'ten).
        self.refresh_trend();
        self.refresh_pattern();
        self.refresh_sessions();
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
        self.refresh_sessions();
        self.refresh_anomalies();
    }

    /// Pattern eksenini değiştir (saatlik ↔ günlük).
    pub fn toggle_pattern_axis(&mut self) {
        self.pattern_axis = self.pattern_axis.toggle();
        self.refresh_pattern();
    }

    /// Compact modu aç/kapat ('c').
    pub fn toggle_compact(&mut self) {
        self.compact = !self.compact;
    }

    /// Process sekmesi görünümünü değiştir: canlı ↔ son-1-saat ('d').
    pub fn toggle_process_view(&mut self) {
        self.process_view = match self.process_view {
            ProcessView::Live => ProcessView::LastHour,
            ProcessView::LastHour => ProcessView::Live,
        };
        if self.process_view == ProcessView::LastHour {
            self.refresh_process_history();
        }
    }

    /// SQLite'ten son 1 saatte ada göre toplu process gücünü yükle.
    pub fn refresh_process_history(&mut self) {
        if let Some(store) = &self.store {
            match store.query_top_processes(3600) {
                Ok(p) => self.process_history = p,
                Err(e) => self.last_error = Some(format!("process history: {e:#}")),
            }
        }
    }

    /// Belirli bir sekmeye git (zaten oradaysa no-op).
    pub fn goto_tab(&mut self, tab: Tab) {
        if self.tab != tab {
            self.tab = tab;
            self.refresh_trend();
            self.refresh_pattern();
            self.refresh_sessions();
            self.refresh_anomalies();
        }
    }

    /// On-battery oturumlarını yükle.
    pub fn refresh_sessions(&mut self) {
        if let Some(store) = &self.store {
            // 10 dk'dan büyük boşluk yeni oturum sayılır.
            match store.query_sessions(600) {
                Ok(s) => self.sessions = s,
                Err(e) => self.last_error = Some(format!("sessions: {e:#}")),
            }
        }
    }

    /// Anomali tespiti: z-skoru ≥ 2 olan güç spike'ları.
    pub fn refresh_anomalies(&mut self) {
        if let Some(store) = &self.store {
            match store.query_anomalies(2.0) {
                Ok(a) => self.anomalies = a,
                Err(e) => self.last_error = Some(format!("anomalies: {e:#}")),
            }
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
    /// Process görünümünü değiştir (canlı ↔ son-1-saat, 'd').
    ToggleProcessView,
    /// Compact modu aç/kapat ('c').
    ToggleCompact,
    Quit,
}
