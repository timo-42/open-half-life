//! Round-trip, accessor, and malformed-field rejection tests for `spr`,
//! using this crate's own synthetic fixture writer
//! (`ohl_formats::test_support::build_minimal_spr`). No bytes here come
//! from any game installation; see `docs/CLEAN_ROOM.md`.

use core::mem::offset_of;
use ohl_formats::spr::{Limits, RawHeader, Spr, SpriteType, SyncType, TextureFormat};
use ohl_formats::test_support::build_minimal_spr;

fn corrupt_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn corrupt_i32(bytes: &mut [u8], offset: usize, value: i32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

#[test]
fn round_trips_header_palette_and_frames() {
    let bytes = build_minimal_spr();
    let limits = Limits::default();
    let spr = Spr::parse(&bytes, &limits).expect("valid synthetic sprite parses");

    assert_eq!(spr.kind(), SpriteType::ParallelUpright);
    assert_eq!(spr.texture_format(), TextureFormat::Normal);
    assert_eq!(spr.sync_type(), SyncType::Synchronized);
    assert_eq!(spr.max_size(), (8, 8));
    assert_eq!(spr.frame_count(), 2);
    assert_eq!(spr.palette().len(), 256);

    let frame0 = spr.frame(0, &limits).unwrap();
    assert_eq!(frame0.origin_x, -4);
    assert_eq!(frame0.origin_y, -4);
    assert_eq!(frame0.image.width, 8);
    assert_eq!(frame0.image.height, 8);
    assert_eq!(frame0.image.pixel(0, 0).unwrap().r, 1);

    let frame1 = spr.frame(1, &limits).unwrap();
    assert_eq!(frame1.image.pixel(0, 0).unwrap().r, 2);

    assert!(spr.frame(2, &limits).is_err());
}

// --- Malformed-field rejection tests -------------------------------------

#[test]
fn rejects_bad_magic() {
    let mut bytes = build_minimal_spr();
    bytes[0..4].copy_from_slice(b"NOPE");
    assert!(Spr::parse(&bytes, &Limits::default()).is_err());
}

#[test]
fn rejects_bad_version() {
    let mut bytes = build_minimal_spr();
    let off = offset_of!(RawHeader, version);
    corrupt_i32(&mut bytes, off, 1);
    assert!(Spr::parse(&bytes, &Limits::default()).is_err());
}

#[test]
fn rejects_frame_count_over_limit() {
    let mut bytes = build_minimal_spr();
    let off = offset_of!(RawHeader, num_frames);
    corrupt_u32(&mut bytes, off, 1_000_000);
    assert!(Spr::parse(&bytes, &Limits::default()).is_err());
}

#[test]
fn rejects_palette_length_over_limit() {
    let mut bytes = build_minimal_spr();
    let limits = Limits {
        max_palette_colors: 10,
        ..Limits::default()
    };
    assert!(Spr::parse(&bytes, &limits).is_err());
    // Also exercise a palette-count field that overruns the file.
    let header_size = core::mem::size_of::<RawHeader>();
    bytes[header_size..header_size + 2].copy_from_slice(&0xFFFFu16.to_le_bytes());
    assert!(Spr::parse(&bytes, &Limits::default()).is_err());
}

#[test]
fn rejects_frame_dimensions_causing_out_of_bounds_pixels() {
    let mut bytes = build_minimal_spr();
    let header_size = core::mem::size_of::<RawHeader>();
    let palette_bytes = 2 + 256 * 3;
    let frame_start = header_size + palette_bytes;
    // Frame layout: group(4) origin_x(4) origin_y(4) width(4) height(4).
    let width_field = frame_start + 12;
    corrupt_u32(&mut bytes, width_field, 0xFFFF);
    assert!(Spr::parse(&bytes, &Limits::default()).is_err());
}

#[test]
fn rejects_frame_index_out_of_range() {
    let bytes = build_minimal_spr();
    let limits = Limits::default();
    let spr = Spr::parse(&bytes, &limits).unwrap();
    assert!(spr.frame(99, &limits).is_err());
}
