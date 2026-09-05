//! Locating the overlay of a PE image from its section table.
//!
//! Written from the public PE/COFF specification (Microsoft, "PE Format",
//! see `docs/FORMAT_SOURCES.md`): the DOS header's `e_lfanew` at offset
//! `0x3C` points at the `PE\0\0` signature, which is followed by the 20-byte
//! COFF file header (`NumberOfSections` at `+2`, `SizeOfOptionalHeader` at
//! `+16`) and then the section table, whose 40-byte entries carry
//! `SizeOfRawData` at `+16` and `PointerToRawData` at `+20`.
//!
//! The overlay is everything after the highest `PointerToRawData +
//! SizeOfRawData`, which is where a Wise package's own data begins.

use ohl_core::CheckedArithmetic as _;

use crate::error::{Error, Limit};
use crate::limits::Limits;
use crate::source::ImageSource;

/// The `MZ` DOS executable signature.
pub const DOS_SIGNATURE: u16 = 0x5A4D;
/// The `PE\0\0` signature.
pub const PE_SIGNATURE: u32 = 0x0000_4550;

/// Offset of `e_lfanew` within the DOS header.
const E_LFANEW_AT: u64 = 0x3C;
/// Smallest `e_lfanew` that can follow a whole DOS header.
const MIN_E_LFANEW: u32 = 0x40;
/// Encoded size of the COFF file header, signature excluded.
const COFF_HEADER_SIZE: u64 = 20;
/// Encoded size of one section header.
const SECTION_HEADER_SIZE: u64 = 40;

/// Where a PE image's appended data starts, and how large it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Overlay {
    /// Offset of the first overlay byte within the image.
    pub offset: u64,
    /// Number of overlay bytes, that is `image_len - offset`.
    pub len: u64,
    /// Total image length.
    pub image_len: u64,
}

impl Overlay {
    /// The end of the overlay, which is the end of the image.
    #[must_use]
    pub const fn end(&self) -> u64 {
        self.image_len
    }

    /// Whether `offset` (absolute, within the image) lies inside the overlay.
    #[must_use]
    pub const fn contains(&self, offset: u64) -> bool {
        offset >= self.offset && offset < self.image_len
    }
}

fn read_exact<S: ImageSource>(source: &mut S, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
    let read = source.read_at(offset, buf)?;
    if read != buf.len() {
        return Err(Error::Truncated);
    }
    Ok(())
}

fn read_u16<S: ImageSource>(source: &mut S, offset: u64) -> Result<u16, Error> {
    let mut bytes = [0u8; 2];
    read_exact(source, offset, &mut bytes)?;
    Ok(u16::from_le_bytes(bytes))
}

