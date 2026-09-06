//! A thin library facade over otherwise binary-only modules.
//!
//! `open-half-life`'s composition root (`src/main.rs`) is a binary crate
//! with no public library surface of its own; every module it declares is
//! private to that binary. This lib target exists only so this crate's own
//! `fuzz/` package can reach [`script`], the scripted-input parser fuzzed by
//! `fuzz_targets/script_parse.rs`, without duplicating its source. It is not
//! part of the shipped binary and adds no behavior of its own.
#[path = "script.rs"]
pub mod script;
