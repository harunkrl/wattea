//! Uygulama durumu — Elm Architecture (Model → Message → Update).

use std::collections::VecDeque;
use std::time::Instant;

use crate::battery::{Battery, BatteryInfo, BatterySample};

/// Sparkline'da tutulan örnek sayısı (≈ 5 dk @ 1 sn örnekleme).
const HISTORY_LEN: usize = 300;

#[derive(Debug)]
pub struct App {
    pub info: BatteryInfo,
    pub sample: Option<BatterySample>,
    /// Son N ölçümün gücü (W) — sparkline için.
    pub power_history: VecDeque<f64>,
    pub last_update: Option<Instant>,
    pub last_error: Option<String>,
    pub should_quit: bool,
}

impl App {
    pub fn new(battery: &Battery) -> Self {
        Self {
            info: battery.info.clone(),
            sample: None,
            power_history: VecDeque::with_capacity(HISTORY_LEN),
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
    }
}

/// Klavye/tick olaylarının uygulamaya ilettiği mesaj.
#[derive(Debug)]
pub enum Message {
    /// Veriyi yenile (manuel 'r' veya otomatik tick).
    Refresh,
    Quit,
}
