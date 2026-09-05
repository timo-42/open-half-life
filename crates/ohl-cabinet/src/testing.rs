//! Synthetic, independently authored cabinet writer for tests and fuzzing.
//!
//! It writes cabinets whose structures this crate already parses, from data
//! the caller supplies. It contains no proprietary data and must never be
//! enabled in a production build.

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use ohl_cabinet_format::testing::{
    HeaderBuilder, SynthFile, VERSION_WORD_2003, VERSION_WORD_V5, VERSION_WORD_V6,
};
use ohl_cabinet_format::{
    CAB_SIGNATURE, COMMON_HEADER_SIZE, FILE_COMPRESSED, FILE_OBFUSCATED, FILE_SPLIT, Layout,
    Version,
};

use crate::error::VolumeError;
use crate::obfuscation::obfuscate;
use crate::volume::{VolumeHeader, VolumeSource};

/// One file to write into a synthetic cabinet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynthEntry {
    /// The file's name.
    pub name: String,
    /// Its directory table index.
    pub directory_index: u32,
    /// Its expanded contents.
    pub data: Vec<u8>,
    /// Store the bytes as length-prefixed raw DEFLATE chunks.
    pub compressed: bool,
    /// Apply the obfuscation keystream to the stored bytes.
    pub obfuscated: bool,
    /// Volume number (1-based) holding the first stored byte.
    pub volume: u16,
    /// Split the stored stream after this many bytes, continuing in the next
    /// volume.
    pub split_after: Option<usize>,
    /// Raw `link_previous` field.
    pub link_previous: u32,
    /// Raw `link_next` field.
    pub link_next: u32,
    /// Raw `link_flags` field.
    pub link_flags: u8,
}

impl SynthEntry {
    /// A stored-as-is entry in volume 1.
    #[must_use]
    pub fn new(name: &str, data: &[u8]) -> Self {
        Self {
            name: name.to_string(),
            directory_index: 0,
            data: data.to_vec(),
            compressed: false,
            obfuscated: false,
            volume: 1,
            split_after: None,
            link_previous: 0,
            link_next: 0,
            link_flags: 0,
        }
    }

    /// The same entry, stored compressed.
    #[must_use]
    pub fn compressed(mut self) -> Self {
        self.compressed = true;
        self
    }

    /// The same entry, stored obfuscated.
    #[must_use]
    pub fn obfuscated(mut self) -> Self {
        self.obfuscated = true;
        self
    }

    /// The same entry, split after `bytes` stored bytes into the next volume.
    #[must_use]
    pub fn split_after(mut self, bytes: usize) -> Self {
        self.split_after = Some(bytes);
        self
    }

    /// The same entry, placed in `volume`.
    #[must_use]
    pub fn in_volume(mut self, volume: u16) -> Self {
        self.volume = volume;
        self
    }

    /// The same entry, carrying explicit split-link fields.
    #[must_use]
    pub fn with_link(mut self, previous: u32, next: u32, flags: u8) -> Self {
        self.link_previous = previous;
        self.link_next = next;
        self.link_flags = flags;
        self
    }
}

/// A built cabinet: one header buffer and one or more volume buffers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynthCabinet {
    /// The `.hdr` bytes.
    pub header: Vec<u8>,
    /// Volume bytes; index 0 is volume 1.
    pub volumes: Vec<Vec<u8>>,
}

impl SynthCabinet {
    /// Bytes of volume `volume` (1-based), if present.
    #[must_use]
    pub fn volume(&self, volume: u16) -> Option<&[u8]> {
        let index = usize::from(volume).checked_sub(1)?;
        self.volumes.get(index).map(Vec::as_slice)
    }
}

impl VolumeSource for SynthCabinet {
    fn read_at(&mut self, volume: u16, offset: u64, buf: &mut [u8]) -> Result<usize, VolumeError> {
        let Some(bytes) = self.volume(volume) else {
            return Err(VolumeError);
        };
        let Ok(offset) = usize::try_from(offset) else {
            return Ok(0);
        };
        if offset >= bytes.len() {
            return Ok(0);
        }
        let available = bytes.len() - offset;
        let count = available.min(buf.len());
        buf[..count].copy_from_slice(&bytes[offset..offset + count]);
        Ok(count)
    }
}

