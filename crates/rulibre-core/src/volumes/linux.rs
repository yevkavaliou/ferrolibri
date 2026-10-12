use super::{MountedVolume, VolumeReport, local_filesystem};
use std::{
    io,
    path::{Path, PathBuf},
};

fn unescape(bytes: &[u8]) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            let digits = bytes.get(i + 1..i + 4).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "truncated mountinfo escape")
            })?;
            if !digits.iter().all(|digit| matches!(digit, b'0'..=b'7')) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid mountinfo escape",
                ));
            }
            let value = (digits[0] - b'0') as u16 * 64
                + (digits[1] - b'0') as u16 * 8
                + (digits[2] - b'0') as u16;
            if value == 0 || value > 255 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid path byte",
                ));
            }
            output.push(value as u8);
            i += 4;
        } else {
            output.push(bytes[i]);
            i += 1;
        }
    }
    Ok(output)
}
fn native_path(bytes: &[u8]) -> io::Result<PathBuf> {
    let bytes = unescape(bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(std::ffi::OsString::from_vec(bytes).into())
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(bytes)
            .map(PathBuf::from)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }
}

#[cfg(any(target_os = "linux", test))]
fn decode_label(bytes: &[u8]) -> String {
    let mut decoded = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes.get(index..index + 2) == Some(b"\\x")
            && let Some(pair) = bytes.get(index + 2..index + 4)
            && let Ok(hex) = std::str::from_utf8(pair)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            decoded.push(byte);
            index += 4;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

pub(super) fn parse_mountinfo(bytes: &[u8]) -> VolumeReport {
    let mut report = VolumeReport::default();
    for line in bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let parsed = (|| -> io::Result<Option<MountedVolume>> {
            let fields = line
                .split(|byte| byte.is_ascii_whitespace())
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>();
            let separator = fields
                .iter()
                .position(|part| *part == b"-")
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "mountinfo separator missing")
                })?;
            if separator < 6 || fields.len() < separator + 4 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "incomplete mountinfo record",
                ));
            }
            let filesystem = std::str::from_utf8(fields[separator + 1])
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            // A subdirectory bind mount is not the root of reader storage.
            if fields[3] != b"/" || !local_filesystem(filesystem) {
                return Ok(None);
            }
            let source = native_path(fields[separator + 2])?;
            let mount_point = native_path(fields[4])?;
            // Validate Linux syntax even when this parser is tested on Windows.
            if unescape(fields[4])?.first() != Some(&b'/') {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "mount path is not absolute",
                ));
            }
            let read_only = fields[5]
                .split(|byte| *byte == b',')
                .any(|value| value == b"ro")
                || fields[separator + 3]
                    .split(|byte| *byte == b',')
                    .any(|value| value == b"ro");
            Ok(Some(MountedVolume {
                id: format!(
                    "linux:{}:{}",
                    String::from_utf8_lossy(fields[2]),
                    source.display()
                ),
                mount_point,
                label: None,
                filesystem: filesystem.into(),
                read_only,
            }))
        })();
        match parsed {
            Ok(Some(volume)) => report.volumes.push(volume),
            Ok(None) => {}
            Err(error) => report.warnings.push(crate::Error::io(
                "parse mounted filesystem",
                Path::new("/proc/self/mountinfo"),
                error,
            )),
        }
    }
    report
}

