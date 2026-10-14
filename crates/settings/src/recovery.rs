use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryDocument {
    pub path: Option<PathBuf>,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recovery {
    pub documents: Vec<RecoveryDocument>,
}

impl Recovery {
    pub fn load(path: &Path) -> Result<Self, String> {
        if !path.try_exists().map_err(|error| error.to_string())? {
            return Ok(Self::default());
        }
        let file = ligature_fs::read(path).map_err(|error| error.to_string())?;
        let recovery: Self = serde_json::from_str(&file.text).map_err(|error| format!("Invalid recovery data: {error}"))?;
        if recovery
            .documents
            .iter()
            .any(|document| document.path.as_ref().is_some_and(|path| !path.is_absolute()))
        {
            return Err("Recovery document paths must be absolute".into());
        }
        Ok(recovery)
    }
}

#[derive(Clone)]
pub struct RecoveryStore {
    path: PathBuf,
    generation: Arc<AtomicU64>,
    writer: Arc<Mutex<()>>,
}

impl RecoveryStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            generation: Arc::default(),
            writer: Arc::default(),
        }
    }

    pub fn default_path() -> Option<PathBuf> {
        super::config_directory().map(|directory| directory.join("recovery.json"))
    }

    pub fn load(&self) -> Result<Recovery, String> {
        Recovery::load(&self.path)
    }

    pub fn prepare(&self, recovery: Recovery) -> RecoveryWrite {
        RecoveryWrite {
            store: self.clone(),
            generation: self.generation.fetch_add(1, Ordering::SeqCst) + 1,
            recovery,
        }
    }
}

pub struct RecoveryWrite {
    store: RecoveryStore,
    generation: u64,
    recovery: Recovery,
}

impl RecoveryWrite {
    pub fn save(self) -> Result<(), String> {
        let _guard = self.store.writer.lock().unwrap();
        if self.generation != self.store.generation.load(Ordering::SeqCst) {
            return Ok(());
        }
        if self.recovery.documents.is_empty() {
            return match std::fs::remove_file(&self.store.path) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(format!("Could not clear recovery data: {error}")),
            };
        }
        super::persist(&self.store.path, &self.recovery)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_round_trips_untitled_unicode_and_crlf_without_writing_sources() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("chapter one.tex");
        std::fs::write(&source, "original").unwrap();
        let store = RecoveryStore::new(directory.path().join("config/recovery.json"));
        let recovery = Recovery {
            documents: vec![
                RecoveryDocument {
                    path: Some(source.clone()),
                    text: "unsaved 猫\r\n".into(),
                },
                RecoveryDocument {
                    path: None,
                    text: "untitled 😀".into(),
                },
            ],
        };
        store.prepare(recovery.clone()).save().unwrap();
        assert_eq!(store.load().unwrap(), recovery);
        assert_eq!(std::fs::read_to_string(source).unwrap(), "original");
    }

    #[test]
    fn obsolete_writes_cannot_resurrect_saved_or_discarded_snapshots() {
        let directory = tempfile::tempdir().unwrap();
        let store = RecoveryStore::new(directory.path().join("recovery.json"));
        let snapshot = || Recovery {
            documents: vec![RecoveryDocument {
                path: None,
                text: "draft".into(),
            }],
        };
        let obsolete = store.prepare(snapshot());
        store.prepare(snapshot()).save().unwrap();
        store.prepare(Recovery::default()).save().unwrap();
        obsolete.save().unwrap();
        assert_eq!(store.load().unwrap(), Recovery::default());
        assert!(!store.path.exists());
    }

    #[test]
    fn corrupt_recovery_is_reported_and_preserved() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("recovery.json");
        std::fs::write(&path, "{broken").unwrap();
        let store = RecoveryStore::new(path.clone());
        assert!(store.load().is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "{broken");
    }
}
