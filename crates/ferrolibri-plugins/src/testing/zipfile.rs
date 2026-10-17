//! A minimal ZIP writer for fixtures: one stored (uncompressed) entry, nothing else.

/// A valid `.zip` with a single file `name` holding `data`, stored without compression.
pub fn single_file_zip(name: &str, data: &[u8]) -> Vec<u8> {
    let crc = crc32(data);
    let size = data.len() as u32;
    let name = name.as_bytes();
    let mut out = Vec::new();

    // Local file header.
    out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
    out.extend_from_slice(&20u16.to_le_bytes()); // version needed
    out.extend_from_slice(&0x0800u16.to_le_bytes()); // flags: UTF-8 name
    out.extend_from_slice(&0u16.to_le_bytes()); // method: stored
    out.extend_from_slice(&0u16.to_le_bytes()); // time
    out.extend_from_slice(&0x0021u16.to_le_bytes()); // date: 1980-01-01
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&(name.len() as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // extra length
    out.extend_from_slice(name);
    out.extend_from_slice(data);

    // Central directory.
    let directory_offset = out.len() as u32;
    out.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
    out.extend_from_slice(&20u16.to_le_bytes()); // version made by
    out.extend_from_slice(&20u16.to_le_bytes()); // version needed
    out.extend_from_slice(&0x0800u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0x0021u16.to_le_bytes());
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&size.to_le_bytes());
    out.extend_from_slice(&(name.len() as u16).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // extra length
    out.extend_from_slice(&0u16.to_le_bytes()); // comment length
    out.extend_from_slice(&0u16.to_le_bytes()); // disk number
    out.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
    out.extend_from_slice(&0u32.to_le_bytes()); // external attributes
    out.extend_from_slice(&0u32.to_le_bytes()); // offset of the local header
    out.extend_from_slice(name);
    let directory_size = out.len() as u32 - directory_offset;

    // End of central directory.
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&directory_size.to_le_bytes());
    out.extend_from_slice(&directory_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn the_archive_has_the_expected_structure() {
        let zip = single_file_zip("book.fb2", b"<FictionBook/>");
        assert_eq!(&zip[..4], b"PK\x03\x04");
        // End of central directory: the last 22 bytes, one entry.
        let eocd = &zip[zip.len() - 22..];
        assert_eq!(&eocd[..4], b"PK\x05\x06");
        assert_eq!(u16::from_le_bytes([eocd[10], eocd[11]]), 1);
        let offset = u32::from_le_bytes([eocd[16], eocd[17], eocd[18], eocd[19]]) as usize;
        assert_eq!(&zip[offset..offset + 4], b"PK\x01\x02");
        assert!(zip.windows(8).any(|w| w == b"book.fb2"));
    }
}
