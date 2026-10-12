//! Synthetic fixtures only: no personal library, real mounts or converters.
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

pub struct Fixture {
    pub root: PathBuf,
    pub library: PathBuf,
    pub book: PathBuf,
    pub kobo: PathBuf,
    pub kindle: PathBuf,
}

impl Fixture {
    pub fn new() -> Self {
        let root = loop {
            let candidate = std::env::temp_dir().join(format!(
                "rulibre-test-{}-{}",
                std::process::id(),
                NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&candidate) {
                Ok(()) => break candidate,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create fixture {}: {error}", candidate.display()),
            }
        };
        let fixture = Self {
            library: root.join("library"),
            book: root.join("library/Автор Example/A small book (42)"),
            kobo: root.join("devices/KOBOeReader"),
            kindle: root.join("devices/Kindle"),
            root,
        };
        fixture.populate();
        fixture
    }

    fn populate(&self) {
        for directory in [
            &self.book,
            &self.kobo.join(".kobo"),
            &self.kindle.join("documents"),
        ] {
            fs::create_dir_all(directory).unwrap();
        }
        fs::write(
            self.book.join("metadata.opf"),
            include_str!("../tests/fixtures/metadata.opf"),
        )
        .unwrap();
        fs::write(self.library.join("metadata.db"), b"synthetic database").unwrap();
        // Extension fixtures are intentionally not valid ebook containers.
        for filename in [
            "A small book.epub",
            "A small book.pdf",
            "A small book.mobi",
            "A small book.kepub.epub",
            "cover.jpg",
            ".DS_Store",
        ] {
            fs::write(
                self.book.join(filename),
                format!("synthetic fixture: {filename}\n"),
            )
            .unwrap();
        }
        for directory in [".caltrash", ".calnotes", "downloaded"] {
            let skipped = self.library.join(directory).join("Hidden (99)");
            fs::create_dir_all(&skipped).unwrap();
            fs::write(skipped.join("hidden.epub"), b"synthetic").unwrap();
        }
        fs::create_dir_all(self.library.join("Zed/Second book (43)")).unwrap();
        fs::write(
            self.library.join("Zed/Second book (43)/Second book.txt"),
            b"synthetic text",
        )
        .unwrap();
        fs::write(
            self.kobo.join(".kobo/version"),
            b"SYNTHETIC-KOBO,unused,4.38.21908,00000000-0000-0000-0000-000000000000\n",
        )
        .unwrap();
        fs::write(
            self.kobo.join(".kobo/KoboReader.sqlite"),
            b"SQLite format 3\0synthetic fixture",
        )
        .unwrap();
        fs::create_dir_all(self.kindle.join("system")).unwrap();
        fs::write(
            self.kindle.join("system/version.txt"),
            b"Kindle 5.4.0 (206145 050)\n",
        )
        .unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.root) {
            eprintln!("cleanup fixture {}: {error}", self.root.display());
        }
    }
}

/// Capture names and bytes of every library file, including metadata.db/OPF.
#[allow(dead_code)] // This fixture module is also included by unrelated integration tests.
pub fn snapshot(root: &std::path::Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    fn visit(
        root: &std::path::Path,
        path: &std::path::Path,
        files: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>,
    ) {
        for entry in fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut files = std::collections::BTreeMap::new();
    visit(root, root, &mut files);
    files
}

/// Synthetic devices deliberately bypass OS enumeration for file-transfer tests.
#[allow(dead_code)]
pub fn device(
    kind: rulibre_core::device::DeviceKind,
    path: &std::path::Path,
) -> rulibre_core::device::Device {
    rulibre_core::device::Device {
        id: rulibre_core::device::DeviceId(format!("fixture:{}", path.display())),
        kind,
        mount_point: path.to_owned(),
        label: None,
        filesystem: "vfat".into(),
        read_only: false,
        evidence: vec![rulibre_core::device::DetectionEvidence::ManualSelection],
    }
}