/// Builds a synthetic cabinet.
#[derive(Debug, Clone)]
pub struct CabinetBuilder {
    version_word: u32,
    directories: Vec<String>,
    entries: Vec<SynthEntry>,
    chunk_bytes: usize,
}

impl CabinetBuilder {
    /// A builder writing the given raw version word.
    #[must_use]
    pub fn new(version_word: u32) -> Self {
        Self {
            version_word,
            directories: Vec::new(),
            entries: Vec::new(),
            chunk_bytes: 8 * 1024,
        }
    }

    /// An InstallShield 5 style cabinet.
    #[must_use]
    pub fn v5() -> Self {
        Self::new(VERSION_WORD_V5)
    }

    /// An InstallShield 6 style cabinet.
    #[must_use]
    pub fn v6() -> Self {
        Self::new(VERSION_WORD_V6)
    }

    /// An InstallShield 2003 style cabinet, with UTF-16LE header strings.
    #[must_use]
    pub fn is2003() -> Self {
        Self::new(VERSION_WORD_2003)
    }

    /// Sets the expanded size of each compressed chunk.
    #[must_use]
    pub fn chunk_bytes(mut self, bytes: usize) -> Self {
        self.chunk_bytes = bytes.max(1);
        self
    }

    /// Adds a directory name.
    #[must_use]
    pub fn directory(mut self, name: &str) -> Self {
        self.directories.push(name.to_string());
        self
    }

    /// Adds a file.
    #[must_use]
    pub fn entry(mut self, entry: SynthEntry) -> Self {
        self.entries.push(entry);
        self
    }

    fn version(&self) -> Version {
        Version::decode(self.version_word)
    }

    fn stored_stream(&self, entry: &SynthEntry) -> Vec<u8> {
        let mut stored = Vec::new();
        if entry.compressed {
            for chunk in entry.data.chunks(self.chunk_bytes) {
                let deflated = miniz_oxide::deflate::compress_to_vec(chunk, 6);
                let length = u16::try_from(deflated.len()).expect("chunk fits in u16");
                stored.extend_from_slice(&length.to_le_bytes());
                stored.extend_from_slice(&deflated);
            }
        } else {
            stored.extend_from_slice(&entry.data);
        }
        if entry.obfuscated {
            let mut seed = 0;
            obfuscate(&mut stored, &mut seed);
        }
        stored
    }

