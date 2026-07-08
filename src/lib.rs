//! Wattea — shared core library.
//!
//! Common code used by both the TUI (`wattea`) and the collector daemon
//! (`wattea-daemon`): battery reading, SQLite storage, and shared model types.

pub mod app;
pub mod battery;
pub mod process;
pub mod storage;
pub mod system;
pub mod ui;
pub mod upower;

#[cfg(test)]
mod tests;

use std::path::PathBuf;

/// Wattea data directory: `~/.local/share/wattea/`.
///
/// Holds the SQLite database and any future runtime artifacts.
pub fn data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("wattea"))
}

/// Full path to the SQLite database file.
pub fn db_path() -> Option<PathBuf> {
    data_dir().map(|d| d.join("db.sqlite"))
}
