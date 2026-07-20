//! Steno translation: strokes, keymaps, dictionaries, translation, formatting.
//!
//! Everything here is platform independent. No Windows APIs, no GUI, no I/O
//! beyond reading dictionary files. That boundary is what keeps a Linux port
//! cheap, so resist putting anything OS specific here.
//!
//! The pipeline is: [`Stroke`] parsing, [`DictionaryStack`] lookup in priority
//! order, then [`Translator`] longest match with retroactive correction.
//! Formatting (spacing, capitalization, meta commands) arrives in M2.

pub mod dictionary;
pub mod stroke;
pub mod translator;

pub use dictionary::{Dictionary, DictionaryError, DictionaryStack};
pub use stroke::{Stroke, StrokeError};
pub use translator::{Delta, Translation, Translator};
