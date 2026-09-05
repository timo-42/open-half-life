# ohl-save

A project-owned, versioned save-file container for Open Half-Life.

**This is not the id Tech/GoldSrc `.sav`/`.hl1` save format.** It is a
from-scratch binary container designed for this project: a fixed magic, a
format version, a bounded header, a tagged section table with a per-section
SHA-256 digest, the section payloads, and a whole-file SHA-256 trailer. See
`src/container.rs` for the exact on-disk layout and versioning rules, and
`docs/ARCHITECTURE.md`'s "Save files" paragraph for how this fits the rest of
the Rust port.

## Layout

```text
magic                 8 bytes,  fixed b"OHLSAVE\0"
format_major          u16 LE
format_minor          u16 LE
header                bounded: game version, creation time, map identity,
                       chapter/title, and a reserved thumbnail slot
section_count         u32 LE
section_table[count]  tag(u32) offset(u64) length(u64) sha256([u8;32])
section_data[count]   concatenated, in table order
trailer_sha256        32 bytes, SHA-256 of every byte above
```

Every offset and length taken from a file is validated against the file size
and a caller-supplied [`Limits`](src/limits.rs) before it is trusted. A major
version mismatch is always rejected; a minor version mismatch and
reserved-tag section entries this build does not interpret are tolerated
(see `SaveReader::unknown_section_count`).

## API

- `SaveWriter::begin(header)` → `add_section(tag, bytes)` /
  `add_section_serde(tag, &value)` (encodes with `postcard`) →
  `finish(&limits) -> Vec<u8>`.
- `SaveReader::open(bytes, &limits)` → `header()`, `sections()`,
  `section(tag) -> &[u8]`, `deserialize::<T>(tag)`.
- `SaveSlot`: a directory of `<slot>.ohlsave` files, published with a
  write-to-temp-then-rename so a slot is never observed partially written;
  see the guarantee documented on `src/slot.rs`. Includes `AUTOSAVE_SLOT_NAME`
  / `QUICKSAVE_SLOT_NAME`, bounded `list()`, and `delete()`.

## Testing

Unit tests cover round-tripping, every-field tamper (header, table, section
digest, trailer), truncation at every byte length (including a dedicated
64-byte-boundary sweep), limits enforcement, unknown-section skipping, and
minor/major version rules. `proptest` checks that `SaveReader::open` never
panics on arbitrary bytes and that arbitrary headers/sections round-trip
exactly. `tests/integration.rs` exercises the same guarantees through the
public API only, including `SaveSlot` listing over a temporary directory.

`fuzz/` is a standalone `cargo-fuzz` package (see its own `Cargo.toml` for
why it is not a workspace member) with two targets:

```
cargo +nightly fuzz run open_fuzz
cargo +nightly fuzz run roundtrip_fuzz
```

Both ran clean for 60 seconds (15M+ and 2.8M+ executions respectively) with
no crashes during development.
