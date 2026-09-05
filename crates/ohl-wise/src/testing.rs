//! Synthetic, independently authored package writer used by this workspace's
//! tests and fuzz targets.
//!
//! Every byte it emits is invented here: the PE stub is a minimal, valid
//! section table written from the public PE/COFF specification, the header
//! filler is a constant, the first stream is a synthetic device-independent
//! bitmap, the script binary contains invented file records, and every path
//! is supplied by the caller (the tests use obviously synthetic ones). No
//! byte, name, size or layout comes from any real medium.
//!
//! Never enable the `test-support` feature in production builds.

use alloc::vec;
use alloc::vec::Vec;

use crate::crc32::crc32;

/// Filler byte written where the real format has an undocumented header. It
/// is deliberately not decodable as raw DEFLATE (block type 3 is reserved).
pub const HEADER_FILLER: u8 = 0xff;

/// Offset of the stub's only section's raw data.
const SECTION_RAW_AT: u32 = 0x400;

/// Encodes one file record exactly as the documented field table describes.
#[must_use]
pub fn encode_file_record(path: &[u8], start: u32, end: u32, inflated: u32, crc: u32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(crate::script::RECORD_PREFIX_BYTES + path.len() + 1);
    bytes.extend_from_slice(&[0x00, 0x00]);
    bytes.extend_from_slice(&start.to_le_bytes());
    bytes.extend_from_slice(&end.to_le_bytes());
    bytes.extend_from_slice(&0x2a2au16.to_le_bytes());
    bytes.extend_from_slice(&0x3b3bu16.to_le_bytes());
    bytes.extend_from_slice(&inflated.to_le_bytes());
    bytes.extend_from_slice(&[0u8; 20]);
    bytes.extend_from_slice(&crc.to_le_bytes());
    bytes.extend_from_slice(path);
    bytes.push(0);
    bytes
}

/// A synthetic device-independent bitmap: a `BITMAPINFOHEADER` followed by
/// invented palette and pixel bytes.
#[must_use]
pub fn synthetic_dib() -> Vec<u8> {
    let mut dib = Vec::new();
    dib.extend_from_slice(&40u32.to_le_bytes()); // biSize
    dib.extend_from_slice(&8i32.to_le_bytes()); // biWidth
    dib.extend_from_slice(&8i32.to_le_bytes()); // biHeight
    dib.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
    dib.extend_from_slice(&8u16.to_le_bytes()); // biBitCount
    dib.extend_from_slice(&0u32.to_le_bytes()); // biCompression
    dib.extend_from_slice(&64u32.to_le_bytes()); // biSizeImage
    dib.extend_from_slice(&0i32.to_le_bytes()); // biXPelsPerMeter
    dib.extend_from_slice(&0i32.to_le_bytes()); // biYPelsPerMeter
    dib.extend_from_slice(&4u32.to_le_bytes()); // biClrUsed
    dib.extend_from_slice(&0u32.to_le_bytes()); // biClrImportant
    for index in 0..4u8 {
        dib.extend_from_slice(&[index * 16, index * 16, index * 16, 0]);
    }
    dib.extend((0..64u8).map(|index| index % 4));
    dib
}

/// One invented file to place in the package.
#[derive(Debug, Clone)]
pub struct SyntheticFile {
    /// Destination path bytes, caller-invented.
    pub path: Vec<u8>,
    /// File contents, caller-invented.
    pub content: Vec<u8>,
}

impl SyntheticFile {
    /// A file with `path` and `content`.
    #[must_use]
    pub fn new(path: &[u8], content: Vec<u8>) -> Self {
        Self {
            path: path.to_vec(),
            content,
        }
    }
}

