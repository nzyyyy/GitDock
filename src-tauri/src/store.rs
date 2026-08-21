use crate::models::{RepositoryRecord, Settings};
use serde::{Deserialize, Serialize};
use std::{fs, io::Write, path::PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    pub version: u32,
    pub next_repository_id: u64,
    pub settings: Settings,
    pub repositories: Vec<RepositoryRecord>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: 1,
            next_repository_id: 1,
            settings: Settings::default(),
            repositories: Vec::new(),
        }
    }
}

pub struct ConfigStore {
    path: PathBuf,
    config: AppConfig,
}

impl ConfigStore {
    pub fn load(path: PathBuf) -> Result<Self, String> {
        let config = if path.exists() {
            let bytes = fs::read(&path).map_err(|e| format!("Cannot read settings: {e}"))?;
            decode_config(&bytes).map_err(|error| {
                let backup = path.with_extension("json.bak");
                let valid_backup = fs::read(&backup)
                    .ok()
                    .and_then(|bytes| decode_config(&bytes).ok())
                    .is_some();
                if valid_backup {
                    format!("{error} Recoverable backup: {}", backup.display())
                } else {
                    error
                }
            })?
        } else {
            AppConfig::default()
        };
        Ok(Self { path, config })
    }

    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    pub fn update<T>(
        &mut self,
        change: impl FnOnce(&mut AppConfig) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut next = self.config.clone();
        let result = change(&mut next)?;
        self.save(&next)?;
        self.config = next;
        Ok(result)
    }

    fn save(&self, config: &AppConfig) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        if self.path.exists() {
            let existing =
                fs::read(&self.path).map_err(|e| format!("Cannot read settings: {e}"))?;
            decode_config(&existing)?;
            write_atomic(&self.path.with_extension("json.bak"), &existing)?;
        }
        let data = serde_json::to_vec_pretty(config).map_err(|e| e.to_string())?;
        write_atomic(&self.path, &data)
    }
}

fn decode_config(bytes: &[u8]) -> Result<AppConfig, String> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|e| format!("Settings are damaged and were not overwritten: {e}"))?;
    match value.get("version").and_then(serde_json::Value::as_u64) {
        Some(1) => serde_json::from_value(value)
            .map_err(|e| format!("Settings are damaged and were not overwritten: {e}")),
        Some(version) => Err(format!("Settings version {version} is not supported.")),
        None => Err("Settings version is missing.".into()),
    }
}

fn write_atomic(path: &std::path::Path, data: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension(format!(
        "{}.tmp",
        path.extension()
            .and_then(|value| value.to_str())
            .unwrap_or("json")
    ));
    let mut file = fs::File::create(&tmp).map_err(|e| e.to_string())?;
    file.write_all(data)
        .and_then(|_| file.sync_all())
        .map_err(|e| e.to_string())?;
    fs::rename(tmp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_to_replace_invalid_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, b"not json").unwrap();
        assert!(ConfigStore::load(path.clone()).is_err());
        assert_eq!(fs::read(path).unwrap(), b"not json");
    }

    #[test]
    fn defaults_legacy_config_to_english() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, br#"{"version":1,"nextRepositoryId":1,"settings":{"gitPath":null,"selectedRepositoryId":null,"leftWidth":240,"rightWidth":360,"outputHeight":190},"repositories":[]}"#).unwrap();
        let store = ConfigStore::load(path).unwrap();
        assert_eq!(
            store.config().settings.language,
            crate::models::Language::English
        );
    }

    #[test]
    fn rejects_unknown_config_versions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, br#"{"version":2}"#).unwrap();
        assert!(ConfigStore::load(path).err().unwrap().contains("version 2"));
    }

    #[test]
    fn backs_up_the_previous_valid_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut store = ConfigStore::load(path.clone()).unwrap();
        store.update(|_| Ok(())).unwrap();
        let previous = fs::read(&path).unwrap();
        store
            .update(|config| {
                config.next_repository_id = 2;
                Ok(())
            })
            .unwrap();
        assert_eq!(fs::read(path.with_extension("json.bak")).unwrap(), previous);
        assert_eq!(
            ConfigStore::load(path).unwrap().config().next_repository_id,
            2
        );
    }

    #[test]
    fn rejected_change_leaves_config_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = ConfigStore::load(dir.path().join("config.json")).unwrap();
        let previous = store.config().clone();
        assert!(store
            .update(|config| {
                config.next_repository_id = 2;
                Err::<(), _>("rejected".into())
            })
            .is_err());
        assert_eq!(store.config(), &previous);
    }

    #[test]
    fn refuses_to_overwrite_a_config_damaged_after_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut store = ConfigStore::load(path.clone()).unwrap();
        let previous = store.config().clone();
        fs::write(&path, b"damaged").unwrap();
        assert!(store
            .update(|config| {
                config.next_repository_id = 2;
                Ok(())
            })
            .is_err());
        assert_eq!(store.config(), &previous);
        assert_eq!(fs::read(path).unwrap(), b"damaged");
    }
}
