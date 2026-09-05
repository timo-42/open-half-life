//! Fuzz target: extraction over an arbitrary header and arbitrary volume
//! bytes must never panic, hang, or allocate past the limits.
#![no_main]

use libfuzzer_sys::fuzz_target;
use ohl_cabinet::{CabinetReader, Limits, VolumeError, VolumeSource};
use ohl_cabinet_format::{CabinetHeader, Limits as FormatLimits};

struct FuzzSource<'a> {
    bytes: &'a [u8],
}

impl VolumeSource for FuzzSource<'_> {
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

fuzz_target!(|data: &[u8]| {
    // The first two bytes choose where the header ends and the volume bytes
    // begin.
    if data.len() < 2 {
        return;
    }
    let rest = &data[2..];
    let split = usize::from(u16::from_le_bytes([data[0], data[1]])).min(rest.len());
    let (header_bytes, volume_bytes) = rest.split_at(split);

    let format_limits = FormatLimits {
        max_header_bytes: 1 << 22,
        max_files: 1024,
        max_directories: 1024,
        max_file_groups: 256,
        max_components: 256,
        max_name_bytes: 1024,
        max_volumes: 8,
    };
    let Ok(header) = CabinetHeader::parse(header_bytes, &format_limits) else {
        return;
    };

    let limits = Limits {
        max_expanded_bytes_per_file: 1 << 20,
        max_total_expanded_bytes: 1 << 22,
        max_volumes: 4,
        max_volume_hops: 8,
        max_link_steps: 64,
        max_chunk_bytes: 1 << 16,
    };
    let mut source = FuzzSource {
        bytes: volume_bytes,
    };
    let mut reader = CabinetReader::new(&header, limits);
    for index in 0..header.file_count().min(32) {
        let _ = reader.extract_to_vec(index, &mut source);
    }
});
