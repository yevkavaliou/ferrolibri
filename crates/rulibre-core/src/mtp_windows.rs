//! Windows WPD sessions and streams. Every COM interface stays on one thread.
use super::{Upload, copy};
use crate::{
    Error, ErrorKind,
    converter::CancellationToken,
    device::{Device, DeviceKind},
};
use std::{
    io,
    path::{Path, PathBuf},
    ptr,
};
use windows::{
    Win32::{
        Devices::PortableDevices::*,
        System::{
            Com::StructuredStorage::{PROPVARIANT, PropVariantClear},
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
                CoTaskMemFree, CoUninitialize, IStream, STGC_DEFAULT,
            },
            Variant::VT_ERROR,
        },
    },
    core::{HRESULT, Interface, PCWSTR, PWSTR},
};

fn error(operation: &'static str, cause: windows::core::Error) -> Error {
    let io = if cause.code().0 == 0x80070005u32 as i32 {
        io::Error::from(io::ErrorKind::PermissionDenied)
    } else {
        io::Error::other(cause.to_string())
    };
    Error::io(operation, Path::new("Kindle (MTP)"), io)
}
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

struct Apartment(bool);
impl Apartment {
    fn new() -> Result<Self, Error> {
        // SAFETY: called on the worker executing this operation. S_OK/S_FALSE
        // require one matching uninitialize; an existing STA remains untouched.
        let status = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if status.0 == 0x80010106u32 as i32 {
            return Ok(Self(false));
        }
        status.ok().map_err(|e| error("initialize WPD COM", e))?;
        Ok(Self(true))
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}
struct Wide(PWSTR);
impl Wide {
    fn string(&self) -> windows::core::Result<String> {
        if self.0.is_null() {
            return Ok(String::new());
        }
        // SAFETY: WPD owns a null-terminated UTF-16 allocation returned to us.
        unsafe { Ok(self.0.to_string()?) }
    }
}
impl Drop for Wide {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0.0.cast())) };
    }
}
struct Variant(PROPVARIANT);
impl Drop for Variant {
    fn drop(&mut self) {
        let _ = unsafe { PropVariantClear(&mut self.0) };
    }
}

fn interface_instance_id(value: &str) -> Option<String> {
    let value = value.to_ascii_lowercase();
    let interface = value.strip_prefix("\\\\?\\")?;
    let parts: Vec<_> = interface.split('#').collect();
    if parts.len() < 4 || parts[0] != "usb" || parts[2].is_empty() {
        return None;
    }
    Some(parts[..3].join("\\"))
}

