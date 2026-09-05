//! Little-endian readers over already length-checked buffers.
//!
//! Each helper is called only after its caller has proved the buffer is long
//! enough, so the indexing here is total; the debug assertions restate that
//! contract for the test build.

pub(crate) fn le16(bytes: &[u8], at: usize) -> u16 {
    debug_assert!(at + 2 <= bytes.len());
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

pub(crate) fn le32(bytes: &[u8], at: usize) -> u32 {
    debug_assert!(at + 4 <= bytes.len());
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

#[cfg(test)]
mod tests {
    use super::{le16, le32};

    #[test]
    fn reads_little_endian_words() {
        let bytes = [0x13, 0x5d, 0x65, 0x8c, 0x3a, 0x01];
        assert_eq!(le16(&bytes, 0), 0x5d13);
        assert_eq!(le32(&bytes, 0), 0x8c65_5d13);
        assert_eq!(le16(&bytes, 4), 0x013a);
    }
}
