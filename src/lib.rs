//! Wattea — paylaşılan çekirdek kütüphane.
//!
//! Hem TUI (`wattea`) hem collector daemon (`wattea-daemon`) tarafından
//! kullanılan ortak kod: batarya okuma, SQLite depolama, model tipleri.

pub mod app;
pub mod battery;
pub mod storage;
pub mod ui;
pub mod upower;

#[cfg(test)]
mod tests;

use std::path::PathBuf;

/// Wattea'nın veri dizini: `~/.local/share/wattea/`.
///
/// SQLite DB ve ileride diğer runtime verisi burada tutulur.
pub fn data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("wattea"))
}

/// SQLite veritabanı dosyasının tam yolu.
pub fn db_path() -> Option<PathBuf> {
    data_dir().map(|d| d.join("db.sqlite"))
}
