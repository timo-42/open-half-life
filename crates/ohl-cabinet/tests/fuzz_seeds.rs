//! Writes synthetic fuzz seeds, on request, for the standalone `fuzz/`
//! workspace.
//!
//! Ignored by default. Run with `OHL_FUZZ_SEED_DIR` pointing at a corpus
//! directory. Every seed is independently authored synthetic data, so no
//! proprietary bytes can enter a corpus.

use std::path::PathBuf;

use ohl_cabinet::testing::{CabinetBuilder, SynthEntry};
use ohl_cabinet_format::testing::{HeaderBuilder, SynthFile};

fn header_seeds() -> Vec<Vec<u8>> {
    let sample = |builder: HeaderBuilder| {
        builder
            .directory("bin")
            .directory("data")
            .file(SynthFile::new("first", 10, 0x100))
            .file(SynthFile {
                flags: ohl_cabinet_format::FILE_COMPRESSED,
                compressed_size: 7,
                ..SynthFile::new("second", 20, 0x200)
            })
            .group("group", 0, 1)
            .component("component", &["group"])
            .build()
    };
    vec![
        sample(HeaderBuilder::v5()),
        sample(HeaderBuilder::v6()),
        sample(HeaderBuilder::is2003()),
        HeaderBuilder::v6().build(),
    ]
}

fn extract_seeds() -> Vec<Vec<u8>> {
    let mut seeds = Vec::new();
    for builder in [
        CabinetBuilder::v5(),
        CabinetBuilder::v6(),
        CabinetBuilder::is2003(),
    ] {
        let cabinet = builder
            .directory("bin")
            .chunk_bytes(512)
            .entry(SynthEntry::new("plain", &[7u8; 600]))
            .entry(SynthEntry::new("packed", &[3u8; 2000]).compressed())
            .entry(SynthEntry::new("hidden", &[9u8; 400]).obfuscated())
            .entry(SynthEntry::new("split", &[1u8; 3000]).split_after(700))
            .build();
        // The extract target reads a two-byte split point, then the header,
        // then the first volume's bytes.
        let mut seed = Vec::new();
        let split = u16::try_from(cabinet.header.len()).unwrap();
        seed.extend_from_slice(&split.to_le_bytes());
        seed.extend_from_slice(&cabinet.header);
        seed.extend_from_slice(&cabinet.volumes[0]);
        seeds.push(seed);
    }
    seeds
}

#[test]
#[ignore = "writes fuzz seeds only when OHL_FUZZ_SEED_DIR is set"]
fn writes_synthetic_fuzz_seeds() {
    let Ok(root) = std::env::var("OHL_FUZZ_SEED_DIR") else {
        println!("OHL_FUZZ_SEED_DIR is not set; skipping");
        return;
    };
    let root = PathBuf::from(root);
    let header_dir = root.join("cabinet_header");
    let extract_dir = root.join("cabinet_extract");
    std::fs::create_dir_all(&header_dir).unwrap();
    std::fs::create_dir_all(&extract_dir).unwrap();

    for (index, seed) in header_seeds().into_iter().enumerate() {
        std::fs::write(header_dir.join(format!("synthetic-{index}")), seed).unwrap();
    }
    for (index, seed) in extract_seeds().into_iter().enumerate() {
        std::fs::write(extract_dir.join(format!("synthetic-{index}")), seed).unwrap();
    }
    println!("wrote synthetic fuzz seeds");
}
