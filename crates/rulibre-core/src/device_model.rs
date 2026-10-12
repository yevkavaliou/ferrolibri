//! Device transport types shared by the host and WASM frontend.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DeviceKind {
    Kobo,
    Kindle,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DeviceId(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetectionEvidence {
    VolumeLabel(String),
    KoboVersion,
    KoboDatabase,
    KindleVersion,
    KindleDocuments,
    ManualSelection,
    WindowsPortableDevice,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    pub id: DeviceId,
    pub kind: DeviceKind,
    /// Empty for portable MTP devices; never interpret it as a local directory.
    pub mount_point: PathBuf,
    pub label: Option<String>,
    pub filesystem: String,
    pub read_only: bool,
    pub evidence: Vec<DetectionEvidence>,
}

impl Device {
    /// MTP devices have an OS identity, but no filesystem mount point.
    pub fn is_mtp(&self) -> bool {
        self.filesystem.eq_ignore_ascii_case("mtp")
    }

    pub fn can_transfer(&self) -> bool {
        !self.read_only && (!self.is_mtp() || cfg!(windows))
    }

    pub fn name(&self) -> &'static str {
        match self.kind {
            DeviceKind::Kobo => "Kobo",
            DeviceKind::Kindle => "Kindle",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeviceChange {
    Connected(Device),
    Disconnected(DeviceId),
    Changed { previous: Device, current: Device },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum DeviceEvent {
    Snapshot {
        devices: Vec<Device>,
        selected_id: Option<DeviceId>,
        changes: Vec<DeviceChange>,
        warnings: Vec<String>,
    },
    PollFailed {
        diagnostic: String,
    },
}

impl DeviceEvent {
    pub fn selected_device(&self) -> Option<&Device> {
        match self {
            Self::Snapshot {
                devices,
                selected_id,
                ..
            } => devices
                .iter()
                .find(|device| Some(&device.id) == selected_id.as_ref()),
            Self::PollFailed { .. } => None,
        }
    }

    pub fn message(&self) -> Result<String, String> {
        match self {
            Self::PollFailed { diagnostic } => Err(format!("Device detection: {diagnostic}")),
            Self::Snapshot { warnings, .. } if !warnings.is_empty() => {
                Err(format!("Device detection: {}", warnings.join("; ")))
            }
            Self::Snapshot { devices, .. } => Ok(match self.selected_device() {
                Some(device) => format!(
                    "{} selected ({})",
                    device.name(),
                    if device.is_mtp() {
                        "USB / MTP".into()
                    } else {
                        device.mount_point.display().to_string()
                    }
                ),
                None if devices.len() > 1 => "Multiple readers detected; select a device".into(),
                None => "No reader selected".into(),
            }),
        }
    }
}