    /// Serialises the cabinet.
    #[must_use]
    #[allow(clippy::too_many_lines)]
    pub fn build(&self) -> SynthCabinet {
        let version = self.version();
        let layout = version.layout();
        let prologue = COMMON_HEADER_SIZE + VolumeHeader::encoded_size(layout);

        let mut volumes: Vec<Vec<u8>> = Vec::new();
        // (file index, volume index, offset, stored length, expanded length)
        let mut placements: Vec<(u32, usize, u64, u64, u64)> = Vec::new();
        let mut files: Vec<SynthFile> = Vec::new();

        let ensure = |volumes: &mut Vec<Vec<u8>>, index: usize| {
            while volumes.len() <= index {
                volumes.push(vec![0u8; prologue]);
            }
        };

        for (index, entry) in self.entries.iter().enumerate() {
            let file_index = u32::try_from(index).expect("bounded by test");
            let stored = self.stored_stream(entry);
            let first_volume = usize::from(entry.volume).saturating_sub(1);
            ensure(&mut volumes, first_volume);

            let mut flags = 0u16;
            if entry.compressed {
                flags |= FILE_COMPRESSED;
            }
            if entry.obfuscated {
                flags |= FILE_OBFUSCATED;
            }

            let data_offset;
            match entry.split_after {
                Some(cut) if cut < stored.len() => {
                    flags |= FILE_SPLIT;
                    let offset = u64::try_from(volumes[first_volume].len()).expect("bounded");
                    volumes[first_volume].extend_from_slice(&stored[..cut]);
                    placements.push((
                        file_index,
                        first_volume,
                        offset,
                        u64::try_from(cut).expect("bounded"),
                        u64::try_from(cut).expect("bounded"),
                    ));
                    data_offset = offset;

                    ensure(&mut volumes, first_volume + 1);
                    let tail = stored.len() - cut;
                    let offset = u64::try_from(volumes[first_volume + 1].len()).expect("bounded");
                    volumes[first_volume + 1].extend_from_slice(&stored[cut..]);
                    placements.push((
                        file_index,
                        first_volume + 1,
                        offset,
                        u64::try_from(tail).expect("bounded"),
                        u64::try_from(tail).expect("bounded"),
                    ));
                }
                _ => {
                    let offset = u64::try_from(volumes[first_volume].len()).expect("bounded");
                    volumes[first_volume].extend_from_slice(&stored);
                    placements.push((
                        file_index,
                        first_volume,
                        offset,
                        u64::try_from(stored.len()).expect("bounded"),
                        u64::try_from(entry.data.len()).expect("bounded"),
                    ));
                    data_offset = offset;
                }
            }

            files.push(SynthFile {
                name: entry.name.clone(),
                directory_index: entry.directory_index,
                flags,
                expanded_size: u64::try_from(entry.data.len()).expect("bounded"),
                compressed_size: u64::try_from(stored.len()).expect("bounded"),
                data_offset,
                md5: md5_of(&entry.data),
                volume: entry.volume,
                link_previous: entry.link_previous,
                link_next: entry.link_next,
                link_flags: entry.link_flags,
            });
        }

        if volumes.is_empty() {
            volumes.push(vec![0u8; prologue]);
        }

        // Write each volume's common header and volume header.
        for (volume_index, bytes) in volumes.iter_mut().enumerate() {
            let mut common = Vec::with_capacity(COMMON_HEADER_SIZE);
            common.extend_from_slice(&CAB_SIGNATURE.to_le_bytes());
            common.extend_from_slice(&self.version_word.to_le_bytes());
            common.extend_from_slice(&0u32.to_le_bytes());
            common.extend_from_slice(&0u32.to_le_bytes());
            common.extend_from_slice(&0u32.to_le_bytes());
            bytes[..COMMON_HEADER_SIZE].copy_from_slice(&common);

            let here: Vec<&(u32, usize, u64, u64, u64)> = placements
                .iter()
                .filter(|placement| placement.1 == volume_index)
                .collect();
            let mut volume_header = VolumeHeader {
                data_offset: u64::try_from(prologue).expect("bounded"),
                ..VolumeHeader::default()
            };
            if let (Some(first), Some(last)) = (here.first(), here.last()) {
                volume_header.first_file_index = first.0;
                volume_header.first_file_offset = first.2;
                volume_header.first_file_size_compressed = first.3;
                volume_header.first_file_size_expanded = first.4;
                volume_header.last_file_index = last.0;
                volume_header.last_file_offset = last.2;
                volume_header.last_file_size_compressed = last.3;
                volume_header.last_file_size_expanded = last.4;
            }
            let encoded = volume_header.encode(layout);
            bytes[COMMON_HEADER_SIZE..COMMON_HEADER_SIZE + encoded.len()].copy_from_slice(&encoded);
        }

        let mut builder = HeaderBuilder::new(self.version_word);
        for directory in &self.directories {
            builder = builder.directory(directory);
        }
        for file in files {
            builder = builder.file(file);
        }
        debug_assert!(matches!(layout, Layout::V5 | Layout::V6));

        SynthCabinet {
            header: builder.build(),
            volumes,
        }
    }
}

/// MD5 of `data`, or all zeroes when the `md5` feature is disabled.
#[must_use]
pub fn md5_of(data: &[u8]) -> [u8; 16] {
    #[cfg(feature = "md5")]
    {
        let digest = <md5::Md5 as md5::Digest>::digest(data);
        let mut output = [0u8; 16];
        output.copy_from_slice(&digest);
        output
    }
    #[cfg(not(feature = "md5"))]
    {
        let _ = data;
        [0u8; 16]
    }
}