/// How to deform the synthetic package.
///
/// The flags are deliberately independent switches over one writer rather
/// than separate builders, so a test can combine any two deformations.
#[derive(Debug, Clone)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent deformation switches for one test writer"
)]
pub struct PackageOptions {
    /// Offset of the overlay within the image; must exceed `0x400`.
    pub overlay_offset: u32,
    /// Number of undocumented header bytes before the first stream.
    pub header_len: usize,
    /// The files to store.
    pub files: Vec<SyntheticFile>,
    /// Insert one padding byte after the stream with this chain index.
    pub pad_after_stream: Option<usize>,
    /// Corrupt the trailing checksum of the stream with this chain index.
    pub corrupt_crc_of_stream: Option<usize>,
    /// Truncate this many bytes off the end of the image.
    pub truncate_bytes: usize,
    /// Write a ZIP local file header signature into the overlay header.
    pub zip_variant: bool,
    /// Declare each file's inflated size as this multiple of the real one.
    pub declared_size_scale: u32,
    /// Write the script's offsets as image-absolute instead of
    /// overlay-relative.
    pub absolute_offsets: bool,
    /// Omit the script stream's file records entirely.
    pub omit_records: bool,
    /// Store a zero CRC-32 in every record, which the documented field table
    /// treats as "not checked" and which forces offset-based mapping.
    pub omit_record_checksums: bool,
}

impl Default for PackageOptions {
    fn default() -> Self {
        Self {
            overlay_offset: 0x1000,
            header_len: 157,
            files: Vec::new(),
            pad_after_stream: None,
            corrupt_crc_of_stream: None,
            truncate_bytes: 0,
            zip_variant: false,
            declared_size_scale: 1,
            absolute_offsets: false,
            omit_records: false,
            omit_record_checksums: false,
        }
    }
}

impl PackageOptions {
    /// Options carrying `files`.
    #[must_use]
    pub fn with_files(files: Vec<SyntheticFile>) -> Self {
        Self {
            files,
            ..Self::default()
        }
    }
}

/// A built synthetic package and the facts a test needs about it.
#[derive(Debug, Clone)]
pub struct Package {
    /// The whole image.
    pub image: Vec<u8>,
    /// Offset of the overlay.
    pub overlay_offset: u64,
    /// Offset of the first stream.
    pub first_stream_offset: u64,
    /// Number of streams written (DIB, script, then one per file).
    pub stream_count: usize,
    /// Each file's overlay-relative compressed start offset.
    pub file_offsets: Vec<u32>,
}

fn deflate_with_crc(plain: &[u8], level: u8, good_crc: bool) -> Vec<u8> {
    let mut bytes = miniz_oxide::deflate::compress_to_vec(plain, level);
    let crc = if good_crc {
        crc32(plain)
    } else {
        crc32(plain) ^ 0x5555_5555
    };
    bytes.extend_from_slice(&crc.to_le_bytes());
    bytes
}

