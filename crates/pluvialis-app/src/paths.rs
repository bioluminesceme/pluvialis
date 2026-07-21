//! Where Pluvialis keeps its data on disk.
//!
//! Everything lives in the folder that holds the executable: `dictionaries\`,
//! `documents\`, and the config file all sit beside `pluvialis-app.exe`. That
//! is a deliberate choice over a hidden per-user location like AppData, so the
//! user can find, back up and edit these files with ordinary tools.
//!
//! Resolving against the executable rather than a hardcoded absolute path is
//! what makes the program portable: copy the exe to any folder, on any machine,
//! and its data folders are created and read right next to it.

use std::path::PathBuf;

/// The folder the executable is in, which is the base for all app data. Falls
/// back to the current directory if the executable path cannot be read, which
/// is rare and leaves the program usable rather than refusing to start.
pub fn base_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."))
}
