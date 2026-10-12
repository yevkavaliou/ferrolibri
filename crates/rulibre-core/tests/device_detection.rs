#[path = "../src/test_support.rs"]
mod test_support;

use rulibre_core::{
    device::{self, DetectionEvidence, DeviceKind},
    volumes::{self, MountedVolume},
};
use std::{fs, path::Path};
use test_support::Fixture;

fn volume(path: &Path, id: &str, label: Option<&str>) -> MountedVolume {
    MountedVolume {
        id: id.into(),
        mount_point: path.into(),
        label: label.map(str::to_owned),
        filesystem: "vfat".into(),
        read_only: false,
    }
}

#[test]
fn ordinary_disk_documents_only_and_labels_without_signatures_are_not_readers() {
    let fixture = Fixture::new();
    let ordinary = fixture.root.join("ordinary");
    let documents = fixture.root.join("flash");
    fs::create_dir(&ordinary).unwrap();
    fs::create_dir_all(documents.join("documents")).unwrap();
    for label in [None, Some("Kindle"), Some("KOBOeReader")] {
        let report = device::detect_devices_from_volumes(&[
            volume(&ordinary, "ordinary", label),
            volume(&documents, "flash", label),
        ]);
        assert!(report.devices.is_empty());
        if label.is_some() {
            assert_eq!(report.warnings.len(), 2);
        }
    }
    // Merely using a directory named Kindle or KOBOeReader is also insufficient.
    fs::remove_file(fixture.kindle.join("system/version.txt")).unwrap();
    fs::remove_file(fixture.kobo.join(".kobo/KoboReader.sqlite")).unwrap();
    assert!(
        device::detect_devices_from_volumes(&[
            volume(&fixture.kobo, "kobo", None),
            volume(&fixture.kindle, "kindle", None)
        ])
        .devices
        .is_empty()
    );
}

#[test]
fn renamed_readers_are_recognized_without_any_special_mount_path_or_label() {
    let fixture = Fixture::new();
    let renamed_kobo = fixture.root.join("Читалка Kobo renamed");
    let renamed_kindle = fixture.root.join("Ридер Kindle renamed");
    fs::rename(&fixture.kobo, &renamed_kobo).unwrap();
    fs::rename(&fixture.kindle, &renamed_kindle).unwrap();
    let report = device::detect_devices_from_volumes(&[
        volume(&renamed_kobo, "stable-kobo", Some("MyReader")),
        volume(&renamed_kindle, "stable-kindle", None),
    ]);
    assert!(report.warnings.is_empty());
    assert_eq!(report.devices.len(), 2);
    let kobo = report
        .devices
        .iter()
        .find(|d| d.kind == DeviceKind::Kobo)
        .unwrap();
    assert_eq!(kobo.mount_point, renamed_kobo);
    assert!(kobo.evidence.contains(&DetectionEvidence::KoboDatabase));
    assert!(
        kobo.evidence
            .contains(&DetectionEvidence::VolumeLabel("MyReader".into()))
    );
    let kindle = report
        .devices
        .iter()
        .find(|d| d.kind == DeviceKind::Kindle)
        .unwrap();
    assert_eq!(kindle.mount_point, renamed_kindle);
    assert!(kindle.evidence.contains(&DetectionEvidence::KindleVersion));
    assert!(report.single_device().is_none());
}

#[test]
fn identity_order_alias_deduplication_and_mount_path_change_are_deterministic() {
    let fixture = Fixture::new();
    let kobo = volume(&fixture.kobo, "uuid-one", Some("KOBOeReader"));
    let kindle = volume(&fixture.kindle, "uuid-two", Some("Kindle"));
    let first = device::detect_devices_from_volumes(&[kobo.clone(), kindle.clone(), kobo.clone()]);
    let second = device::detect_devices_from_volumes(&[kindle, kobo.clone()]);
    assert_eq!(first.devices, second.devices);
    assert_eq!(first.devices.len(), 2);
    let before = first
        .devices
        .iter()
        .find(|d| d.kind == DeviceKind::Kobo)
        .unwrap()
        .id
        .clone();
    let moved = fixture.root.join("moved");
    fs::rename(&fixture.kobo, &moved).unwrap();
    let mut changed = kobo;
    changed.mount_point = moved;
    let report = device::detect_devices_from_volumes(&[changed]);
    assert_eq!(report.single_device().unwrap().id, before);
}

