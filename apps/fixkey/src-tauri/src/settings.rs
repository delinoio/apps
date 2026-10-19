//! Versioned settings with atomic publication and fail-closed recovery.
//! This schema deliberately has no input text or correction-history fields.

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// User-selected presentation mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Theme {
    /// Follow the OS.
    #[default]
    System,
    /// Light surfaces.
    Light,
    /// Dark surfaces.
    Dark,
}

pub use crate::localization::UiLanguage;

/// Stable, platform-owned application identities. Display labels are never
/// used for exclusion matching.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "platform", rename_all = "kebab-case", deny_unknown_fields)]
pub enum AppIdentity {
    /// macOS bundle identifier.
    Macos {
        /// Bundle identifier.
        bundle_id: String,
    },
    /// Windows `AppUserModelID`, when present.
    WindowsApp {
        /// Application model identifier.
        app_user_model_id: String,
    },
    /// Windows full executable path, when no `AppUserModelID` exists.
    WindowsExecutable {
        /// Full executable path.
        path: String,
    },
    /// Linux `WM_CLASS` and optional desktop entry.
    Linux {
        /// `WM_CLASS` class.
        class: String,
        /// Desktop-entry identifier, when available.
        desktop_entry_id: Option<String>,
    },
}

/// One retained exclusion. Resolution status is derived by the platform,
/// never used to silently delete a saved exclusion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExcludedApp {
    /// Stable matching key.
    pub identity: AppIdentity,
    /// Presentation label from the application picker.
    pub display_name: String,
}

/// All persisted desktop settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Independent persisted user switches are not runtime states."
)]
pub struct Settings {
    /// Settings schema, independent from application version.
    pub schema_version: u32,
    /// User-requested pause.
    pub paused: bool,
    /// Enable Korean Dubeolsik.
    pub korean_enabled: bool,
    /// Enable US English QWERTY.
    pub english_enabled: bool,
    /// Additional stable dictionary/layout IDs; old settings default to none.
    #[serde(default)]
    pub additional_languages: Vec<String>,
    /// Launch after login; opt-in.
    pub login_startup: bool,
    /// UI language.
    pub ui_language: UiLanguage,
    /// Theme.
    pub theme: Theme,
    /// Registered manual picker shortcut; absent by default.
    pub shortcut: Option<String>,
    /// Applications excluded from correction.
    pub excluded_apps: Vec<ExcludedApp>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: 2,
            paused: false,
            korean_enabled: true,
            english_enabled: true,
            additional_languages: Vec::new(),
            login_startup: false,
            ui_language: UiLanguage::System,
            theme: Theme::System,
            shortcut: None,
            excluded_apps: Vec::new(),
        }
    }
}

impl Settings {
    /// Validates persisted identities and schema values before OS changes.
    ///
    /// # Errors
    ///
    /// Returns [`SettingsError::Invalid`] for an unsupported value.
    pub fn validate(&self) -> Result<(), SettingsError> {
        validate(self)
    }

    /// Returns the engine's enabled layouts without exposing UI state to it.
    #[must_use]
    pub fn languages(&self) -> fixkey_core::LanguageSet {
        let mut languages = fixkey_core::LanguageSet::default()
            .with(fixkey_core::Language::KoDubeolsik, self.korean_enabled)
            .with(fixkey_core::Language::EnUsQwerty, self.english_enabled);
        for id in &self.additional_languages {
            if let Some(language) = fixkey_core::Language::from_id(id) {
                languages = languages.with(language, true);
            }
        }
        languages
    }
}

/// Stable error codes that contain no user configuration or file contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(rename_all = "kebab-case")]
pub enum SettingsError {
    /// Existing settings could not be read or parsed.
    #[error("settings-unreadable")]
    Unreadable,
    /// A newer format must be preserved for recovery.
    #[error("settings-newer-schema")]
    NewerSchema,
    /// Atomic publication failed.
    #[error("settings-save-failed")]
    SaveFailed,
    /// The requested value does not conform to the schema.
    #[error("settings-invalid")]
    Invalid,
    /// No readable previous-version backup exists.
    #[error("settings-backup-unavailable")]
    BackupUnavailable,
}

/// Settings plus their storage health. Loading errors retain defaults only
/// in memory and block writes until the user explicitly restores a backup.
pub struct SettingsStore {
    path: PathBuf,
    current: Settings,
    error: Option<SettingsError>,
}