struct Session {
    // Keep the device alive until all its content/property interfaces are gone.
    properties: IPortableDeviceProperties,
    content: IPortableDeviceContent,
    _device: IPortableDevice,
    documents: String,
    storage: String,
}
impl Session {
    fn open(device: &Device, writable: bool) -> Result<Self, Error> {
        if device.kind != DeviceKind::Kindle || !device.is_mtp() || device.read_only {
            return Err(Error::unavailable("select writable MTP Kindle", None));
        }
        let wanted = device
            .id
            .0
            .strip_prefix("Kindle:windows:mtp:")
            .ok_or_else(|| Error::unavailable("resolve MTP Kindle identity", None))?;
        // SAFETY: caller initialized COM; all returned interfaces and allocations
        // remain on this thread, with RAII ownership and valid output pointers.
        unsafe {
            let manager: IPortableDeviceManager =
                CoCreateInstance(&PortableDeviceManager, None, CLSCTX_INPROC_SERVER)
                    .map_err(|e| error("create WPD manager", e))?;
            let mut count = 0;
            manager
                .GetDevices(ptr::null_mut(), &mut count)
                .map_err(|e| error("count WPD devices", e))?;
            if count > 4096 {
                return Err(Error::unavailable("too many WPD devices", None));
            }
            let mut ids = vec![PWSTR::null(); count as usize];
            let result = manager.GetDevices(ids.as_mut_ptr(), &mut count);
            let ids: Vec<_> = ids.into_iter().map(Wide).collect();
            result.map_err(|e| error("enumerate WPD devices", e))?;
            let mut matched = None;
            for id in &ids {
                let text = id.string().map_err(|e| error("read WPD identity", e))?;
                if interface_instance_id(&text).as_deref() == Some(wanted) {
                    if matched.is_some() {
                        return Err(Error::unavailable("ambiguous WPD identity", None));
                    }
                    matched = Some(text);
                }
            }
            let id =
                wide(&matched.ok_or_else(|| Error::unavailable("MTP Kindle disconnected", None))?);
            let client: IPortableDeviceValues =
                CoCreateInstance(&PortableDeviceValues, None, CLSCTX_INPROC_SERVER)
                    .map_err(|e| error("create WPD client values", e))?;
            client
                .SetStringValue(&WPD_CLIENT_NAME, windows::core::w!("Rulibre"))
                .map_err(|e| error("configure WPD client", e))?;
            client
                .SetUnsignedIntegerValue(
                    &WPD_CLIENT_DESIRED_ACCESS,
                    if writable { 0xC0000000 } else { 0x80000000 },
                )
                .map_err(|e| error("configure WPD access", e))?;
            let native: IPortableDevice =
                CoCreateInstance(&PortableDeviceFTM, None, CLSCTX_INPROC_SERVER)
                    .map_err(|e| error("create WPD device", e))?;
            native
                .Open(PCWSTR(id.as_ptr()), &client)
                .map_err(|e| error("open MTP Kindle", e))?;
            let content = native.Content().map_err(|e| error("open WPD content", e))?;
            let properties = content
                .Properties()
                .map_err(|e| error("open WPD properties", e))?;
            let mut session = Self {
                properties,
                content,
                _device: native,
                documents: String::new(),
                storage: String::new(),
            };
            let mut destinations = vec![];
            for storage in session.children("DEVICE")? {
                let values = session.values(&storage)?;
                if values.GetGuidValue(&WPD_FUNCTIONAL_OBJECT_CATEGORY).ok()
                    != Some(WPD_FUNCTIONAL_CATEGORY_STORAGE)
                {
                    continue;
                }
                if writable
                    && values
                        .GetUnsignedIntegerValue(&WPD_STORAGE_ACCESS_CAPABILITY)
                        .is_ok_and(|v| v != 0)
                {
                    continue;
                }
                for child in session.children(&storage)? {
                    let values = session.values(&child)?;
                    if values.GetGuidValue(&WPD_OBJECT_CONTENT_TYPE).ok()
                        == Some(WPD_CONTENT_TYPE_FOLDER)
                        && Self::name(&values)?.eq_ignore_ascii_case("documents")
                    {
                        destinations.push((storage.clone(), child));
                    }
                }
            }
            if destinations.len() != 1 {
                return Err(Error::unavailable(
                    "find one writable Kindle documents folder",
                    None,
                ));
            }
            (session.storage, session.documents) = destinations.pop().unwrap();
            Ok(session)
        }
    }
    fn values(&self, id: &str) -> Result<IPortableDeviceValues, Error> {
        let id = wide(id);
        unsafe {
            self.properties
                .GetValues(PCWSTR(id.as_ptr()), None::<&IPortableDeviceKeyCollection>)
        }
        .map_err(|e| error("read WPD object properties", e))
    }
    fn name(values: &IPortableDeviceValues) -> Result<String, Error> {
        let value = unsafe {
            values
                .GetStringValue(&WPD_OBJECT_ORIGINAL_FILE_NAME)
                .or_else(|_| values.GetStringValue(&WPD_OBJECT_NAME))
        }
        .map_err(|e| error("read WPD object name", e))?;
        Wide(value)
            .string()
            .map_err(|e| error("decode WPD object name", e))
    }
    fn children(&self, id: &str) -> Result<Vec<String>, Error> {
        let id = wide(id);
        let enumerator = unsafe {
            self.content
                .EnumObjects(0, PCWSTR(id.as_ptr()), None::<&IPortableDeviceValues>)
        }
        .map_err(|e| error("enumerate WPD folder", e))?;
        let mut result = vec![];
        loop {
            let mut ids = [PWSTR::null(); 32];
            let mut fetched = 0;
            let status = unsafe { enumerator.Next(&mut ids, &mut fetched) };
            let ids = ids.map(Wide);
            status
                .ok()
                .map_err(|e| error("read WPD folder entries", e))?;
            if fetched > 32 {
                return Err(Error::unavailable("invalid WPD enumeration count", None));
            }
            for id in &ids[..fetched as usize] {
                result.push(id.string().map_err(|e| error("decode WPD object ID", e))?);
            }
            if fetched == 0 {
                break;
            }
            if result.len() > 100_000 {
                return Err(Error::unavailable("WPD folder enumeration limit", None));
            }
        }
        Ok(result)
    }
    fn delete(&self, id: &str) -> Result<(), Error> {
        // Delete exactly our newly created object, never an existing filename match.
        unsafe {
            let values: IPortableDeviceValues =
                CoCreateInstance(&PortableDeviceValues, None, CLSCTX_INPROC_SERVER)
                    .map_err(|e| error("create WPD deletion values", e))?;
            let id = wide(id);
            values
                .SetStringValue(&WPD_OBJECT_ID, PCWSTR(id.as_ptr()))
                .map_err(|e| error("set WPD deletion ID", e))?;
            let value = Variant(
                values
                    .GetValue(&WPD_OBJECT_ID)
                    .map_err(|e| error("encode WPD deletion ID", e))?,
            );
            let ids: IPortableDevicePropVariantCollection = CoCreateInstance(
                &PortableDevicePropVariantCollection,
                None,
                CLSCTX_INPROC_SERVER,
            )
            .map_err(|e| error("create WPD deletion list", e))?;
            ids.Add(&value.0)
                .map_err(|e| error("add WPD deletion ID", e))?;
            let mut results = None;
            self.content
                .Delete(0, &ids, &mut results)
                .map_err(|e| error("delete failed MTP upload", e))?;
            let results =
                results.ok_or_else(|| Error::unavailable("missing WPD deletion result", None))?;
            let mut result = Variant(PROPVARIANT::default());
            results
                // The generated signature uses *const for this COM output.
                // Preserve mutable provenance instead of passing a shared reference.
                .GetAt(0, &raw mut result.0)
                .map_err(|e| error("read WPD deletion result", e))?;
            if result.0.Anonymous.Anonymous.vt != VT_ERROR {
                return Err(Error::unavailable("invalid WPD deletion result", None));
            }
            HRESULT(result.0.Anonymous.Anonymous.Anonymous.scode)
                .ok()
                .map_err(|e| error("remove failed MTP upload", e))
        }
    }
}