#[test]
fn conflicting_signatures_and_conflicting_labels_require_explicit_selection() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.kobo.join("documents")).unwrap();
    fs::create_dir_all(fixture.kobo.join("system")).unwrap();
    fs::write(
        fixture.kobo.join("system/version.txt"),
        b"Kindle 5.4.0 (206145 050)",
    )
    .unwrap();
    let dual = volume(&fixture.kobo, "dual", None);
    let report = device::detect_devices_from_volumes(std::slice::from_ref(&dual));
    assert!(report.devices.is_empty());
    assert_eq!(report.warnings.len(), 1);
    assert!(report.warnings[0].to_string().contains("conflicting"));
    let chosen =
        device::manual_device_from_volumes(&fixture.kobo, DeviceKind::Kindle, &[dual]).unwrap();
    assert_eq!(chosen.evidence, vec![DetectionEvidence::ManualSelection]);
    assert_eq!(chosen.kind, DeviceKind::Kindle);
    let report = device::detect_devices_from_volumes(&[volume(
        &fixture.kindle,
        "wrong-label",
        Some("KOBOeReader"),
    )]);
    assert!(report.devices.is_empty());
    assert!(report.warnings[0].to_string().contains("label conflicts"));
}

#[test]
fn malformed_empty_and_incomplete_signatures_do_not_classify() {
    let fixture = Fixture::new();
    for version in [
        b"".as_slice(),
        b"not a reader",
        b"serial,unused,not-a-version",
    ] {
        fs::write(fixture.kobo.join(".kobo/version"), version).unwrap();
        assert!(
            device::detect_devices_from_volumes(&[volume(&fixture.kobo, "kobo", None)])
                .devices
                .is_empty()
        );
    }
    fs::write(
        fixture.kobo.join(".kobo/version"),
        b"serial,unused,4.38.21908",
    )
    .unwrap();
    fs::write(fixture.kobo.join(".kobo/KoboReader.sqlite"), b"not sqlite").unwrap();
    assert!(
        device::detect_devices_from_volumes(&[volume(&fixture.kobo, "kobo", None)])
            .devices
            .is_empty()
    );
    for version in [b"".as_slice(), b"version 5.4.0", b"Kindle unknown"] {
        fs::write(fixture.kindle.join("system/version.txt"), version).unwrap();
        assert!(
            device::detect_devices_from_volumes(&[volume(&fixture.kindle, "kindle", None)])
                .devices
                .is_empty()
        );
    }
}

#[test]
fn unavailable_or_bad_marker_is_warning_and_other_reader_survives() {
    let fixture = Fixture::new();
    fs::remove_file(fixture.kobo.join(".kobo/version")).unwrap();
    fs::create_dir(fixture.kobo.join(".kobo/version")).unwrap();
    let report = device::detect_devices_from_volumes(&[
        volume(&fixture.root.join("missing"), "missing", None),
        volume(&fixture.kobo, "kobo", None),
        volume(&fixture.kindle, "kindle", None),
    ]);
    assert_eq!(report.warnings.len(), 2);
    assert_eq!(report.single_device().unwrap().kind, DeviceKind::Kindle);
}

