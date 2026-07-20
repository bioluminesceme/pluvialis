//! Keystroke emulation into other applications, via Win32 `SendInput`.
//!
//! Only used when a window other than Pluvialis has focus. When Pluvialis is
//! focused, output goes to the in-app document instead and this crate is not
//! called at all. **Exactly one destination per output batch, never both.**
//! That is what makes double typing structurally impossible rather than an
//! intermittent bug to chase.
//!
//! Text is sent as Unicode scan codes rather than virtual keys, so it does not
//! depend on the user's keyboard layout: a Dutch layout and a US layout produce
//! the same characters. Key combos (`{#Control_L(Left)}`) do use virtual keys,
//! because they mean physical keys rather than characters.

pub mod keys;

#[cfg(windows)]
mod send;

#[cfg(windows)]
pub use send::Keyboard;

pub use keys::{Chord, parse_combo};

#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("SendInput accepted {sent} of {expected} events")]
    Partial { sent: usize, expected: usize },

    #[error("unknown key name {0:?}")]
    UnknownKey(String),

    #[error("malformed key combo {0:?}")]
    MalformedCombo(String),
}
