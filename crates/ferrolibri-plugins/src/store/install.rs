//! Installing a plugin from a folder in two steps: [`PluginStore::inspect_local`] reads only
//! the manifest and tells what the plugin is and asks for; [`PluginStore::install`] copies it
//! into the store, atomically (D2, D11).

use std::{
    fs,
    hash::{BuildHasher, Hasher, RandomState},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use ferrolibri_core::{Error, ErrorKind};
use semver::Version;

use super::{
    InstallRecord, InstallSource, InstalledPlugin, MANIFEST_FILE, PluginStore, RECORD_FILE,
    not_compatible, read_plain_file, sha256_hex, state,
};
use crate::{
    Compatibility, Manifest, Reason, SettingKind, app_version,
    limits::{DEFAULT_CALL_TIMEOUT_S, MAX_MANIFEST_BYTES, MAX_SCRIPT_BYTES},
    manifest::check_entry,
    plugin_error,
};

/// Staging and backup directories left by a crash are removed after this long.
const STALE_AFTER: Duration = Duration::from_secs(60 * 60);
const STAGING_PREFIX: &str = ".staging-";
const BACKUP_PREFIX: &str = ".old-";

/// Names that Windows treats as devices, with or without an extension. A directory with such a
/// name cannot be used there, so no plugin may have such an `id`.
const RESERVED_IDS: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// What installing would do to the plugin that is already there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallChange {
    New,
    Reinstall {
        version: String,
    },
    Upgrade {
        from: String,
        to: String,
    },
    /// Needs [`Downgrade::Allow`].
    Downgrade {
        from: String,
        to: String,
    },
}

/// Whether [`PluginStore::install`] may replace a plugin with an older version.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Downgrade {
    #[default]
    Refuse,
    Allow,
}

/// What the plugin may do, for the user to read before installing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Permissions {
    /// `network.domains` as declared.
    pub domains: Vec<String>,
    /// The time budget of one call; more than 30 seconds is worth pointing out.
    pub call_timeout_s: u32,
    /// Labels of the settings whose address the plugin may then reach (`network = true`).
    pub network_settings: Vec<String>,
}

/// The result of [`PluginStore::inspect_local`]. Nothing was installed and nothing was copied.
#[derive(Clone, Debug)]
pub struct Inspection {
    pub manifest: Manifest,
    /// The contract's checks plus the store's: see `RESERVED_IDS`. Only a compatible plugin
    /// can be installed.
    pub compatibility: Compatibility,
    pub permissions: Permissions,
    /// Bytes of `plugin.toml` and the script together (the manifest alone if it is incompatible).
    pub size: u64,
    pub change: InstallChange,
    pub source: InstallSource,
    dir: PathBuf,
    /// For a plugin that was downloaded: the directory the files were put in, which goes away
    /// together with the last copy of the inspection.
    _download: Option<Arc<Staging>>,
}

impl PluginStore {
    /// Reads `plugin.toml` from `dir` and checks that the script exists and is not too large.
    /// Fails if the folder, the manifest or the script cannot be used; an incompatible manifest
    /// is a successful result with [`Compatibility::Incompatible`], so the reasons can be shown.
    /// A manifest for an unsupported `api_version` cannot be read at all and is an error.
    pub fn inspect_local(&self, dir: &Path) -> Result<Inspection, Error> {
        let mut inspection = self.inspect_dir(dir)?;
        inspection.source = InstallSource::Local {
            path: std::path::absolute(dir)
                .unwrap_or_else(|_| dir.to_owned())
                .to_string_lossy()
                .into_owned(),
        };
        Ok(inspection)
    }

