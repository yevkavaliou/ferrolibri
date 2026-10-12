use rulibre_core::{Error, config};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewMode {
    #[default]
    Grid,
    List,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    pub text_size: u16,
    pub cover_size: u16,
    pub details_width: u16,
    pub view: ViewMode,
    pub sidebar_visible: bool,
    pub details_visible: bool,
    /// Window opacity in percent: 100 is opaque, lower values let the desktop
    /// show through (Windows only).
    pub window_opacity: u8,
    /// Blur the desktop behind a translucent window.
    pub window_blur: bool,
    pub prefer_kepub: bool,
    pub export_folder: Option<PathBuf>,
}
impl Default for UiSettings {
    fn default() -> Self {
        Self {
            text_size: 16,
            cover_size: 160,
            details_width: 300,
            view: ViewMode::Grid,
            sidebar_visible: true,
            details_visible: true,
            window_opacity: 70,
            window_blur: true,
            prefer_kepub: true,
            export_folder: None,
        }
    }
}
impl UiSettings {
    pub fn normalize(&mut self) {
        self.text_size = self.text_size.clamp(14, 20);
        self.cover_size = self.cover_size.clamp(120, 200);
        self.details_width = self.details_width.clamp(240, 480);
        self.window_opacity = self.window_opacity.clamp(10, 100);
    }
    pub fn path() -> Result<PathBuf, Error> {
        Ok(config::Config::path()?.with_file_name("ui.toml"))
    }
    pub fn load_from(path: &Path) -> Result<Self, Error> {
        match fs::read_to_string(path) {
            Ok(content) => {
                let mut settings: Self = toml::from_str(&content)
                    .map_err(|e| Error::invalid_data("parse UI settings", path, e))?;
                settings.normalize();
                Ok(settings)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(Error::io("read UI settings", path, e)),
        }
    }
    pub fn save_to(&self, path: &Path) -> Result<(), Error> {
        let content = toml::to_string_pretty(self)
            .map_err(|e| Error::invalid_data("serialize UI settings", path, e))?;
        config::write_atomic(path, content.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_round_trip_and_replace() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/ui.toml");
        assert_eq!(UiSettings::load_from(&path).unwrap(), UiSettings::default());
        let settings = UiSettings {
            view: ViewMode::List,
            sidebar_visible: false,
            details_visible: false,
            window_opacity: 30,
            window_blur: false,
            prefer_kepub: false,
            export_folder: Some(dir.path().join("Экспорт książki")),
            text_size: 18,
            cover_size: 200,
            details_width: 420,
        };
        settings.save_to(&path).unwrap();
        assert_eq!(UiSettings::load_from(&path).unwrap(), settings);
        UiSettings::default().save_to(&path).unwrap();
        assert_eq!(UiSettings::load_from(&path).unwrap(), UiSettings::default());
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }
    #[test]
    fn corrupt_file_is_reported_and_preserved() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ui.toml");
        fs::write(&path, "view = 'invalid'").unwrap();
        assert!(UiSettings::load_from(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "view = 'invalid'");
    }
    #[test]
    fn new_fields_use_defaults() {
        let settings: UiSettings = toml::from_str("sidebar_visible = false").unwrap();
        assert!(!settings.sidebar_visible);
        assert_eq!(settings.window_opacity, 70);
        assert!(settings.window_blur);
        assert_eq!(settings.text_size, 16);
        assert_eq!(settings.cover_size, 160);
        assert_eq!(settings.details_width, 300);
    }
    #[test]
    fn malformed_appearance_ranges_are_bounded_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ui.toml");
        fs::write(
            &path,
            "text_size = 0\ncover_size = 60000\ndetails_width = 0",
        )
        .unwrap();
        let settings = UiSettings::load_from(&path).unwrap();
        assert_eq!(settings.text_size, 14);
        assert_eq!(settings.cover_size, 200);
        assert_eq!(settings.details_width, 240);
    }
    #[test]
    fn failed_replacement_cleans_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ui.toml");
        fs::create_dir(&path).unwrap();
        fs::write(path.join("keep"), b"previous data").unwrap();
        assert!(UiSettings::default().save_to(&path).is_err());
        assert_eq!(fs::read(path.join("keep")).unwrap(), b"previous data");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[cfg(windows)]
    #[test]
    fn readonly_settings_file_keeps_previous_bytes_on_failed_save() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ui.toml");
        let previous = UiSettings::default();
        previous.save_to(&path).unwrap();
        let bytes = fs::read(&path).unwrap();
        let original = fs::metadata(&path).unwrap().permissions();
        let mut readonly = original.clone();
        readonly.set_readonly(true);
        fs::set_permissions(&path, readonly).unwrap();
        let changed = UiSettings {
            window_opacity: 30,
            window_blur: false,
            ..previous.clone()
        };
        let result = changed.save_to(&path);
        fs::set_permissions(&path, original).unwrap();
        assert!(result.is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(UiSettings::load_from(&path).unwrap(), previous);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
