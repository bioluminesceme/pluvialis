//! Keystroke emulation into other applications, via Win32 `SendInput`.
//!
//! Only used when a window other than Pluvialis has focus. When Pluvialis is
//! focused, output goes to the in-app document instead and this crate is not
//! called at all. Exactly one destination per output batch, never both.
//!
//! Populated in M5.
