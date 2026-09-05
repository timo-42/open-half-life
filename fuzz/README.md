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
```

Corpora are not committed: every input is synthetic, and no corpus may contain
bytes from any real medium.