    /// Writes the files of a plugin that was downloaded into a directory of the store and
    /// inspects them there. The files live as long as the inspection (or a copy of it) does.
    /// `script` is `None` for a plugin that is not going to be installed anyway.
    pub(crate) fn inspect_downloaded(
        &self,
        manifest: &[u8],
        script: Option<(&str, &[u8])>,
        source: InstallSource,
    ) -> Result<Inspection, Error> {
        fs::create_dir_all(&self.plugins_dir)
            .map_err(|e| Error::io("create plugins directory", &self.plugins_dir, e))?;
        sweep_stale(&self.plugins_dir, STALE_AFTER);
        let staging = Staging::create(&self.plugins_dir)?;
        write_file(&staging.path().join(MANIFEST_FILE), manifest)?;
        if let Some((name, bytes)) = script {
            // The name is the manifest's `entry`; a name that is not a plain file name is refused
            // here, not left to the file system.
            check_entry(name).map_err(|m| Error::invalid_data("copy plugin", staging.path(), m))?;
            write_file(&staging.path().join(name), bytes)?;
        }
        let mut inspection = self.inspect_dir(staging.path())?;
        inspection.source = source;
        inspection._download = Some(Arc::new(staging));
        Ok(inspection)
    }

    /// `inspect_local` without the source, which the callers fill in.
    fn inspect_dir(&self, dir: &Path) -> Result<Inspection, Error> {
        let candidate = examine(dir)?;
        let compatibility = store_compatibility(&candidate.manifest);
        let change = match compatibility {
            Compatibility::Compatible => self.change_for(&candidate.manifest),
            Compatibility::Incompatible(_) => InstallChange::New,
        };
        let manifest = candidate.manifest;
        let size = candidate.manifest_len + candidate.script_len.unwrap_or(0);
        Ok(Inspection {
            permissions: permissions(&manifest),
            manifest,
            compatibility,
            size,
            change,
            source: InstallSource::Url { url: String::new() },
            dir: dir.to_owned(),
            _download: None,
        })
    }

    /// Copies the inspected plugin into the store and enables it (a new plugin; for an update
    /// the switch stays as it was). The folder is read again and must still say what was
    /// inspected. The script must match the manifest's `sha256`, if it has one. The files go to a
    /// staging directory first and are renamed into `plugins/<id>/` at the end, so a failure
    /// leaves the previous version intact. Settings the new version still accepts are kept.
    pub fn install(
        &self,
        inspection: &Inspection,
        downgrade: Downgrade,
    ) -> Result<InstalledPlugin, Error> {
        self.install_with(inspection, downgrade, |_| Ok(()))
    }

    /// `before_publish` runs when the staging directory is complete (tests make it fail).
    pub(super) fn install_with(
        &self,
        inspection: &Inspection,
        downgrade: Downgrade,
        before_publish: impl FnOnce(&Path) -> io::Result<()>,
    ) -> Result<InstalledPlugin, Error> {
        let _guard = self.guard();
        let dir = &inspection.dir;
        let candidate = examine(dir)?;
        let manifest = candidate.manifest;
        let id = manifest.id.clone();
        if manifest != inspection.manifest {
            return Err(plugin_error(
                "install plugin",
                &id,
                "the plugin was changed after it was inspected",
            ));
        }
        if let Compatibility::Incompatible(reasons) = store_compatibility(&manifest) {
            return Err(plugin_error(
                "install plugin",
                &id,
                not_compatible(&reasons),
            ));
        }
        let script = read_plain_file(&dir.join(&manifest.entry), MAX_SCRIPT_BYTES)?;
        let sha256 = sha256_hex(&script);
        if let Some(expected) = &manifest.sha256
            && !expected.eq_ignore_ascii_case(&sha256)
        {
            return Err(plugin_error(
                "install plugin",
                &id,
                format!(
                    "{} does not match the sha256 in plugin.toml",
                    manifest.entry
                ),
            ));
        }
        let old = self.read_installed(&id).ok().map(|f| f.manifest);
        let change = classify(old.as_ref(), &manifest);
        if let (Downgrade::Refuse, InstallChange::Downgrade { from, to }) = (downgrade, &change) {
            return Err(plugin_error(
                "install plugin",
                &id,
                format!("replacing {from} with the older {to} needs confirmation"),
            ));
        }
        // Fail on a damaged plugins.toml before touching anything.
        let mut state = state::StateFile::read(&self.state_path)?;

        fs::create_dir_all(&self.plugins_dir)
            .map_err(|e| Error::io("create plugins directory", &self.plugins_dir, e))?;
        sweep_stale(&self.plugins_dir, STALE_AFTER);
        let staging = Staging::create(&self.plugins_dir)?;
        let record = InstallRecord {
            source: inspection.source.clone(),
            version: manifest.version.clone(),
            sha256,
            installed_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
        };
        let record_json = serde_json::to_vec_pretty(&record)
            .map_err(|e| Error::invalid_data("write install record", staging.path(), e))?;
        write_file(
            &staging.path().join(MANIFEST_FILE),
            &candidate.manifest_bytes,
        )?;
        write_file(&staging.path().join(&manifest.entry), &script)?;
        write_file(&staging.path().join(RECORD_FILE), &record_json)?;
        before_publish(staging.path()).map_err(|e| Error::io("copy plugin", staging.path(), e))?;
        self.publish(&staging, &id)?;

        let entry = state
            .plugins
            .entry(id.clone())
            .or_insert_with(|| state::PluginState {
                enabled: true,
                ..Default::default()
            });
        entry.settings =
            state::carried(old.as_ref(), &manifest, std::mem::take(&mut entry.settings));
        state.write(&self.state_path)?;
        self.get(&id)
    }

