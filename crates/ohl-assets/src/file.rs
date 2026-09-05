//! A handle to one resolved asset: either a whole loose file or a bounded
//! byte range inside a PAK archive.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};

/// An open asset, readable and seekable regardless of whether it came from
/// a loose file or a PAK archive member.
pub struct AssetFile {
    inner: Inner,
}

enum Inner {
    Loose(File),
    PakEntry(PakSlice),
}

/// A bounded, seekable view over one PAK entry's byte range inside its
/// archive file. Every read is clamped to `[0, len)`; nothing here ever
/// reads outside the entry's declared range, no matter what `seek` is asked
/// for.
struct PakSlice {
    file: File,
    base_offset: u64,
    len: u64,
    pos: u64,
}

impl AssetFile {
    pub(crate) fn loose(file: File) -> Self {
        Self {
            inner: Inner::Loose(file),
        }
    }

    pub(crate) fn pak_entry(file: File, base_offset: u64, len: u64) -> Self {
        Self {
            inner: Inner::PakEntry(PakSlice {
                file,
                base_offset,
                len,
                pos: 0,
            }),
        }
    }
}

impl Read for AssetFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match &mut self.inner {
            Inner::Loose(file) => file.read(buf),
            Inner::PakEntry(slice) => slice.read(buf),
        }
    }
}

impl Seek for AssetFile {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        match &mut self.inner {
            Inner::Loose(file) => file.seek(pos),
            Inner::PakEntry(slice) => slice.seek(pos),
        }
    }
}

impl Read for PakSlice {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let remaining = self.len.saturating_sub(self.pos);
        if remaining == 0 || buf.is_empty() {
            return Ok(0);
        }
        let capacity = u64::try_from(buf.len()).unwrap_or(u64::MAX);
        let to_read = remaining.min(capacity);
        // `to_read <= buf.len()`, so this cast never truncates.
        #[allow(clippy::cast_possible_truncation)]
        let to_read_usize = to_read as usize;

        self.file
            .seek(SeekFrom::Start(self.base_offset + self.pos))?;
        let read = self.file.read(&mut buf[..to_read_usize])?;
        self.pos = self.pos.saturating_add(read as u64);
        Ok(read)
    }
}

impl Seek for PakSlice {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::End(offset) => i128::from(self.len) + i128::from(offset),
            SeekFrom::Current(offset) => i128::from(self.pos) + i128::from(offset),
        };
        if target < 0 || target > i128::from(self.len) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek target outside the PAK entry's byte range",
            ));
        }
        // Bounded above by `self.len`, which is a `u64`, so this never
        // truncates.
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let position = target as u64;
        self.pos = position;
        Ok(self.pos)
    }
}

#[cfg(test)]
mod tests {
    use super::AssetFile;
    use std::io::{Read, Seek, SeekFrom, Write};
    use tempfile::NamedTempFile;

    fn archive_with(prefix: &[u8], body: &[u8], suffix: &[u8]) -> std::fs::File {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(prefix).unwrap();
        f.write_all(body).unwrap();
        f.write_all(suffix).unwrap();
        f.reopen().unwrap()
    }

    #[test]
    fn pak_entry_reads_only_its_own_range() {
        let file = archive_with(b"HEADERBYTES", b"payload-bytes-here", b"TRAILINGJUNK");
        let mut asset = AssetFile::pak_entry(file, 11, 18);
        let mut out = Vec::new();
        asset.read_to_end(&mut out).unwrap();
        assert_eq!(out, b"payload-bytes-here");
    }

    #[test]
    fn pak_entry_seek_is_bounded() {
        let file = archive_with(b"HDR", b"0123456789", b"END");
        let mut asset = AssetFile::pak_entry(file, 3, 10);
        assert!(asset.seek(SeekFrom::Start(11)).is_err());
        assert_eq!(asset.seek(SeekFrom::Start(10)).unwrap(), 10);
        let mut out = [0u8; 4];
        assert_eq!(asset.read(&mut out).unwrap(), 0);

        asset.seek(SeekFrom::Start(2)).unwrap();
        let mut out = [0u8; 4];
        assert_eq!(asset.read(&mut out).unwrap(), 4);
        assert_eq!(&out, b"2345");
    }

    #[test]
    fn pak_entry_seek_from_end_and_current() {
        let file = archive_with(b"", b"abcdefgh", b"");
        let mut asset = AssetFile::pak_entry(file, 0, 8);
        assert_eq!(asset.seek(SeekFrom::End(-2)).unwrap(), 6);
        let mut out = [0u8; 2];
        asset.read_exact(&mut out).unwrap();
        assert_eq!(&out, b"gh");
        assert!(asset.seek(SeekFrom::Current(1)).is_err());
    }
}