fn pe_stub(overlay_offset: u32) -> Vec<u8> {
    assert!(
        overlay_offset > SECTION_RAW_AT,
        "overlay must follow the section"
    );
    let mut image = vec![0u8; overlay_offset as usize];
    image[0] = b'M';
    image[1] = b'Z';
    image[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    image[0x80..0x84].copy_from_slice(&crate::overlay::PE_SIGNATURE.to_le_bytes());
    image[0x86..0x88].copy_from_slice(&1u16.to_le_bytes()); // NumberOfSections
    image[0x94..0x96].copy_from_slice(&224u16.to_le_bytes()); // SizeOfOptionalHeader
    let section = 0x80 + 4 + 20 + 224;
    image[section + 16..section + 20]
        .copy_from_slice(&(overlay_offset - SECTION_RAW_AT).to_le_bytes());
    image[section + 20..section + 24].copy_from_slice(&SECTION_RAW_AT.to_le_bytes());
    image
}

/// Builds a synthetic Wise-shaped package.
///
/// # Panics
///
/// Panics when the options are self-contradictory, for example an overlay
/// offset inside the stub's own section.
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "one linear writer whose stages must stay in one place"
)]
pub fn build_package(options: &PackageOptions) -> Package {
    let dib = synthetic_dib();
    let dib_stream = deflate_with_crc(&dib, 6, options.corrupt_crc_of_stream != Some(0));

    // The file streams are compressed first so their lengths are known; the
    // script's records are then written with the offsets those lengths imply.
    let file_streams: Vec<Vec<u8>> = options
        .files
        .iter()
        .enumerate()
        .map(|(index, file)| {
            let good = options.corrupt_crc_of_stream != Some(index + 2);
            deflate_with_crc(&file.content, 6, good)
        })
        .collect();

    // Level 0 keeps the script stream's compressed length a function of its
    // uncompressed length alone, so one pass suffices.
    let record_bytes = |offsets: &[u32]| -> Vec<u8> {
        let mut script = vec![0x5au8; 16];
        if options.omit_records {
            return script;
        }
        for (index, file) in options.files.iter().enumerate() {
            let start = offsets[index];
            let stream_len =
                u32::try_from(file_streams[index].len()).expect("synthetic streams stay small");
            let end = start + stream_len - 4;
            let declared = u32::try_from(file.content.len())
                .expect("synthetic files stay small")
                .saturating_mul(options.declared_size_scale);
            script.extend_from_slice(&encode_file_record(
                &file.path,
                start,
                end,
                declared,
                if options.omit_record_checksums {
                    0
                } else {
                    crc32(&file.content)
                },
            ));
            script.extend_from_slice(&[0x11u8; 5]);
        }
        script
    };

    let placeholder: Vec<u32> = (0..options.files.len())
        .map(|index| u32::try_from(index).expect("synthetic file counts stay small") + 1)
        .collect();
    let script_len = record_bytes(&placeholder).len();
    let script_stream_len =
        miniz_oxide::deflate::compress_to_vec(&vec![0u8; script_len], 0).len() + 4;

    let base = if options.absolute_offsets {
        u64::from(options.overlay_offset)
    } else {
        0
    };
    let mut cursor = options.header_len as u64 + dib_stream.len() as u64 + script_stream_len as u64;
    if options.pad_after_stream == Some(0) {
        cursor += 1;
    }
    if options.pad_after_stream == Some(1) {
        cursor += 1;
    }
    let mut offsets = Vec::with_capacity(options.files.len());
    for (index, stream) in file_streams.iter().enumerate() {
        offsets.push(u32::try_from(base + cursor).expect("synthetic offsets stay small"));
        cursor += stream.len() as u64;
        if options.pad_after_stream == Some(index + 2) {
            cursor += 1;
        }
    }

    let script = record_bytes(&offsets);
    assert_eq!(script.len(), script_len, "script length must be stable");
    let script_stream = deflate_with_crc(&script, 0, options.corrupt_crc_of_stream != Some(1));
    assert_eq!(script_stream.len(), script_stream_len);

    let mut image = pe_stub(options.overlay_offset);
    let overlay_offset = image.len() as u64;
    let mut header = vec![HEADER_FILLER; options.header_len];
    if options.zip_variant {
        assert!(options.header_len >= 12, "zip signature needs header room");
        header[8..12].copy_from_slice(&crate::header::ZIP_LOCAL_FILE_SIGNATURE);
    }
    image.extend_from_slice(&header);
    let first_stream_offset = image.len() as u64;

    image.extend_from_slice(&dib_stream);
    if options.pad_after_stream == Some(0) {
        image.push(0x00);
    }
    image.extend_from_slice(&script_stream);
    if options.pad_after_stream == Some(1) {
        image.push(0x00);
    }
    for (index, stream) in file_streams.iter().enumerate() {
        image.extend_from_slice(stream);
        if options.pad_after_stream == Some(index + 2) {
            image.push(0x00);
        }
    }
    let keep = image.len().saturating_sub(options.truncate_bytes);
    image.truncate(keep);

    Package {
        image,
        overlay_offset,
        first_stream_offset,
        stream_count: options.files.len() + 2,
        file_offsets: offsets,
    }
}

#[cfg(test)]
mod tests {
    use super::{PackageOptions, SyntheticFile, build_package, synthetic_dib};
    use alloc::vec;

    #[test]
    fn builds_a_package_whose_parts_line_up() {
        let package = build_package(&PackageOptions::with_files(vec![SyntheticFile::new(
            b"dir\\one.dat",
            vec![1u8; 256],
        )]));
        assert_eq!(package.stream_count, 3);
        assert_eq!(package.overlay_offset, 0x1000);
        assert_eq!(package.first_stream_offset, 0x1000 + 157);
        assert_eq!(package.file_offsets.len(), 1);
    }

    #[test]
    fn the_synthetic_dib_leads_with_a_bitmapinfoheader() {
        let dib = synthetic_dib();
        assert_eq!(&dib[..4], &40u32.to_le_bytes());
        assert!(dib.len() > 40);
    }
}