#[test]
fn manual_selection_validates_mount_root_kind_layout_readonly_and_library() {
    let fixture = Fixture::new();
    let list = [
        volume(&fixture.kobo, "kobo", None),
        volume(&fixture.kindle, "kindle", None),
    ];
    let before = test_support::snapshot(&fixture.root);
    fs::remove_file(fixture.kindle.join("system/version.txt")).unwrap();
    let selected =
        device::manual_device_from_volumes(&fixture.kindle, DeviceKind::Kindle, &list).unwrap();
    assert!(
        selected
            .evidence
            .contains(&DetectionEvidence::ManualSelection)
    );
    let after_remove = test_support::snapshot(&fixture.root);
    assert!(
        device::manual_device_from_volumes(
            &fixture.kindle.join("documents"),
            DeviceKind::Kindle,
            &list
        )
        .is_err()
    );
    assert!(device::manual_device_from_volumes(&fixture.kobo, DeviceKind::Kindle, &list).is_err());
    assert!(device::manual_device_from_volumes(&fixture.kindle, DeviceKind::Kobo, &list).is_err());
    assert!(
        device::manual_device_from_volumes(
            &fixture.root.join("missing"),
            DeviceKind::Kindle,
            &list
        )
        .is_err()
    );
    assert!(device::manual_device_from_volumes(&fixture.kindle, DeviceKind::Kindle, &[]).is_err());
    let mut readonly = list[1].clone();
    readonly.read_only = true;
    assert!(
        device::manual_device_from_volumes(&fixture.kindle, DeviceKind::Kindle, &[readonly])
            .is_err()
    );
    fs::create_dir(fixture.library.join("documents")).unwrap();
    assert!(
        device::manual_device_from_volumes(
            &fixture.library,
            DeviceKind::Kindle,
            &[volume(&fixture.library, "library", None)]
        )
        .is_err()
    );
    assert_eq!(test_support::snapshot(&fixture.root), after_remove);
    assert_ne!(before, after_remove);
}

#[test]
fn mtp_and_network_filesystems_are_excluded_even_with_reader_markers() {
    let fixture = Fixture::new();
    for filesystem in [
        "mtp",
        "fuse.jmtpfs",
        "fuse.simple-mtpfs",
        "fuse.gvfsd-fuse",
        "cifs",
        "nfs",
    ] {
        let mut unsupported = volume(&fixture.kobo, "mtp", Some("KOBOeReader"));
        unsupported.filesystem = filesystem.into();
        assert!(
            device::detect_devices_from_volumes(std::slice::from_ref(&unsupported))
                .devices
                .is_empty()
        );
        assert!(
            device::manual_device_from_volumes(&fixture.kobo, DeviceKind::Kobo, &[unsupported])
                .is_err()
        );
    }
}

#[cfg(windows)]
#[test]
fn locked_marker_returns_io_warning_without_hiding_other_devices() {
    use std::os::windows::fs::OpenOptionsExt;
    let fixture = Fixture::new();
    let _lock = fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(fixture.kobo.join(".kobo/version"))
        .unwrap();
    let report = device::detect_devices_from_volumes(&[
        volume(&fixture.kobo, "locked", None),
        volume(&fixture.kindle, "kindle", None),
    ]);
    assert_eq!(report.warnings.len(), 1);
    assert!(matches!(
        report.warnings[0].kind,
        rulibre_core::ErrorKind::Io(_)
    ));
    assert_eq!(report.single_device().unwrap().kind, DeviceKind::Kindle);
}

#[test]
fn linked_reader_directory_is_not_a_signature_or_manual_destination() {
    let fixture = Fixture::new();
    let root = fixture.root.join("linked-reader");
    fs::create_dir(&root).unwrap();
    let alias = root.join(".kobo");
    #[cfg(unix)]
    std::os::unix::fs::symlink(fixture.kobo.join(".kobo"), &alias).unwrap();
    #[cfg(windows)]
    {
        let output = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&alias)
            .arg(
                fixture
                    .kobo
                    .join(".kobo")
                    .to_string_lossy()
                    .replace('/', "\\"),
            )
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let list = [volume(&root, "linked", None)];
    assert!(
        device::detect_devices_from_volumes(&list)
            .devices
            .is_empty()
    );
    assert!(device::manual_device_from_volumes(&root, DeviceKind::Kobo, &list).is_err());
    #[cfg(windows)]
    fs::remove_dir(alias).unwrap();
}

#[test]
#[ignore = "read-only smoke check against the host's actual mounted local volumes"]
fn native_volume_enumeration_smoke() {
    let report = volumes::mounted_volumes().unwrap();
    assert!(!report.volumes.is_empty());
    for volume in &report.volumes {
        assert!(volume.mount_point.is_absolute());
        assert!(!volume.id.is_empty());
    }
    let detected = device::detect_devices().unwrap();
    println!(
        "Mounted local filesystems: {}; enumeration warnings: {}; readers: {}; recognition warnings: {}",
        report.volumes.len(),
        report.warnings.len(),
        detected.devices.len(),
        detected.warnings.len()
    );
    for warning in report.warnings.iter().chain(&detected.warnings) {
        println!("{warning}");
    }
}
