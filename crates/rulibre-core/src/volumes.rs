//! Enumerate mounted local filesystems independently of reader recognition.
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountedVolume {
    /// OS volume identity, independent of the displayed mount path when available.
    pub id: String,
    pub mount_point: PathBuf,
    pub label: Option<String>,
    pub filesystem: String,
    pub read_only: bool,
}

#[derive(Debug, Default)]
pub struct VolumeReport {
    pub volumes: Vec<MountedVolume>,
    pub warnings: Vec<crate::Error>,
}

#[cfg(any(target_os = "linux", test))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

pub fn mounted_volumes() -> Result<VolumeReport, crate::Error> {
    #[cfg(windows)]
    {
        windows::enumerate()
    }
    #[cfg(target_os = "linux")]
    {
        linux::enumerate()
    }
    #[cfg(target_os = "macos")]
    {
        macos::enumerate()
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        Err(crate::Error::unavailable(
            "enumerate mounted local filesystems on this platform",
            None,
        ))
    }
}

/// Excludes network, pseudo-filesystems and MTP/FUSE bridges. Recognition only
/// handles filesystem paths; portable-device objects require a separate backend.
pub(crate) fn local_filesystem(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "fat"
            | "fat32"
            | "vfat"
            | "msdos"
            | "exfat"
            | "ntfs"
            | "ntfs3"
            | "fuseblk"
            | "refs"
            | "ext2"
            | "ext3"
            | "ext4"
            | "btrfs"
            | "xfs"
            | "f2fs"
            | "hfs"
            | "hfsplus"
            | "apfs"
            | "ufs"
    )
}
