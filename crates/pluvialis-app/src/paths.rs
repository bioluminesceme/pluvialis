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
//!
//! The one exception is a Cargo build directory, which resolves to the project
//! root instead; see [`resolve`].

use std::path::{Path, PathBuf};

/// The folder the executable is in, which is the base for all app data. Falls
/// back to the current directory if the executable path cannot be read, which
/// is rare and leaves the program usable rather than refusing to start.
pub fn base_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(PathBuf::from))
        .map(|dir| resolve(&dir))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// An executable inside a Cargo build directory uses the project root as its
/// base, not the build directory.
///
/// On a development machine the same program exists at two paths: `cargo build`
/// writes `target\release\pluvialis-app.exe`, and that file is then copied to
/// the project root so it does not live somewhere `cargo clean` deletes. Data
/// beside the executable would therefore be two separate libraries, one of them
/// destroyed by a routine clean. That is not hypothetical: a taskbar shortcut
/// pointed into `target\release\`, and the dictionaries diverged unnoticed for
/// days until the two copies were compared.
///
/// Only a `debug` or `release` folder whose parent is `target` is treated this
/// way, so an installation in a folder the user happens to have named `release`
/// keeps its own data.
fn resolve(exe_dir: &Path) -> PathBuf {
    let in_profile_dir = matches!(
        exe_dir.file_name().and_then(|name| name.to_str()),
        Some("debug" | "release")
    );
    if !in_profile_dir {
        return exe_dir.to_path_buf();
    }

    // `target\release\` normally, `target\<triple>\release\` when a target
    // triple is given, so `target` can be one or two levels up.
    let parent = exe_dir.parent();
    for candidate in [parent, parent.and_then(Path::parent)] {
        let Some(dir) = candidate else { continue };
        if dir.file_name().and_then(|name| name.to_str()) == Some("target")
            && let Some(root) = dir.parent()
        {
            return root.to_path_buf();
        }
    }

    exe_dir.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Forward slashes so these read the same on either platform; Windows
    // accepts them as separators.

    #[test]
    fn an_installed_exe_uses_its_own_folder() {
        assert_eq!(
            resolve(Path::new("C:/Pluvialis")),
            PathBuf::from("C:/Pluvialis")
        );
    }

    #[test]
    fn a_build_output_uses_the_project_root() {
        assert_eq!(
            resolve(Path::new("F:/Steno/Pluvialis/target/release")),
            PathBuf::from("F:/Steno/Pluvialis")
        );
        assert_eq!(
            resolve(Path::new("F:/Steno/Pluvialis/target/debug")),
            PathBuf::from("F:/Steno/Pluvialis")
        );
    }

    /// `cargo build --target x86_64-pc-windows-msvc` adds a level.
    #[test]
    fn a_target_triple_build_uses_the_project_root_too() {
        assert_eq!(
            resolve(Path::new(
                "F:/Steno/Pluvialis/target/x86_64-pc-windows-msvc/release"
            )),
            PathBuf::from("F:/Steno/Pluvialis")
        );
    }

    /// Without `target` above it, a folder called `release` is just a folder.
    #[test]
    fn a_release_folder_outside_target_keeps_its_own_data() {
        assert_eq!(
            resolve(Path::new("C:/Steno/release")),
            PathBuf::from("C:/Steno/release")
        );
    }
}