    pub(crate) fn change_for(&self, manifest: &Manifest) -> InstallChange {
        classify(
            self.read_installed(&manifest.id)
                .ok()
                .map(|f| f.manifest)
                .as_ref(),
            manifest,
        )
    }

    /// Puts the staging directory in place of `plugins/<id>/`. The old directory is moved
    /// aside first and put back if the new one cannot be moved in.
    fn publish(&self, staging: &Staging, id: &str) -> Result<(), Error> {
        let target = self.plugins_dir.join(id);
        let mut backup = None;
        if fs::symlink_metadata(&target).is_ok() {
            let aside = self
                .plugins_dir
                .join(format!("{BACKUP_PREFIX}{}", unique_suffix()));
            fs::rename(&target, &aside).map_err(|e| Error::io("replace plugin", &target, e))?;
            backup = Some(aside);
        }
        if let Err(e) = fs::rename(staging.path(), &target) {
            if let Some(aside) = &backup {
                let _ = fs::rename(aside, &target);
            }
            return Err(Error::io("install plugin", &target, e));
        }
        if let Some(aside) = backup {
            let _ = fs::remove_dir_all(aside);
        }
        Ok(())
    }
}

/// The contract's compatibility plus the store's own rule about reserved names.
pub(crate) fn store_compatibility(manifest: &Manifest) -> Compatibility {
    let mut reasons = manifest.compatibility(&app_version()).reasons().to_vec();
    if RESERVED_IDS.contains(&manifest.id.as_str()) {
        reasons.push(Reason::Invalid {
            field: "id".into(),
            message: "is a reserved file name on Windows".into(),
        });
    }
    if reasons.is_empty() {
        Compatibility::Compatible
    } else {
        Compatibility::Incompatible(reasons)
    }
}

fn permissions(manifest: &Manifest) -> Permissions {
    Permissions {
        domains: manifest.network.domains.clone(),
        call_timeout_s: manifest
            .limits
            .call_timeout_s
            .unwrap_or(DEFAULT_CALL_TIMEOUT_S),
        network_settings: manifest
            .settings
            .iter()
            .filter(|s| s.kind == SettingKind::Url && s.network)
            .map(|s| s.label.clone())
            .collect(),
    }
}

/// `old` is the plugin that is installed now, if it is readable. If its version is not semver
/// (a manifest damaged by hand), the install counts as new.
fn classify(old: Option<&Manifest>, new: &Manifest) -> InstallChange {
    let Some(old) = old else {
        return InstallChange::New;
    };
    let (Ok(from), Ok(to)) = (Version::parse(&old.version), Version::parse(&new.version)) else {
        return InstallChange::New;
    };
    let (from_text, to_text) = (old.version.clone(), new.version.clone());
    match to.cmp(&from) {
        std::cmp::Ordering::Equal => InstallChange::Reinstall { version: to_text },
        std::cmp::Ordering::Greater => InstallChange::Upgrade {
            from: from_text,
            to: to_text,
        },
        std::cmp::Ordering::Less => InstallChange::Downgrade {
            from: from_text,
            to: to_text,
        },
    }
}