impl SettingsStore {
    /// Loads the current OS user's settings file without changing it.
    #[must_use]
    pub fn load(path: PathBuf) -> Self {
        let result = match fs::read(&path) {
            Ok(bytes) => decode(&bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(_) => Err(SettingsError::Unreadable),
        };
        match result {
            Ok(current) => Self {
                path,
                current,
                error: None,
            },
            Err(error) => {
                tracing::warn!(reason = %error, "fixkey settings load failed");
                Self {
                    path,
                    current: Settings::default(),
                    error: Some(error),
                }
            }
        }
    }

    /// Returns the effective settings.
    #[must_use]
    pub const fn current(&self) -> &Settings {
        &self.current
    }

    /// Returns a stable load failure code.
    #[must_use]
    pub const fn error(&self) -> Option<SettingsError> {
        self.error
    }

    /// Publishes a complete validated value and then changes in-memory state.
    ///
    /// # Errors
    ///
    /// Returns a load, validation or publication error. The previous value
    /// remains effective after failure.
    pub fn save(&mut self, settings: Settings) -> Result<(), SettingsError> {
        if let Some(error) = self.error {
            return Err(error);
        }
        validate(&settings)?;
        let previous = match fs::read(&self.path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err(SettingsError::SaveFailed),
        };
        if let Some(bytes) = &previous {
            // Another version or an external edit may have changed the file
            // since startup. Never bypass recovery by overwriting that data.
            decode(bytes)?;
        }
        if let Some(bytes) = previous
            && serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .and_then(|value| {
                    value
                        .get("schemaVersion")
                        .and_then(serde_json::Value::as_u64)
                })
                == Some(1)
        {
            // Version-one binaries reject new language IDs and fields. Publish
            // their exact readable file before the first version-two write.
            // A failed publication may retry with the same backup bytes.
            let directory = self.path.parent().ok_or(SettingsError::SaveFailed)?;
            let backup = directory.join("settings.v1.bak");
            let mut temporary = tempfile::NamedTempFile::new_in(directory)
                .map_err(|_| SettingsError::SaveFailed)?;
            temporary
                .write_all(&bytes)
                .and_then(|()| temporary.as_file().sync_all())
                .map_err(|_| SettingsError::SaveFailed)?;
            if let Err(error) = temporary.persist_noclobber(&backup)
                && (error.error.kind() != std::io::ErrorKind::AlreadyExists
                    || fs::read(&backup).ok().as_deref() != Some(bytes.as_slice()))
            {
                return Err(SettingsError::SaveFailed);
            }
        }
        atomic_write(&self.path, &settings)?;
        self.current = settings;
        tracing::info!(reason = "settings-saved", "fixkey settings changed");
        Ok(())
    }

    /// Explicitly restores the supported version's backup. The unreadable
    /// current file is retained separately for manual recovery.
    ///
    /// # Errors
    ///
    /// Returns an error if the backup cannot be read or publication fails.
    pub fn restore_backup(&mut self) -> Result<(), SettingsError> {
        let directory = self.path.parent().ok_or(SettingsError::SaveFailed)?;
        let backup = fs::read(directory.join("settings.v1.bak"))
            .map_err(|_| SettingsError::BackupUnavailable)?;
        let restored = decode(&backup).map_err(|_| SettingsError::BackupUnavailable)?;
        if self.path.exists() {
            let bytes = fs::read(&self.path).map_err(|_| SettingsError::SaveFailed)?;
            preserve_unreadable(directory, &bytes)?;
        }
        atomic_write(&self.path, &restored)?;
        self.current = restored;
        self.error = None;
        Ok(())
    }
}

fn preserve_unreadable(directory: &Path, bytes: &[u8]) -> Result<(), SettingsError> {
    // Publish a complete recovery file without overwriting an earlier one.
    // An identical file permits retry after settings publication failed.
    let path = directory.join("settings.unreadable.bak");
    let mut copy =
        tempfile::NamedTempFile::new_in(directory).map_err(|_| SettingsError::SaveFailed)?;
    copy.write_all(bytes)
        .and_then(|()| copy.as_file().sync_all())
        .map_err(|_| SettingsError::SaveFailed)?;
    match copy.persist_noclobber(&path) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            if fs::read(&path).map_err(|_| SettingsError::SaveFailed)? == bytes {
                Ok(())
            } else {
                Err(SettingsError::SaveFailed)
            }
        }
        Err(_) => Err(SettingsError::SaveFailed),
    }
}

