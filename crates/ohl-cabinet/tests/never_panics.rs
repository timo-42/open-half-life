//! Property test: no arbitrary byte string can make extraction panic.

use ohl_cabinet::testing::{CabinetBuilder, SynthCabinet, SynthEntry};
use ohl_cabinet::{CabinetReader, Limits, VolumeError, VolumeSource};
use ohl_cabinet_format::{CabinetHeader, Limits as FormatLimits};
use proptest::prelude::*;

/// A volume source backed by one arbitrary byte string for every volume.
struct ArbitrarySource {
    bytes: Vec<u8>,
}

impl VolumeSource for ArbitrarySource {
    fn read_at(&mut self, volume: u16, offset: u64, buf: &mut [u8]) -> Result<usize, VolumeError> {
        if volume > 4 {
            return Err(VolumeError);
        }
        let Ok(offset) = usize::try_from(offset) else {
            return Ok(0);
        };
        if offset >= self.bytes.len() {
            return Ok(0);
        }
        let count = (self.bytes.len() - offset).min(buf.len());
        buf[..count].copy_from_slice(&self.bytes[offset..offset + count]);
        Ok(count)
    }
}

fn limits() -> Limits {
    Limits {
        max_expanded_bytes_per_file: 1 << 20,
        max_total_expanded_bytes: 1 << 22,
        max_volumes: 8,
        max_volume_hops: 8,
        max_link_steps: 32,
        max_chunk_bytes: 4096,
    }
}

fn exercise(header_bytes: &[u8], volume_bytes: &[u8]) {
    let format_limits = FormatLimits {
        max_header_bytes: 1 << 20,
        ..FormatLimits::default()
    };
    let Ok(header) = CabinetHeader::parse(header_bytes, &format_limits) else {
        return;
    };
    let mut source = ArbitrarySource {
        bytes: volume_bytes.to_vec(),
    };
    let mut reader = CabinetReader::new(&header, limits());
    for index in 0..header.file_count().min(64) {
        let _ = reader.extract_to_vec(index, &mut source);
    }
}

fn sample_cabinet() -> SynthCabinet {
    CabinetBuilder::v6()
        .directory("d")
        .chunk_bytes(512)
        .entry(SynthEntry::new("plain", &[7u8; 900]))
        .entry(SynthEntry::new("packed", &[3u8; 4000]).compressed())
        .entry(SynthEntry::new("hidden", &[9u8; 600]).obfuscated())
        .entry(SynthEntry::new("split", &[1u8; 5000]).split_after(1200))
        .build()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn arbitrary_bytes_never_panic(
        header in proptest::collection::vec(any::<u8>(), 0..2048),
        volume in proptest::collection::vec(any::<u8>(), 0..2048),
    ) {
        exercise(&header, &volume);
    }

    #[test]
    fn corrupting_a_real_cabinet_never_panics(
        header_index in 0usize..4096,
        header_value in any::<u8>(),
        volume_index in 0usize..8192,
        volume_value in any::<u8>(),
    ) {
        let cabinet = sample_cabinet();
        let mut header = cabinet.header.clone();
        if header_index < header.len() {
            header[header_index] = header_value;
        }
        let mut volume = cabinet.volumes[0].clone();
        if volume_index < volume.len() {
            volume[volume_index] = volume_value;
        }
        exercise(&header, &volume);
    }

    #[test]
    fn truncating_a_real_cabinet_never_panics(
        header_len in 0usize..4096,
        volume_len in 0usize..8192,
    ) {
        let cabinet = sample_cabinet();
        let header = &cabinet.header[..header_len.min(cabinet.header.len())];
        let volume = &cabinet.volumes[0][..volume_len.min(cabinet.volumes[0].len())];
        exercise(header, volume);
    }
}
