//! Steno machines: the `Machine` trait, the Auto scanner, and one module per
//! protocol.
//!
//! The scanner's contract is the reason this project exists: absent hardware is
//! a state, not a failure. It retries forever and never requires user action.
//! Read `thingstonote.md` before implementing a protocol.
//!
//! Populated in M4a (trait, keymap wiring, Gemini PR) and M4b (Stenograph USB).
