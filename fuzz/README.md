# Media preflight fuzzing

This directory is a standalone Cargo workspace so the pinned stable build never
acquires `libfuzzer-sys` or a nightly toolchain. Both targets assert only that
the project-owned bounded preflights terminate without panicking on arbitrary
bytes; the same property is covered deterministically by the `proptest` cases
in `crates/ohl-iso9660` and `crates/ohl-udf`, which do run in CI.

```
cargo install cargo-fuzz
cargo +nightly fuzz run iso9660_preflight
cargo +nightly fuzz run udf_preflight
cargo +nightly fuzz run cabinet_header
cargo +nightly fuzz run cabinet_extract
```

The two cabinet targets cover `ohl-cabinet-format` and `ohl-cabinet`, which are
a licensed derivative of Unshield rather than project-owned parsers; the same
never-panics property is covered deterministically by their `proptest` cases,
which do run in CI. Seed a corpus only from the synthetic writers in
`ohl_cabinet_format::testing` and `ohl_cabinet::testing`; the ignored
`fuzz_seeds` test in `crates/ohl-cabinet` writes them when `OHL_FUZZ_SEED_DIR`
is set.

Corpora are not committed: every input is synthetic, and no corpus may contain
bytes from any real medium.