struct NativeUpload<'a> {
    session: &'a Session,
    stream: Option<IStream>,
    object: Option<String>,
    committed: bool,
}
impl Upload for NativeUpload<'_> {
    fn begin(&mut self, name: &str, size: u64) -> Result<(), Error> {
        for id in self.session.children(&self.session.documents)? {
            if Session::name(&self.session.values(&id)?)?.to_lowercase() == name.to_lowercase() {
                return Err(Error {
                    operation: "create MTP book",
                    path: Some(PathBuf::from("Kindle (MTP)").join("documents").join(name)),
                    kind: ErrorKind::Conflict,
                });
            }
        }
        unsafe {
            if self
                .session
                .values(&self.session.storage)?
                .GetUnsignedLargeIntegerValue(&WPD_STORAGE_FREE_SPACE_IN_BYTES)
                .is_ok_and(|free| free < size)
            {
                return Err(Error::io(
                    "MTP storage full",
                    Path::new("Kindle (MTP)"),
                    io::Error::from_raw_os_error(112),
                ));
            }
            let values: IPortableDeviceValues =
                CoCreateInstance(&PortableDeviceValues, None, CLSCTX_INPROC_SERVER)
                    .map_err(|e| error("create WPD upload properties", e))?;
            let parent = wide(&self.session.documents);
            let filename = wide(name);
            values
                .SetStringValue(&WPD_OBJECT_PARENT_ID, PCWSTR(parent.as_ptr()))
                .map_err(|e| error("set MTP parent", e))?;
            values
                .SetStringValue(&WPD_OBJECT_NAME, PCWSTR(filename.as_ptr()))
                .map_err(|e| error("set MTP name", e))?;
            values
                .SetStringValue(&WPD_OBJECT_ORIGINAL_FILE_NAME, PCWSTR(filename.as_ptr()))
                .map_err(|e| error("set MTP filename", e))?;
            values
                .SetUnsignedLargeIntegerValue(&WPD_OBJECT_SIZE, size)
                .map_err(|e| error("set MTP size", e))?;
            values
                .SetGuidValue(&WPD_OBJECT_CONTENT_TYPE, &WPD_CONTENT_TYPE_GENERIC_FILE)
                .map_err(|e| error("set MTP content type", e))?;
            values
                .SetGuidValue(&WPD_OBJECT_FORMAT, &WPD_OBJECT_FORMAT_UNSPECIFIED)
                .map_err(|e| error("set MTP format", e))?;
            let mut optimal = 0;
            self.session
                .content
                .CreateObjectWithPropertiesAndData(
                    &values,
                    &mut self.stream,
                    &mut optimal,
                    ptr::null_mut(),
                )
                .map_err(|e| error("create MTP upload stream", e))?;
        }
        if self.stream.is_none() {
            return Err(Error::unavailable("missing MTP upload stream", None));
        }
        Ok(())
    }
    fn write(&mut self, bytes: &[u8]) -> Result<usize, Error> {
        let mut written = 0;
        unsafe {
            self.stream.as_ref().unwrap().Write(
                bytes.as_ptr().cast(),
                bytes.len() as u32,
                Some(&mut written),
            )
        }
        .ok()
        .map_err(|e| error("write MTP book", e))?;
        Ok(written as usize)
    }
    fn finish(&mut self, size: u64) -> Result<(), Error> {
        let stream = self.stream.as_ref().unwrap();
        let data: IPortableDeviceDataStream =
            stream.cast().map_err(|e| error("get WPD data stream", e))?;
        unsafe { stream.Commit(STGC_DEFAULT) }.map_err(|e| error("commit MTP book", e))?;
        self.committed = true;
        let id = Wide(
            unsafe { data.GetObjectID() }.map_err(|e| error("get uploaded MTP object ID", e))?,
        )
        .string()
        .map_err(|e| error("decode uploaded MTP ID", e))?;
        self.object = Some(id.clone());
        let actual = unsafe {
            self.session
                .values(&id)?
                .GetUnsignedLargeIntegerValue(&WPD_OBJECT_SIZE)
        }
        .map_err(|e| error("verify uploaded MTP size", e))?;
        if actual != size {
            return Err(Error::invalid_data(
                "verify MTP book",
                Path::new("Kindle (MTP)"),
                "uploaded size differs",
            ));
        }
        Ok(())
    }
    fn abort(&mut self) -> Result<(), Error> {
        if let Some(id) = self.object.take() {
            return self.session.delete(&id);
        }
        if self.committed {
            return Err(Error::unavailable(
                "MTP upload committed but object ID is unavailable",
                None,
            ));
        }
        if let Some(stream) = self.stream.take() {
            unsafe { stream.Revert() }.map_err(|e| error("revert incomplete MTP upload", e))?;
        }
        Ok(())
    }
}

