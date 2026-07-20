//! Steno translation: strokes, keymaps, dictionaries, translation, formatting.
//!
//! Everything here is platform independent. No Windows APIs, no GUI, no I/O
//! beyond reading dictionary files. That boundary is what keeps a Linux port
//! cheap, so resist putting anything OS specific here.
//!
//! Populated in M1 (strokes, dictionaries, translator) and M2 (formatter).