#[cfg(target_os = "linux")]
pub(super) fn enumerate() -> Result<VolumeReport, crate::Error> {
    use std::os::unix::ffi::OsStrExt;
    use std::{collections::BTreeMap, fs};
    let table = Path::new("/proc/self/mountinfo");
    let bytes =
        fs::read(table).map_err(|error| crate::Error::io("read mount table", table, error))?;
    let mut report = parse_mountinfo(&bytes);
    let mut identities: BTreeMap<PathBuf, (Option<String>, Option<String>)> = BTreeMap::new();
    for (directory, uuid) in [("/dev/disk/by-uuid", true), ("/dev/disk/by-label", false)] {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => {
                report.warnings.push(crate::Error::io(
                    "read filesystem identities",
                    Path::new(directory),
                    error,
                ));
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    report.warnings.push(crate::Error::io(
                        "read filesystem identity entry",
                        Path::new(directory),
                        error,
                    ));
                    continue;
                }
            };
            match fs::canonicalize(entry.path()) {
                Ok(device) => {
                    let value = entry.file_name().to_string_lossy().into_owned();
                    let identity = identities.entry(device).or_default();
                    if uuid {
                        identity.0 = Some(value);
                    } else {
                        identity.1 = Some(decode_label(entry.file_name().as_bytes()));
                    }
                }
                Err(error) => report.warnings.push(crate::Error::io(
                    "resolve filesystem identity",
                    &entry.path(),
                    error,
                )),
            }
        }
    }
    // Use the table's source field for exact block-device matching. UUIDs survive
    // mount-path changes and /dev/sdX reassignment; fallback IDs are session-scoped.
    for volume in &mut report.volumes {
        let Some(source) = volume
            .id
            .strip_prefix("linux:")
            .and_then(|value| value.splitn(3, ':').nth(2))
        else {
            continue;
        };
        if let Ok(source) = fs::canonicalize(source)
            && let Some((uuid, label)) = identities.get(&source)
        {
            if let Some(uuid) = uuid {
                volume.id = format!("linux:uuid:{uuid}");
            }
            volume.label = label.clone();
        }
    }
    report
        .volumes
        .sort_by(|a, b| (&a.id, &a.mount_point).cmp(&(&b.id, &b.mount_point)));
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn udev_labels_decode_spaces_and_unicode_bytes() {
        assert_eq!(
            decode_label(b"Kindle\\x20Main\\x20Memory"),
            "Kindle Main Memory"
        );
        assert_eq!(decode_label(b"Reader\\xc5\\x82"), "Readerł");
        assert_eq!(decode_label(b"Reader\\xZZ"), "Reader\\xZZ");
    }
    #[test]
    fn mount_table_handles_renamed_paths_escapes_readonly_and_optional_fields() {
        let report = parse_mountinfo(b"36 25 8:17 / /media/user/My\\040Reader ro,nosuid shared:3 - vfat /dev/sdb1 ro\n37 25 8:18 / /mnt/other\\134reader rw - exfat /dev/sdb2 rw\n");
        assert!(report.warnings.is_empty());
        assert_eq!(report.volumes.len(), 2);
        assert_eq!(
            report.volumes[0].mount_point,
            Path::new("/media/user/My Reader")
        );
        assert!(report.volumes[0].read_only);
        assert!(!report.volumes[1].read_only);
        assert_eq!(report.volumes[1].filesystem, "exfat");
    }
    #[test]
    fn rejects_malformed_records_and_excludes_mtp_network_and_bind_subdirectories() {
        let report = parse_mountinfo(b"bad record\n1 0 8:1 / /mnt/bad\\099 rw - vfat /dev/sda1 rw\n2 0 0:1 / /run/user/mtp rw - fuse.jmtpfs jmtpfs rw\n3 0 0:2 / /mnt/share rw - cifs //server/share rw\n4 0 8:1 /documents /mnt/bind rw - vfat /dev/sda1 rw\n");
        assert!(report.volumes.is_empty());
        assert_eq!(report.warnings.len(), 2);
    }
    #[cfg(unix)]
    #[test]
    fn native_non_unicode_mount_path_is_preserved() {
        use std::os::unix::ffi::OsStrExt;
        let report = parse_mountinfo(b"1 0 8:1 / /media/reader-\\377 rw - vfat /dev/sda1 rw\n");
        assert_eq!(
            report.volumes[0].mount_point.as_os_str().as_bytes(),
            b"/media/reader-\xff"
        );
    }
}
