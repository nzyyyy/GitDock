use crate::models::{RepositoryRecord, Settings};
use serde::{Deserialize, Serialize};
use std::{fs, io::Write, path::PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub config: AppConfig,
}

impl ConfigStore {
    pub fn load(path: PathBuf) -> Result<Self, String> {
        let config = if path.exists() {
            let bytes = fs::read(&path).map_err(|e| format!("Cannot read settings: {e}"))?;
            serde_json::from_slice(&bytes)
                .map_err(|e| format!("Settings are damaged and were not overwritten: {e}"))?
        } else {
            AppConfig::default()
        };
        Ok(Self { path, config })
    }

    pub fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let tmp = self.path.with_extension("json.tmp");
        let data = serde_json::to_vec_pretty(&self.config).map_err(|e| e.to_string())?;
        let mut file = fs::File::create(&tmp).map_err(|e| e.to_string())?;
        file.write_all(&data)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }
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
}