struct Candidate {
    manifest: Manifest,
    manifest_bytes: Vec<u8>,
    manifest_len: u64,
    /// Size of the script; known if the manifest names a usable file.
    script_len: Option<u64>,
}

/// Reads the manifest of the folder and looks at the script without reading it. The script is
/// required only for a compatible manifest: for another one `entry` may be anything.
fn examine(dir: &Path) -> Result<Candidate, Error> {
    match fs::metadata(dir) {
        Ok(meta) if meta.is_dir() => {}
        Ok(_) => {
            return Err(Error::invalid_data(
                "inspect plugin",
                dir,
                "not a folder".to_owned(),
            ));
        }
        Err(e) => return Err(Error::io("inspect plugin", dir, e)),
    }
    let manifest_path = dir.join(MANIFEST_FILE);
    let manifest_bytes = read_plain_file(&manifest_path, MAX_MANIFEST_BYTES)?;
    let text = std::str::from_utf8(&manifest_bytes).map_err(|_| {
        Error::invalid_data(
            "read plugin.toml",
            &manifest_path,
            "not UTF-8 text".to_owned(),
        )
    })?;
    let manifest = Manifest::parse(text)
        .map_err(|e| Error::invalid_data("read plugin.toml", &manifest_path, e))?;
    let compatible = manifest.compatibility(&app_version()).is_compatible();
    let script_len = if check_entry(&manifest.entry).is_ok() {
        let script_path = dir.join(&manifest.entry);
        match fs::symlink_metadata(&script_path) {
            Ok(meta) if compatible => Some(checked_script(&script_path, &meta)?),
            Ok(meta) => Some(meta.len()),
            Err(e) if compatible => return Err(Error::io("find plugin script", &script_path, e)),
            Err(_) => None,
        }
    } else {
        None
    };
    Ok(Candidate {
        manifest_len: manifest_bytes.len() as u64,
        manifest,
        manifest_bytes,
        script_len,
    })
}

fn checked_script(path: &Path, meta: &fs::Metadata) -> Result<u64, Error> {
    let invalid = |message: String| Error::invalid_data("inspect plugin script", path, message);
    if meta.file_type().is_symlink() {
        return Err(invalid("symbolic links are not followed".into()));
    }
    if !meta.is_file() {
        return Err(invalid("not a regular file".into()));
    }
    if meta.len() > MAX_SCRIPT_BYTES as u64 {
        return Err(invalid(format!(
            "the script is larger than {MAX_SCRIPT_BYTES} bytes"
        )));
    }
    Ok(meta.len())
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let write = || -> io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        file.write_all(bytes)?;
        file.sync_all()
    };
    write().map_err(|e| Error::io("copy plugin", path, e))
}

/// A directory being filled; removed on drop unless it was moved into place.
#[derive(Debug)]
struct Staging {
    path: PathBuf,
}

impl Staging {
    fn create(plugins_dir: &Path) -> Result<Self, Error> {
        for _ in 0..16 {
            let path = plugins_dir.join(format!("{STAGING_PREFIX}{}", unique_suffix()));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(Error::io("create staging directory", &path, e)),
            }
        }
        Err(Error {
            operation: "create staging directory",
            path: Some(plugins_dir.to_owned()),
            kind: ErrorKind::Conflict,
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        // After a successful rename the path is gone and this does nothing.
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// An unpredictable name, so another program cannot guess where a plugin is being written.
fn unique_suffix() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut hasher = RandomState::new().build_hasher();
    hasher.write_u32(std::process::id());
    hasher.write_u64(NEXT.fetch_add(1, Ordering::Relaxed));
    hasher.write_u128(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    format!("{:016x}", hasher.finish())
}

/// Removes staging and backup directories that a crash left behind.
pub(super) fn sweep_stale(plugins_dir: &Path, older_than: Duration) {
    let Ok(entries) = fs::read_dir(plugins_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with(STAGING_PREFIX) || name.starts_with(BACKUP_PREFIX)) {
            continue;
        }
        let age = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok());
        if age.is_some_and(|age| age >= older_than) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}
