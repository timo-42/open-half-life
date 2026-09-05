//! Bounds- and length-validated string table access.

use alloc::string::String;

use crate::error::{FormatError, Limit};

/// Decodes a NUL-terminated single-byte string, lossily.
///
/// `max_bytes` bounds the encoded length, terminator excluded.
pub fn decode_ascii(data: &[u8], max_bytes: usize) -> Result<String, FormatError> {
    let scan_len = data.len().min(max_bytes);
    let Some(end) = data[..scan_len].iter().position(|byte| *byte == 0) else {
        return Err(if data.len() > max_bytes {
            FormatError::LimitExceeded(Limit::NameBytes)
        } else {
            FormatError::InvalidString
        });
    };
    Ok(String::from_utf8_lossy(&data[..end]).into_owned())
}

/// Decodes a NUL-terminated UTF-16LE string, lossily.
///
/// `max_bytes` bounds the encoded length in bytes, terminator excluded. An
/// odd number of bytes before the end of the buffer is a truncated encoding.
pub fn decode_utf16le(data: &[u8], max_bytes: usize) -> Result<String, FormatError> {
    let scan_len = data.len().min(max_bytes);
    let mut units = alloc::vec::Vec::new();
    let mut index = 0usize;
    loop {
        if index + 2 > scan_len {
            return Err(if data.len() > max_bytes {
                FormatError::LimitExceeded(Limit::NameBytes)
            } else {
                FormatError::InvalidString
            });
        }
        let unit = u16::from_le_bytes([data[index], data[index + 1]]);
        if unit == 0 {
            break;
        }
        units.push(unit);
        index += 2;
    }

    Ok(char::decode_utf16(units)
        .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect())
}

/// Decodes either encoding, chosen by `unicode`.
pub fn decode(data: &[u8], unicode: bool, max_bytes: usize) -> Result<String, FormatError> {
    if unicode {
        decode_utf16le(data, max_bytes)
    } else {
        decode_ascii(data, max_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::{decode, decode_ascii, decode_utf16le};
    use crate::error::{FormatError, Limit};
    use alloc::vec::Vec;

    #[test]
    fn decodes_a_terminated_ascii_string() {
        assert_eq!(decode_ascii(b"setup\0rest", 64).unwrap(), "setup");
    }

    #[test]
    fn decodes_invalid_utf8_lossily() {
        assert_eq!(decode_ascii(b"a\xffb\0", 64).unwrap(), "a\u{fffd}b");
    }

    #[test]
    fn rejects_an_unterminated_ascii_string() {
        assert_eq!(decode_ascii(b"setup", 64), Err(FormatError::InvalidString));
    }

    #[test]
    fn enforces_the_name_length_limit() {
        let long = [b'a'; 64];
        assert_eq!(
            decode_ascii(&long, 8),
            Err(FormatError::LimitExceeded(Limit::NameBytes))
        );
    }

    #[test]
    fn decodes_a_terminated_utf16_string() {
        let mut bytes = Vec::new();
        for unit in "sétup".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        bytes.extend_from_slice(&0u16.to_le_bytes());
        assert_eq!(decode_utf16le(&bytes, 64).unwrap(), "sétup");
        assert_eq!(decode(&bytes, true, 64).unwrap(), "sétup");
    }

    #[test]
    fn replaces_an_unpaired_surrogate() {
        let bytes = [0x00, 0xd8, 0x41, 0x00, 0x00, 0x00];
        assert_eq!(decode_utf16le(&bytes, 64).unwrap(), "\u{fffd}A");
    }

    #[test]
    fn rejects_a_truncated_utf16_unit() {
        assert_eq!(decode_utf16le(&[0x41], 64), Err(FormatError::InvalidString));
    }
}