pub(super) fn validate(device: &Device) -> Result<(), Error> {
    let _apartment = Apartment::new()?;
    Session::open(device, true).map(|_| ())
}
pub(super) fn transfer(
    file: &Path,
    device: &Device,
    cancel: &CancellationToken,
    progress: impl FnMut(u64, u64),
) -> Result<PathBuf, Error> {
    let _apartment = Apartment::new()?;
    let session = Session::open(device, true)?;
    let mut upload = NativeUpload {
        session: &session,
        stream: None,
        object: None,
        committed: false,
    };
    copy(file, &mut upload, cancel, progress)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Com::STGM_READ;

    fn read_back(session: &Session, id: &str) -> Result<Vec<u8>, Error> {
        let id = wide(id);
        unsafe {
            let resources = session
                .content
                .Transfer()
                .map_err(|e| error("open MTP readback", e))?;
            let mut optimal = 0;
            let mut stream = None;
            resources
                .GetStream(
                    PCWSTR(id.as_ptr()),
                    &WPD_RESOURCE_DEFAULT,
                    STGM_READ.0,
                    &mut optimal,
                    &mut stream,
                )
                .map_err(|e| error("read uploaded MTP resource", e))?;
            let stream =
                stream.ok_or_else(|| Error::unavailable("missing MTP readback stream", None))?;
            let mut result = vec![];
            let mut bytes = [0u8; 65_536];
            loop {
                let mut count = 0;
                stream
                    .Read(
                        bytes.as_mut_ptr().cast(),
                        bytes.len() as u32,
                        Some(&mut count),
                    )
                    .ok()
                    .map_err(|e| error("read MTP verification bytes", e))?;
                if count == 0 {
                    break;
                }
                result.extend_from_slice(&bytes[..count as usize]);
                if result.len() > 10_000_000 {
                    return Err(Error::unavailable("MTP verification size limit", None));
                }
            }
            Ok(result)
        }
    }

    #[test]
    #[ignore = "writes a unique synthetic AZW3 to attached Kindle, verifies and removes only that object; requires ebook-convert"]
    fn native_mtp_roundtrip_and_cancel() {
        let f = crate::test_support::Fixture::new();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let name = format!("Rulibre MTP verification {stamp}.azw3");
        let file = f.book.join(&name);
        let text = f.root.join("verification.txt");
        std::fs::write(
            &text,
            "Rulibre MTP transfer verification.\nTemporary test book.\n",
        )
        .unwrap();
        let conversion = std::process::Command::new("ebook-convert")
            .arg(&text)
            .arg(&file)
            .args([
                "--title",
                "Rulibre MTP verification",
                "--authors",
                "Rulibre",
            ])
            .output()
            .expect("installed ebook-convert");
        assert!(
            conversion.status.success(),
            "{}",
            String::from_utf8_lossy(&conversion.stderr)
        );
        let original = std::fs::read(&file).unwrap();
        let library_before = crate::test_support::snapshot(&f.library);
        let report = crate::device::detect_devices().unwrap();
        let device = report
            .devices
            .into_iter()
            .find(Device::is_mtp)
            .expect("attach MTP Kindle");
        let _apartment = Apartment::new().unwrap();
        let before = {
            let session = Session::open(&device, false).unwrap();
            session.children(&session.documents).unwrap()
        };
        // Exercise the production job route: it must never ask for a mount path.
        let executor = crate::jobs::JobExecutor::with_volume_source(1, 2, || {
            Err(Error::unavailable(
                "MTP must not enumerate filesystem volumes",
                None,
            ))
        })
        .unwrap();
        let handle = executor
            .submit(crate::jobs::JobRequest::Send {
                book: f.book.clone(),
                author: "Rulibre".into(),
                device: device.clone(),
                options: Default::default(),
            })
            .unwrap();
        let result = handle.wait().unwrap();
        assert!(matches!(result, crate::jobs::JobOutput::Sent(_)));
        let session = Session::open(&device, true).unwrap();
        let created: Vec<_> = session
            .children(&session.documents)
            .unwrap()
            .into_iter()
            .filter(|id| !before.contains(id))
            .collect();
        let own: Vec<_> = created
            .into_iter()
            .filter(|id| Session::name(&session.values(id).unwrap()).unwrap() == name)
            .collect();
        assert_eq!(own.len(), 1, "one unique verification object expected");
        let id = &own[0];
        let verification = (|| -> Result<(), Error> {
            if read_back(&session, id)? != original {
                return Err(Error::unavailable("MTP verification bytes differ", None));
            }
            let mut duplicate = NativeUpload {
                session: &session,
                stream: None,
                object: None,
                committed: false,
            };
            let duplicate = copy(
                &file,
                &mut duplicate,
                &CancellationToken::default(),
                |_, _| {},
            )
            .unwrap_err();
            if !matches!(duplicate.kind, ErrorKind::Conflict) {
                return Err(duplicate);
            }
            if read_back(&session, id)? != original {
                return Err(Error::unavailable("duplicate changed existing book", None));
            }
            Ok(())
        })();
        // Delete the exact new object even if byte verification fails.
        session.delete(id).unwrap();
        verification.unwrap();
        assert_eq!(crate::test_support::snapshot(&f.library), library_before);
        let cancelled_name = format!("Rulibre MTP cancellation {stamp}.azw3");
        let cancelled_file = f.root.join(&cancelled_name);
        std::fs::copy(&file, &cancelled_file).unwrap();
        let cancel = CancellationToken::default();
        let mut upload = NativeUpload {
            session: &session,
            stream: None,
            object: None,
            committed: false,
        };
        let error = copy(&cancelled_file, &mut upload, &cancel, |n, _| {
            if n > 0 {
                cancel.cancel();
            }
        })
        .unwrap_err();
        assert!(matches!(error.kind, ErrorKind::Cancelled), "{error}");
        for id in session.children(&session.documents).unwrap() {
            assert_ne!(
                Session::name(&session.values(&id).unwrap()).unwrap(),
                cancelled_name
            );
        }
        println!(
            "MTP job: AZW3 copied, byte-for-byte readback passed; duplicate preserved; cancellation reverted; test objects removed"
        );
    }
    #[test]
    fn maps_interface_to_exact_usb_instance() {
        assert_eq!(
            interface_instance_id("\\\\?\\USB#VID_1949&PID_9981#SERIAL#{guid}"),
            Some("usb\\vid_1949&pid_9981\\serial".into())
        );
        assert!(interface_instance_id("USB\\VID_1949\\serial").is_none());
    }
    #[test]
    #[ignore = "reads an attached MTP Kindle storage; no writes"]
    fn native_mtp_documents() {
        let report = crate::device::detect_devices().unwrap();
        let device = report
            .devices
            .iter()
            .find(|d| d.is_mtp())
            .expect("attach MTP Kindle");
        let _apartment = Apartment::new().unwrap();
        let session = Session::open(device, false).unwrap();
        println!(
            "Kindle documents found; {} entries",
            session.children(&session.documents).unwrap().len()
        );
    }
}
