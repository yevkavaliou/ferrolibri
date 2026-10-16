use super::{MountedVolume, VolumeReport, local_filesystem};
use std::{ffi::OsString, io, os::windows::ffi::OsStringExt, path::Path, ptr};
use windows_sys::Win32::{
    Foundation::{ERROR_MORE_DATA, ERROR_NO_MORE_FILES, HANDLE, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{
        FindFirstVolumeW, FindNextVolumeW, FindVolumeClose, GetDriveTypeW, GetVolumeInformationW,
        GetVolumePathNamesForVolumeNameW,
    },
};

struct Search(HANDLE);
impl Drop for Search {
    fn drop(&mut self) {
        unsafe {
            FindVolumeClose(self.0);
        }
    }
}
fn wide_string(bytes: &[u16]) -> String {
    String::from_utf16_lossy(bytes.split(|value| *value == 0).next().unwrap_or_default())
}
fn volume_paths(name: &[u16]) -> io::Result<Vec<PathBuf>> {
    let mut buffer = vec![0u16; 256];
    loop {
        let mut required = 0;
        let success = unsafe {
            GetVolumePathNamesForVolumeNameW(
                name.as_ptr(),
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                &mut required,
            )
        };
        if success != 0 {
            return Ok(buffer
                .split(|value| *value == 0)
                .take_while(|part| !part.is_empty())
                .map(|part| PathBuf::from(OsString::from_wide(part)))
                .collect());
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_MORE_DATA as i32) && required as usize > buffer.len()
        {
            buffer.resize(required as usize, 0);
        } else {
            return Err(error);
        }
    }
}

pub(super) fn enumerate() -> Result<VolumeReport, crate::Error> {
    let mut report = VolumeReport::default();
    let mut name = vec![0u16; 1024];
    let handle = unsafe { FindFirstVolumeW(name.as_mut_ptr(), name.len() as u32) };
    if handle == INVALID_HANDLE_VALUE {
        return Err(crate::Error::io(
            "enumerate Windows volumes",
            Path::new("/"),
            io::Error::last_os_error(),
        ));
    }
    let search = Search(handle);
    loop {
        let guid = wide_string(&name);
        let path = Path::new(&guid);
        match volume_paths(&name) {
            Ok(paths) if !paths.is_empty() => {
                // USB readers can report as either removable (2) or fixed (3).
                if matches!(unsafe { GetDriveTypeW(name.as_ptr()) }, 2 | 3) {
                    let mut label = [0u16; 261];
                    let mut filesystem = [0u16; 261];
                    let mut flags = 0;
                    let success = unsafe {
                        GetVolumeInformationW(
                            name.as_ptr(),
                            label.as_mut_ptr(),
                            label.len() as u32,
                            ptr::null_mut(),
                            ptr::null_mut(),
                            &mut flags,
                            filesystem.as_mut_ptr(),
                            filesystem.len() as u32,
                        )
                    };
                    if success == 0 {
                        report.warnings.push(crate::Error::io(
                            "read Windows volume information",
                            path,
                            io::Error::last_os_error(),
                        ));
                    } else {
                        let filesystem = wide_string(&filesystem);
                        if local_filesystem(&filesystem) {
                            let label = wide_string(&label);
                            for mount_point in paths {
                                report.volumes.push(MountedVolume {
                                    id: format!("windows:{}", guid.to_ascii_lowercase()),
                                    mount_point,
                                    label: if label.is_empty() {
                                        None
                                    } else {
                                        Some(label.clone())
                                    },
                                    filesystem: filesystem.clone(),
                                    read_only: flags & 0x0008_0000 != 0, /* FILE_READ_ONLY_VOLUME */
                                });
                            }
                        }
                    }
                }
            }
            Ok(_) => {}
            Err(error) => report.warnings.push(crate::Error::io(
                "read Windows volume mount paths",
                path,
                error,
            )),
        }
        name.fill(0);
        if unsafe { FindNextVolumeW(search.0, name.as_mut_ptr(), name.len() as u32) } == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                break;
            }
            return Err(crate::Error::io(
                "enumerate next Windows volume",
                Path::new("/"),
                error,
            ));
        }
    }
    report
        .volumes
        .sort_by(|a, b| (&a.id, &a.mount_point).cmp(&(&b.id, &b.mount_point)));
    Ok(report)
}

use std::path::PathBuf;
