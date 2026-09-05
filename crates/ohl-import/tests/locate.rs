//! Container location over project-authored synthetic PE images and ISOs.
//!
//! No byte here comes from any real medium. The PE header layout follows
//! Microsoft's public "PE Format" specification, recorded in
//! `docs/FORMAT_SOURCES.md`; the file names are invented.

mod support;

use ohl_import::locate::{
    CABINET_SIGNATURE, ContainerKind, INSTALLSHIELD_Z_SIGNATURE, LocateLimits,
    find_container_signature, is_cabinet_at_zero, locate_containers, pe_overlay_offset,
};
use ohl_import::{CancellationSource, CancellationToken};
use support::{
    MountedFixture, PE_HEADER_OFFSET, PE_OVERLAY_OFFSET, synthetic_bytes, synthetic_pe,
    synthetic_pe_with_z_overlay,
};

#[test]
fn a_two_section_pe_reports_the_end_of_its_last_section_as_the_overlay() {
    let image = synthetic_pe(&synthetic_bytes(1_024));
    assert_eq!(pe_overlay_offset(&image), Some(PE_OVERLAY_OFFSET));
}

#[test]
fn a_pe_whose_sections_fill_the_file_has_no_overlay_to_search() {
    let image = synthetic_pe(&[]);
    // The offset is still reported; it is the caller that compares it with
    // the file size and finds nothing past the sections.
    assert_eq!(pe_overlay_offset(&image), Some(PE_OVERLAY_OFFSET));
    assert_eq!(image.len() as u64, PE_OVERLAY_OFFSET);
}

#[test]
fn a_truncated_header_is_refused_at_every_stage() {
    let image = synthetic_pe(&synthetic_bytes(64));

    // No MZ at all.
    let mut no_dos = image.clone();
    no_dos[0] = b'X';
    assert_eq!(pe_overlay_offset(&no_dos), None);

    // e_lfanew past the end of the prefix.
    let mut runaway = image.clone();
    runaway[0x3c..0x40].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(pe_overlay_offset(&runaway), None);

    // No PE signature where e_lfanew points.
    let mut no_pe = image.clone();
    no_pe[PE_HEADER_OFFSET] = b'X';
    assert_eq!(pe_overlay_offset(&no_pe), None);

    // Cut short of the DOS header, the COFF header, and the section table in
    // turn: every prefix that cannot hold the whole table is refused.
    // The section table ends at 0x1d8; one byte short of it is still short.
    for length in [0, 2, 0x3f, PE_HEADER_OFFSET, PE_HEADER_OFFSET + 20, 0x1d7] {
        assert_eq!(
            pe_overlay_offset(&image[..length.min(image.len())]),
            None,
            "a {length}-byte prefix must not yield an overlay"
        );
    }
}

#[test]
fn an_impossible_section_count_is_refused() {
    let mut image = synthetic_pe(&synthetic_bytes(64));
    let coff = PE_HEADER_OFFSET + 4;
    image[coff + 2..coff + 4].copy_from_slice(&0_u16.to_le_bytes());
    assert_eq!(pe_overlay_offset(&image), None);
    image[coff + 2..coff + 4].copy_from_slice(&97_u16.to_le_bytes());
    assert_eq!(pe_overlay_offset(&image), None);
}

#[test]
fn both_container_signatures_are_recognised() {
    let mut window = synthetic_bytes(256);
    window[64..68].copy_from_slice(&INSTALLSHIELD_Z_SIGNATURE.to_le_bytes());
    assert_eq!(
        find_container_signature(&window),
        Some((64, ContainerKind::InstallShieldZ))
    );

    let mut window = synthetic_bytes(256);
    window[32..36].copy_from_slice(&CABINET_SIGNATURE);
    assert_eq!(
        find_container_signature(&window),
        Some((32, ContainerKind::MicrosoftCabinet))
    );

    assert_eq!(find_container_signature(&synthetic_bytes(3)), None);
    assert!(is_cabinet_at_zero(b"MSCFxxxx"));
    assert!(!is_cabinet_at_zero(b"xMSCF"));
}

