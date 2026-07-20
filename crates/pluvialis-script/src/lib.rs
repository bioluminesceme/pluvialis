//! Lua host for programmatic ("scripty") dictionaries.
//!
//! A `.lua` dictionary exposes `lookup(strokes) -> string | nil` and optionally
//! `reverse_lookup(text)`. Scripts are sandboxed: no filesystem, no network.
//! Only consulted when the JSON dictionaries above it in priority order miss.
//!
//! Populated in M6.
