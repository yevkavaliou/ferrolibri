//! Reads the table of contents of a ZIP file and nothing else (D14): no entry is ever unpacked.
//!
//! Only the "end of central directory" record and the central directory are read, so a
//! multi-gigabyte archive costs a few reads. ZIP64 and multi-disk archives are refused: no book
//! within the download limit needs them.

use std::io::{self, Read, Seek, SeekFrom};

const END_RECORD: [u8; 4] = *b"PK\x05\x06";
const END_RECORD_LEN: usize = 22;
const ENTRY_RECORD: [u8; 4] = *b"PK\x01\x02";
const ENTRY_RECORD_LEN: usize = 46;
/// The comment after the end record is at most this long, so the record is in the last bytes.
const MAX_COMMENT: u64 = u16::MAX as u64;
/// A central directory larger than this is not read (an EPUB of 20 000 files needs about 1 MiB).
const MAX_DIRECTORY_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug)]
pub struct Entry {
    /// The raw bytes of the name; names are only compared, never used as paths.
    pub name: Vec<u8>,
    /// What the directory claims the entry unpacks to.
    pub size: u64,
}

impl Entry {
    pub fn is_directory(&self) -> bool {
        self.name.ends_with(b"/")
    }

    pub fn has_extension(&self, extension: &str) -> bool {
        let name = self.name.as_slice();
        let (Some(dot), true) = (name.iter().rposition(|b| *b == b'.'), !self.is_directory())
        else {
            return false;
        };
        name[dot + 1..].eq_ignore_ascii_case(extension.as_bytes())
    }
}

#[derive(Debug)]
pub enum ZipError {
    /// There is no end-of-directory record: this is not a ZIP file.
    NotZip,
    /// The directory is cut short or contradicts itself.
    Damaged,
    /// ZIP64 or several parts: not needed for a book within the download limit.
    Unsupported,
    Io(io::Error),
}

impl From<io::Error> for ZipError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub fn read_directory(file: &mut (impl Read + Seek)) -> Result<Vec<Entry>, ZipError> {
    let length = file.seek(SeekFrom::End(0))?;
    if length < END_RECORD_LEN as u64 {
        return Err(ZipError::NotZip);
    }
    let tail_length = length.min(END_RECORD_LEN as u64 + MAX_COMMENT);
    let tail_start = length - tail_length;
    file.seek(SeekFrom::Start(tail_start))?;
    let mut tail = vec![0u8; tail_length as usize];
    file.read_exact(&mut tail)?;

    let position = (0..=tail.len() - END_RECORD_LEN)
        .rev()
        .find(|&at| tail[at..at + 4] == END_RECORD)
        .ok_or(ZipError::NotZip)?;
    let record = &tail[position..position + END_RECORD_LEN];
    let on_this_disk = u16::from_le_bytes([record[8], record[9]]);
    let entries = u16::from_le_bytes([record[10], record[11]]);
    let size = u32::from_le_bytes([record[12], record[13], record[14], record[15]]);
    let offset = u32::from_le_bytes([record[16], record[17], record[18], record[19]]);
    if u16::from_le_bytes([record[4], record[5]]) != 0
        || u16::from_le_bytes([record[6], record[7]]) != 0
        || on_this_disk != entries
    {
        return Err(ZipError::Unsupported);
    }
    if entries == u16::MAX || size == u32::MAX || offset == u32::MAX {
        return Err(ZipError::Unsupported);
    }
    let end_of_directory = tail_start + position as u64;
    if u64::from(offset) + u64::from(size) > end_of_directory
        || u64::from(size) > MAX_DIRECTORY_BYTES
    {
        return Err(ZipError::Damaged);
    }

    file.seek(SeekFrom::Start(u64::from(offset)))?;
    let mut directory = vec![0u8; size as usize];
    file.read_exact(&mut directory)?;

    let mut found = Vec::with_capacity(usize::from(entries));
    let mut rest = directory.as_slice();
    while !rest.is_empty() {
        if rest.len() < ENTRY_RECORD_LEN || rest[..4] != ENTRY_RECORD {
            return Err(ZipError::Damaged);
        }
        let uncompressed = u32::from_le_bytes([rest[24], rest[25], rest[26], rest[27]]);
        let name_length = usize::from(u16::from_le_bytes([rest[28], rest[29]]));
        let extra_length = usize::from(u16::from_le_bytes([rest[30], rest[31]]));
        let comment_length = usize::from(u16::from_le_bytes([rest[32], rest[33]]));
        let total = ENTRY_RECORD_LEN + name_length + extra_length + comment_length;
        if rest.len() < total {
            return Err(ZipError::Damaged);
        }
        if uncompressed == u32::MAX {
            return Err(ZipError::Unsupported);
        }
        found.push(Entry {
            name: rest[ENTRY_RECORD_LEN..ENTRY_RECORD_LEN + name_length].to_vec(),
            size: u64::from(uncompressed),
        });
        rest = &rest[total..];
    }
    if found.len() != usize::from(entries) {
        return Err(ZipError::Damaged);
    }
    Ok(found)
}