fn decode(bytes: &[u8]) -> Result<Settings, SettingsError> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| SettingsError::Unreadable)?;
    if value
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|v| v > 2)
    {
        return Err(SettingsError::NewerSchema);
    }
    if value
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        == Some(1)
        && (value.get("additionalLanguages").is_some()
            || !matches!(
                value.get("uiLanguage").and_then(serde_json::Value::as_str),
                Some("system" | "en" | "ko")
            ))
    {
        return Err(SettingsError::Unreadable);
    }
    let mut settings: Settings =
        serde_json::from_value(value).map_err(|_| SettingsError::Unreadable)?;
    if settings.schema_version == 1 {
        settings.schema_version = 2;
    }
    validate(&settings)?;
    Ok(settings)
}

fn validate(settings: &Settings) -> Result<(), SettingsError> {
    if settings.schema_version != 2
        || settings
            .shortcut
            .as_ref()
            .is_some_and(|key| key.is_empty() || key.len() > 128)
    {
        return Err(SettingsError::Invalid);
    }
    for (index, id) in settings.additional_languages.iter().enumerate() {
        if !matches!(
            fixkey_core::Language::from_id(id),
            Some(fixkey_core::Language::Additional(_))
        ) || settings.additional_languages[..index].contains(id)
        {
            return Err(SettingsError::Invalid);
        }
    }
    for (index, app) in settings.excluded_apps.iter().enumerate() {
        let valid = match &app.identity {
            AppIdentity::Macos { bundle_id } => !bundle_id.trim().is_empty(),
            AppIdentity::WindowsApp { app_user_model_id } => !app_user_model_id.trim().is_empty(),
            AppIdentity::WindowsExecutable { path } => {
                // Validate Windows paths even in cross-platform fixture tests.
                path.starts_with("\\\\") || path.as_bytes().get(1..3) == Some(b":\\")
            }
            AppIdentity::Linux {
                class,
                desktop_entry_id,
            } => {
                !class.trim().is_empty()
                    && desktop_entry_id
                        .as_ref()
                        .is_none_or(|id| !id.trim().is_empty())
            }
        };
        if !valid
            || app.display_name.trim().is_empty()
            || settings.excluded_apps[..index]
                .iter()
                .any(|other| other.identity == app.identity)
        {
            return Err(SettingsError::Invalid);
        }
    }
    Ok(())
}

