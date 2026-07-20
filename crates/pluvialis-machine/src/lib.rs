//! Steno machines: the `Machine` trait, the Auto scanner, and one module per
//! protocol.
//!
//! The scanner's contract is the reason this project exists: absent hardware is
//! a state, not a failure. It retries forever and never requires user action.
//! Read `thingstonote.md` before implementing a protocol.
//!
//! Three layers, deliberately separate:
//!
//! 1. A protocol module ([`gemini`]) turns bytes into **machine keys**. It does
//!    not know what a steno key is.
//! 2. The [`keymap`] collapses machine keys onto the 23 steno keys. This is
//!    where a split S becomes one `S-`.
//! 3. The [`scanner`] owns connection policy, so no protocol reimplements
//!    retrying and none can get it wrong by giving up.

pub mod gemini;
pub mod keymap;
pub mod machine;
pub mod scanner;
pub mod stenograph;

pub use gemini::GeminiPr;
pub use keymap::Keymap;
pub use machine::{Machine, MachineError, MachineEvent, MachineStatus};
pub use scanner::Scanner;
#[cfg(windows)]
pub use stenograph::Stenograph;

/// Every machine Pluvialis can speak to, in the order Auto mode tries them.
///
/// Stenograph first: it is the user's Luminex, her primary writer, and if both
/// are attached that is the one she means. Gemini PR is the Peregrine.
pub fn all_machines() -> Vec<Box<dyn Machine>> {
    vec![
        #[cfg(windows)]
        Box::new(Stenograph::new()),
        Box::new(GeminiPr::new()),
    ]
}
