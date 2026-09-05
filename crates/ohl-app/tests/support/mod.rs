//! Project-authored synthetic media fixtures shared by the composition
//! root's integration tests and its own `cfg(test)` unit tests.
//!
//! Every byte here is invented. The PE header layout follows Microsoft's
//! public "PE Format" specification (recorded in `docs/FORMAT_SOURCES.md`);
//! the image contains no code and is never executed, loaded, or linked.

#![allow(dead_code, reason = "each test target uses a subset")]

use std::sync::Arc;

/// Builds a single-file ISO 9660 image holding `contents` as `SETUP.EXE`.
#[must_use]
pub fn synthetic_iso(contents: Vec<u8>) -> Vec<u8> {
    use hadris_iso::read::PathSeparator;
    use hadris_iso::write::options::{BaseIsoLevel, CreationFeatures, IsoFormatOptions};
    use hadris_iso::write::{File as IsoFile, InputFiles, IsoImageWriter};

    let capacity = contents.len() + 4 * 1024 * 1024;
    let files = InputFiles {
        path_separator: PathSeparator::ForwardSlash,
        files: vec![IsoFile::File {
            name: Arc::new(String::from("SETUP.EXE")),
            contents,
        }],
    };
    let options = IsoFormatOptions {
        volume_name: String::from("SYNTHETIC"),
        system_id: None,
        volume_set_id: None,
        publisher_id: None,
        preparer_id: None,
        application_id: None,
        sector_size: 2_048,
        path_separator: PathSeparator::ForwardSlash,
        features: CreationFeatures {
            filenames: BaseIsoLevel::Level1 {
                supports_lowercase: false,
                supports_rrip: false,
            },
            long_filenames: false,
            joliet: None,
            rock_ridge: None,
            el_torito: None,
            hybrid_boot: None,
        },
        strict_charset: false,
    };
    let mut buffer = std::io::Cursor::new(vec![0u8; capacity]);
    IsoImageWriter::create(&mut buffer, files, options).expect("synthetic iso image");
    buffer.into_inner()
}

/// A synthetic ISO whose only file is a PE with an InstallShield 3 Z
/// signature at the start of its overlay: exactly one located container.
#[must_use]
pub fn synthetic_container_iso() -> Vec<u8> {
    synthetic_iso(ohl_import::testing::synthetic_pe_with_z_overlay(128 * 1024))
}

/// Deterministic, incompressible bytes, so a synthetic package is larger than
/// one read window and one data chunk.
#[must_use]
pub fn noise(len: usize, seed: u32) -> Vec<u8> {
    let mut state = seed | 1;
    (0..len)
        .map(|_| {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 24) as u8
        })
        .collect()
}

/// The files the synthetic Wise package carries, with their recorded paths.
#[must_use]
pub fn synthetic_wise_files() -> Vec<ohl_wise::testing::SyntheticFile> {
    use ohl_wise::testing::SyntheticFile;
    vec![
        SyntheticFile::new(b"%MAINDIR%\\valve\\one.bsp", noise(200_000, 0x1234)),
        SyntheticFile::new(
            b"%MAINDIR%\\valve\\two.cfg",
            b"alpha beta gamma ".repeat(64),
        ),
        SyntheticFile::new(b"%MAINDIR%\\three.wad", noise(90_000, 0x9876)),
    ]
}

/// A synthetic ISO whose only file is a PE carrying a synthetic Wise package
/// in its overlay.
#[must_use]
pub fn synthetic_wise_iso() -> Vec<u8> {
    use ohl_wise::testing::{PackageOptions, build_package};
    synthetic_iso(build_package(&PackageOptions::with_files(synthetic_wise_files())).image)
}