fn atomic_write(path: &Path, settings: &Settings) -> Result<(), SettingsError> {
    let directory = path.parent().ok_or(SettingsError::SaveFailed)?;
    fs::create_dir_all(directory).map_err(|_| SettingsError::SaveFailed)?;
    let bytes = serde_json::to_vec_pretty(settings).map_err(|_| SettingsError::SaveFailed)?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(directory).map_err(|_| SettingsError::SaveFailed)?;
    temporary
        .write_all(&bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|_| SettingsError::SaveFailed)?;
    temporary
        .persist(path)
        .map_err(|_| SettingsError::SaveFailed)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saving_never_overwrites_a_newer_file_that_appeared_after_startup() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        let mut store = SettingsStore::load(path.clone());
        let bytes = br#"{"schemaVersion":3,"future":true}"#;
        fs::write(&path, bytes).unwrap();
        assert_eq!(
            store.save(Settings::default()),
            Err(SettingsError::NewerSchema)
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn legacy_settings_are_loaded_without_writes_and_backed_up_before_migration() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        let mut legacy = serde_json::to_value(Settings::default()).unwrap();
        legacy["schemaVersion"] = 1.into();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("additionalLanguages");
        let bytes = serde_json::to_vec(&legacy).unwrap();
        fs::write(&path, &bytes).unwrap();
        let mut store = SettingsStore::load(path.clone());
        assert!(store.error().is_none());
        assert_eq!(store.current().schema_version, 2);
        assert_eq!(fs::read(&path).unwrap(), bytes);
        let mut next = store.current().clone();
        next.additional_languages = vec!["ru-xkb-ru".into()];
        next.ui_language = UiLanguage::Ru;
        store.save(next.clone()).unwrap();
        assert_eq!(
            fs::read(temp.path().join("settings.v1.bak")).unwrap(),
            bytes
        );
        assert_eq!(SettingsStore::load(path).current(), &next);
    }

    #[test]
    fn additional_languages_reject_unknown_legacy_and_duplicate_identifiers() {
        for languages in [
            vec!["future"],
            vec!["ko-dubeolsik"],
            vec!["ru-xkb-ru", "ru-xkb-ru"],
        ] {
            let settings = Settings {
                additional_languages: languages.into_iter().map(str::to_owned).collect(),
                ..Settings::default()
            };
            assert_eq!(settings.validate(), Err(SettingsError::Invalid));
        }
    }

    #[test]
    fn defaults_are_offline_and_startup_is_opt_in() {
        let settings = Settings::default();
        assert!(settings.korean_enabled && settings.english_enabled);
        assert!(!settings.login_startup);
        assert_eq!(settings.shortcut, None);
        assert_eq!(settings.excluded_apps, Vec::new());
    }

    #[test]
    fn immediate_changes_survive_reload_and_failure_keeps_previous_value() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        let mut store = SettingsStore::load(path.clone());
        let changed = Settings {
            theme: Theme::Dark,
            ..Settings::default()
        };
        store.save(changed.clone()).unwrap();
        assert_eq!(SettingsStore::load(path.clone()).current(), &changed);
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        assert_eq!(
            store.save(Settings::default()),
            Err(SettingsError::SaveFailed)
        );
        assert_eq!(store.current(), &changed);
    }

    #[test]
    fn unreadable_and_newer_settings_are_never_overwritten() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        for (bytes, expected) in [
            (b"broken".as_slice(), SettingsError::Unreadable),
            (
                b"{\"schemaVersion\":3,\"future\":true}".as_slice(),
                SettingsError::NewerSchema,
            ),
        ] {
            fs::write(&path, bytes).unwrap();
            let mut store = SettingsStore::load(path.clone());
            assert_eq!(store.error(), Some(expected));
            assert_eq!(store.save(Settings::default()), Err(expected));
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn restoring_backup_preserves_unreadable_current_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        fs::write(&path, "future data").unwrap();
        let restored = Settings {
            paused: true,
            ..Settings::default()
        };
        fs::write(
            temp.path().join("settings.v1.bak"),
            serde_json::to_vec(&restored).unwrap(),
        )
        .unwrap();
        let mut store = SettingsStore::load(path.clone());
        store.restore_backup().unwrap();
        assert_eq!(store.current(), &restored);
        assert_eq!(store.error(), None);
        assert_eq!(
            fs::read_to_string(temp.path().join("settings.unreadable.bak")).unwrap(),
            "future data"
        );
        assert_eq!(SettingsStore::load(path).current(), &restored);
    }

    #[test]
    fn restoration_retries_reuse_identical_recovery_files_without_overwriting_conflicts() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("settings.json");
        let recovery = temp.path().join("settings.unreadable.bak");
        fs::write(&path, "future data").unwrap();
        fs::write(
            temp.path().join("settings.v1.bak"),
            serde_json::to_vec(&Settings::default()).unwrap(),
        )
        .unwrap();
        fs::write(&recovery, "other data").unwrap();
        let mut store = SettingsStore::load(path.clone());
        assert_eq!(store.restore_backup(), Err(SettingsError::SaveFailed));
        assert_eq!(fs::read_to_string(&path).unwrap(), "future data");
        assert_eq!(fs::read_to_string(&recovery).unwrap(), "other data");
        fs::write(&recovery, "future data").unwrap();
        store.restore_backup().unwrap();
        assert_eq!(store.error(), None);
        assert_eq!(fs::read_to_string(&recovery).unwrap(), "future data");
    }

    #[test]
    fn identity_validation_rejects_duplicate_and_unstable_keys() {
        let app = ExcludedApp {
            identity: AppIdentity::Macos {
                bundle_id: "io.example.editor".into(),
            },
            display_name: "Editor".into(),
        };
        let settings = Settings {
            excluded_apps: vec![app.clone(), app],
            ..Settings::default()
        };
        assert_eq!(validate(&settings), Err(SettingsError::Invalid));
        let settings = Settings {
            excluded_apps: vec![ExcludedApp {
                identity: AppIdentity::WindowsExecutable {
                    path: "editor.exe".into(),
                },
                display_name: "Editor".into(),
            }],
            ..Settings::default()
        };
        assert_eq!(validate(&settings), Err(SettingsError::Invalid));
    }
}
