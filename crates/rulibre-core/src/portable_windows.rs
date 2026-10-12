//! Read present Windows portable devices through SetupAPI. No device writes.
use super::{DetectionEvidence, Device, DeviceId, DeviceKind, DiscoveryReport};
use std::{io, mem::size_of, path::Path, ptr};
use windows_sys::Win32::{
    Devices::DeviceAndDriverInstallation::{
        DIGCF_PRESENT, GUID_DEVCLASS_WPD, HDEVINFO, SP_DEVINFO_DATA, SPDRP_FRIENDLYNAME,
        SPDRP_SERVICE, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW,
        SetupDiGetDeviceInstanceIdW, SetupDiGetDeviceRegistryPropertyW,
    },
    Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_DATA, ERROR_NO_MORE_ITEMS},
};

struct DeviceSet(HDEVINFO);
impl Drop for DeviceSet {
    fn drop(&mut self) {
        // SAFETY: this handle was returned by SetupDiGetClassDevsW and is owned here.
        unsafe { SetupDiDestroyDeviceInfoList(self.0) };
    }
}

fn decode(buffer: &[u16]) -> String {
    String::from_utf16_lossy(buffer.split(|c| *c == 0).next().unwrap_or_default())
}

fn property(set: &DeviceSet, info: &SP_DEVINFO_DATA, key: u32) -> io::Result<String> {
    let mut buffer = vec![0u16; 256];
    loop {
        let mut required = 0;
        // SAFETY: info is initialized by enumeration; the aligned UTF-16 buffer is
        // writable for the supplied byte count, and no pointers escape the call.
        let ok = unsafe {
            SetupDiGetDeviceRegistryPropertyW(
                set.0,
                info,
                key,
                ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                (buffer.len() * 2) as u32,
                &mut required,
            )
        };
        if ok != 0 {
            return Ok(decode(&buffer));
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_INVALID_DATA as i32) {
            return Ok(String::new()); // optional property is absent
        }
        if error.raw_os_error() == Some(ERROR_INSUFFICIENT_BUFFER as i32)
            && required as usize > buffer.len() * 2
            && required <= 65_536
        {
            buffer.resize((required as usize).div_ceil(2), 0);
        } else {
            return Err(error);
        }
    }
}

fn instance_id(set: &DeviceSet, info: &SP_DEVINFO_DATA) -> io::Result<String> {
    let mut buffer = vec![0u16; 256];
    loop {
        let mut required = 0;
        // SAFETY: the device info and output buffer are valid for this call.
        let ok = unsafe {
            SetupDiGetDeviceInstanceIdW(
                set.0,
                info,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                &mut required,
            )
        };
        if ok != 0 {
            return Ok(decode(&buffer));
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_INSUFFICIENT_BUFFER as i32)
            && required as usize > buffer.len()
            && required <= 32_768
        {
            buffer.resize(required as usize, 0);
        } else {
            return Err(error);
        }
    }
}

fn kindle(id: &str, name: &str, service: &str) -> Option<Device> {
    // Do not classify phones, arbitrary devices named Kindle, or Amazon Fire
    // tablets as readers. All three independently obtained properties must agree.
    let normalized = id.to_ascii_lowercase();
    let usb = normalized.strip_prefix("usb\\")?;
    let hardware = usb.split('\\').next()?;
    if !hardware.split('&').any(|part| part == "vid_1949")
        || !name
            .split_whitespace()
            .any(|word| word.eq_ignore_ascii_case("kindle"))
        || !service.eq_ignore_ascii_case("WUDFWpdMtp")
    {
        return None;
    }
    Some(Device {
        id: DeviceId(format!("Kindle:windows:mtp:{normalized}")),
        kind: DeviceKind::Kindle,
        mount_point: Default::default(),
        label: Some(name.to_owned()),
        filesystem: "MTP".into(),
        read_only: false,
        evidence: vec![DetectionEvidence::WindowsPortableDevice],
    })
}

pub(super) fn enumerate() -> Result<DiscoveryReport, crate::Error> {
    // SAFETY: this constant identifies the WPD setup class, optional arguments
    // are null, and DIGCF_PRESENT excludes disconnected device records.
    let handle = unsafe {
        SetupDiGetClassDevsW(
            &GUID_DEVCLASS_WPD,
            ptr::null(),
            ptr::null_mut(),
            DIGCF_PRESENT,
        )
    };
    if handle == -1 {
        return Err(crate::Error::io(
            "enumerate Windows portable devices",
            Path::new("WPD"),
            io::Error::last_os_error(),
        ));
    }
    let set = DeviceSet(handle);
    let mut report = DiscoveryReport::default();
    for index in 0..u32::MAX {
        let mut info = SP_DEVINFO_DATA {
            cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
            ..Default::default()
        };
        // SAFETY: set is live and info has the required cbSize and writable storage.
        if unsafe { SetupDiEnumDeviceInfo(set.0, index, &mut info) } == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(ERROR_NO_MORE_ITEMS as i32) {
                break;
            }
            report.warnings.push(crate::Error::io(
                "enumerate Windows portable device",
                Path::new("WPD"),
                error,
            ));
            break;
        }
        let probe = || -> io::Result<Option<Device>> {
            let id = instance_id(&set, &info)?;
            let name = property(&set, &info, SPDRP_FRIENDLYNAME)?;
            let service = property(&set, &info, SPDRP_SERVICE)?;
            Ok(kindle(&id, &name, &service))
        };
        match probe() {
            Ok(Some(device)) => report.devices.push(device),
            Ok(None) => {}
            Err(error) => report.warnings.push(crate::Error::io(
                "read Windows portable device properties",
                Path::new("WPD"),
                error,
            )),
        }
    }
    report.devices.sort_by(|a, b| a.id.cmp(&b.id));
    report.devices.dedup_by(|a, b| a.id == b.id);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requires_amazon_usb_identity_kindle_name_and_mtp_service() {
        let id = "USB\\VID_1949&PID_9981\\fixture";
        let reader = kindle(id, "Kindle Paperwhite Signature Edition", "WUDFWpdMtp").unwrap();
        assert!(reader.is_mtp());
        assert!(reader.can_transfer());
        assert!(reader.mount_point.as_os_str().is_empty());
        assert_eq!(
            reader.id,
            kindle(&id.to_lowercase(), "Kindle", "wudfwpdmtp")
                .unwrap()
                .id
        );
        for (id, name, service) in [
            ("USB\\VID_1234&PID_9981\\fixture", "Kindle", "WUDFWpdMtp"),
            ("USB\\VID_19490&PID_9981\\fixture", "Kindle", "WUDFWpdMtp"),
            ("ROOT\\VID_1949\\fixture", "Kindle", "WUDFWpdMtp"),
            (id, "Fire tablet", "WUDFWpdMtp"),
            (id, "NotKindle", "WUDFWpdMtp"),
            (id, "Kindle", "USBSTOR"),
        ] {
            assert!(kindle(id, name, service).is_none());
        }
    }

    #[test]
    #[ignore = "reads attached Windows portable devices"]
    fn native_portable_device_smoke() {
        let report = enumerate().unwrap();
        for device in &report.devices {
            println!(
                "{} · {}",
                device.label.as_deref().unwrap_or("Kindle"),
                device.filesystem
            );
        }
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        assert!(
            report.devices.iter().any(Device::is_mtp),
            "attach an MTP Kindle"
        );
    }
}
