use super::{MountedVolume, VolumeReport, local_filesystem};
use std::{ffi::CStr, io, os::unix::ffi::OsStringExt, path::Path, sync::Mutex};

// getmntinfo owns a shared static buffer. Copy it while holding this lock.
static MOUNT_TABLE: Mutex<()> = Mutex::new(());
pub(super) fn enumerate() -> Result<VolumeReport, crate::Error> {
    let _guard = MOUNT_TABLE.lock().map_err(|error| {
        crate::Error::invalid_data("lock macOS mount table", Path::new("/"), error.to_string())
    })?;
    let mut pointer = std::ptr::null_mut();
    let count = unsafe { libc::getmntinfo(&mut pointer, libc::MNT_NOWAIT) };
    if count <= 0 || pointer.is_null() {
        return Err(crate::Error::io(
            "read macOS mount table",
            Path::new("/"),
            io::Error::last_os_error(),
        ));
    }
    let mut report = VolumeReport::default();
    for entry in unsafe { std::slice::from_raw_parts(pointer, count as usize) } {
        // statfs names are NUL-terminated fixed-size strings supplied by libc.
        let filesystem = unsafe { CStr::from_ptr(entry.f_fstypename.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        if !local_filesystem(&filesystem) || entry.f_flags & libc::MNT_LOCAL as u32 == 0 {
            continue;
        }
        let source = unsafe { CStr::from_ptr(entry.f_mntfromname.as_ptr()) }.to_bytes();
        let mount = unsafe { CStr::from_ptr(entry.f_mntonname.as_ptr()) }.to_bytes();
        report.volumes.push(MountedVolume {
            id: format!("macos:{}", String::from_utf8_lossy(source)),
            mount_point: std::ffi::OsString::from_vec(mount.to_vec()).into(),
            label: None,
            filesystem,
            read_only: entry.f_flags & libc::MNT_RDONLY as u32 != 0,
        });
    }
    report
        .volumes
        .sort_by(|a, b| (&a.id, &a.mount_point).cmp(&(&b.id, &b.mount_point)));
    Ok(report)
}