fn read_u32<S: ImageSource>(source: &mut S, offset: u64) -> Result<u32, Error> {
    let mut bytes = [0u8; 4];
    read_exact(source, offset, &mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

/// Accepts a caller-supplied overlay offset, validating it against the image.
///
/// Callers who already know the stub's overlay offset (the published table of
/// Wise stub offsets lists several) can skip section-table parsing entirely.
pub fn overlay_at<S: ImageSource>(source: &mut S, offset: u64) -> Result<Overlay, Error> {
    let image_len = source.len()?;
    if offset >= image_len {
        return Err(Error::NoOverlay);
    }
    Ok(Overlay {
        offset,
        len: image_len.checked_sub_bounded(offset)?,
        image_len,
    })
}

/// Computes the overlay offset from the PE section table.
pub fn find_overlay<S: ImageSource>(source: &mut S, limits: &Limits) -> Result<Overlay, Error> {
    let image_len = source.len()?;
    if image_len < E_LFANEW_AT + 4 {
        return Err(Error::NotExecutable);
    }
    if read_u16(source, 0)? != DOS_SIGNATURE {
        return Err(Error::NotExecutable);
    }

    let e_lfanew = u64::from(read_u32(source, E_LFANEW_AT)?);
    if e_lfanew < u64::from(MIN_E_LFANEW) || e_lfanew > limits.max_pe_header_bytes {
        return Err(Error::LimitExceeded(Limit::PeHeaderBytes));
    }
    if e_lfanew >= image_len {
        return Err(Error::MalformedExecutable);
    }
    if read_u32(source, e_lfanew)? != PE_SIGNATURE {
        return Err(Error::MalformedExecutable);
    }

    let coff = e_lfanew.checked_add_bounded(4)?;
    let section_count = read_u16(source, coff.checked_add_bounded(2)?)?;
    if section_count == 0 {
        return Err(Error::MalformedExecutable);
    }
    if section_count > limits.max_sections {
        return Err(Error::LimitExceeded(Limit::Sections));
    }
    let optional_header_size = u64::from(read_u16(source, coff.checked_add_bounded(16)?)?);
    let table_start = coff
        .checked_add_bounded(COFF_HEADER_SIZE)?
        .checked_add_bounded(optional_header_size)?;
    let table_bytes = SECTION_HEADER_SIZE.checked_mul_bounded(u64::from(section_count))?;
    let table_end = table_start.checked_add_bounded(table_bytes)?;
    if table_end > limits.max_pe_header_bytes {
        return Err(Error::LimitExceeded(Limit::PeHeaderBytes));
    }
    if table_end > image_len {
        return Err(Error::Truncated);
    }

    let mut overlay_offset = table_end;
    for index in 0..u64::from(section_count) {
        let header =
            table_start.checked_add_bounded(index.checked_mul_bounded(SECTION_HEADER_SIZE)?)?;
        let size_of_raw_data = u64::from(read_u32(source, header.checked_add_bounded(16)?)?);
        let pointer_to_raw_data = u64::from(read_u32(source, header.checked_add_bounded(20)?)?);
        if size_of_raw_data == 0 || pointer_to_raw_data == 0 {
            continue;
        }
        let end = pointer_to_raw_data.checked_add_bounded(size_of_raw_data)?;
        if end > image_len {
            return Err(Error::MalformedExecutable);
        }
        overlay_offset = overlay_offset.max(end);
    }

    if overlay_offset >= image_len {
        return Err(Error::NoOverlay);
    }
    Ok(Overlay {
        offset: overlay_offset,
        len: image_len.checked_sub_bounded(overlay_offset)?,
        image_len,
    })
}

#[cfg(test)]
mod tests {
    use super::{find_overlay, overlay_at};
    use crate::error::{Error, Limit};
    use crate::limits::Limits;
    use crate::source::SliceSource;
    use alloc::vec;
    use alloc::vec::Vec;

    /// A minimal PE stub with one section whose raw data ends at
    /// `overlay_offset`, followed by `overlay` bytes.
    fn stub(overlay_offset: u32, overlay: &[u8]) -> Vec<u8> {
        let mut image = vec![0u8; overlay_offset as usize];
        image[0] = b'M';
        image[1] = b'Z';
        image[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        image[0x80..0x84].copy_from_slice(&0x0000_4550u32.to_le_bytes());
        // COFF: machine, section count = 1, ..., optional header size = 224.
        image[0x86..0x88].copy_from_slice(&1u16.to_le_bytes());
        image[0x94..0x96].copy_from_slice(&224u16.to_le_bytes());
        let section = 0x80 + 4 + 20 + 224;
        image[section + 16..section + 20].copy_from_slice(&(overlay_offset - 0x400).to_le_bytes());
        image[section + 20..section + 24].copy_from_slice(&0x400u32.to_le_bytes());
        image.extend_from_slice(overlay);
        image
    }

    #[test]
    fn finds_the_overlay_from_the_section_table() {
        let image = stub(0x1000, &[7u8; 64]);
        let mut source = SliceSource::new(&image);
        let overlay = find_overlay(&mut source, &Limits::DEFAULT).unwrap();
        assert_eq!(overlay.offset, 0x1000);
        assert_eq!(overlay.len, 64);
        assert_eq!(overlay.end(), 0x1000 + 64);
        assert!(overlay.contains(0x1000));
        assert!(!overlay.contains(0x1000 + 64));
    }

    #[test]
    fn rejects_a_non_executable() {
        let mut source = SliceSource::new(&[0u8; 512]);
        assert_eq!(
            find_overlay(&mut source, &Limits::DEFAULT),
            Err(Error::NotExecutable)
        );
    }

    #[test]
    fn rejects_an_image_without_trailing_data() {
        let image = stub(0x1000, &[]);
        let mut source = SliceSource::new(&image);
        assert_eq!(
            find_overlay(&mut source, &Limits::DEFAULT),
            Err(Error::NoOverlay)
        );
    }

    #[test]
    fn rejects_an_oversized_header_offset() {
        let mut image = stub(0x1000, &[1u8; 16]);
        image[0x3c..0x40].copy_from_slice(&0xffff_fff0u32.to_le_bytes());
        let mut source = SliceSource::new(&image);
        assert_eq!(
            find_overlay(&mut source, &Limits::DEFAULT),
            Err(Error::LimitExceeded(Limit::PeHeaderBytes))
        );
    }

    #[test]
    fn rejects_a_section_reaching_past_the_image() {
        let mut image = stub(0x1000, &[1u8; 16]);
        let section = 0x80 + 4 + 20 + 224;
        image[section + 16..section + 20].copy_from_slice(&0xffff_0000u32.to_le_bytes());
        let mut source = SliceSource::new(&image);
        assert_eq!(
            find_overlay(&mut source, &Limits::DEFAULT),
            Err(Error::MalformedExecutable)
        );
    }

    #[test]
    fn accepts_a_caller_supplied_offset() {
        let image = stub(0x1000, &[3u8; 8]);
        let mut source = SliceSource::new(&image);
        let overlay = overlay_at(&mut source, 0x1000).unwrap();
        assert_eq!(overlay.len, 8);
        assert_eq!(
            overlay_at(&mut source, image.len() as u64),
            Err(Error::NoOverlay)
        );
    }

    #[test]
    fn rejects_too_many_sections() {
        let mut image = stub(0x1000, &[1u8; 16]);
        image[0x86..0x88].copy_from_slice(&4096u16.to_le_bytes());
        let mut source = SliceSource::new(&image);
        assert_eq!(
            find_overlay(&mut source, &Limits::DEFAULT),
            Err(Error::LimitExceeded(Limit::Sections))
        );
    }
}