#[test]
fn a_pe_with_a_z_overlay_is_located_in_a_mounted_image() {
    let file = synthetic_pe_with_z_overlay(128 * 1024);
    let fixture = MountedFixture::new("SETUP.EXE", file.clone());

    let found = locate_containers(
        fixture.mount(),
        &LocateLimits::default(),
        &CancellationToken::default(),
    )
    .expect("a bounded walk");

    assert_eq!(found.len(), 1, "exactly one candidate: {found:?}");
    let candidate = &found[0];
    assert_eq!(candidate.kind, ContainerKind::InstallShieldZ);
    assert_eq!(candidate.offset, PE_OVERLAY_OFFSET);
    assert_eq!(candidate.length, file.len() as u64 - PE_OVERLAY_OFFSET);
    // The candidate's path is media-derived, so only its shape is asserted.
    assert!(candidate.archive_path.as_str().starts_with('/'));
}

#[test]
fn a_cabinet_at_offset_zero_is_located_in_a_mounted_image() {
    let mut file = synthetic_bytes(96 * 1024);
    file[0..4].copy_from_slice(&CABINET_SIGNATURE);
    let fixture = MountedFixture::new("DATA1.CAB", file.clone());

    let found = locate_containers(
        fixture.mount(),
        &LocateLimits::default(),
        &CancellationToken::default(),
    )
    .expect("a bounded walk");

    assert_eq!(found.len(), 1, "exactly one candidate: {found:?}");
    assert_eq!(found[0].kind, ContainerKind::MicrosoftCabinet);
    assert_eq!(found[0].offset, 0);
    assert_eq!(found[0].length, file.len() as u64);
}

#[test]
fn a_file_under_the_minimum_size_is_never_classified() {
    let mut file = synthetic_bytes(1_024);
    file[0..4].copy_from_slice(&CABINET_SIGNATURE);
    let fixture = MountedFixture::new("SMALL.CAB", file);
    let found = locate_containers(
        fixture.mount(),
        &LocateLimits::default(),
        &CancellationToken::default(),
    )
    .expect("a bounded walk");
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_plain_file_yields_no_candidate() {
    let fixture = MountedFixture::new("PLAIN.DAT", synthetic_bytes(96 * 1024));
    let found = locate_containers(
        fixture.mount(),
        &LocateLimits::default(),
        &CancellationToken::default(),
    )
    .expect("a bounded walk");
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_read_budget_of_zero_truncates_the_search_without_failing() {
    let fixture = MountedFixture::new("SETUP.EXE", synthetic_pe_with_z_overlay(128 * 1024));
    let limits = LocateLimits {
        total_read_bytes: 0,
        ..LocateLimits::default()
    };
    let found = locate_containers(fixture.mount(), &limits, &CancellationToken::default())
        .expect("a truncated walk is not an error");
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn an_observed_cancellation_truncates_the_search() {
    let fixture = MountedFixture::new("SETUP.EXE", synthetic_pe_with_z_overlay(128 * 1024));
    let source = CancellationSource::new();
    source.cancel();
    let found = locate_containers(fixture.mount(), &LocateLimits::default(), &source.token())
        .expect("a cancelled walk is not an error");
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn a_candidate_debug_never_prints_its_path() {
    let fixture = MountedFixture::new("SETUP.EXE", synthetic_pe_with_z_overlay(128 * 1024));
    let found = locate_containers(
        fixture.mount(),
        &LocateLimits::default(),
        &CancellationToken::default(),
    )
    .expect("a bounded walk");
    let rendered = format!("{:?}", found[0]);
    assert!(rendered.contains("<redacted>"), "{rendered}");
    assert!(!rendered.contains("SETUP"), "{rendered}");
}
