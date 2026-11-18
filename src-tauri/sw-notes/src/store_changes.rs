use std::fs::Metadata;
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::UNIX_EPOCH;

use crate::store::{SessionStore, SESSION_EXT};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionChange {
    Saved(String),
    Removed(String),
}

pub type SessionChangeObserver = Arc<dyn Fn(SessionChange) + Send + Sync>;

#[derive(Default)]
pub(crate) struct ChangeObserverSlot(RwLock<Option<SessionChangeObserver>>);

impl ChangeObserverSlot {
    fn replace(&self, observer: Option<SessionChangeObserver>) {
        let mut slot = self.0.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = observer;
    }

    pub(crate) fn notify(&self, change: SessionChange) {
        let observer = self
            .0
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(observer) = observer {
            observer(change);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionFileStat {
    pub session_id: String,
    pub len: u64,
    pub modified_ms: u64,
}

impl SessionStore {
    pub fn set_change_observer(&self, observer: Option<SessionChangeObserver>) {
        self.changes.replace(observer);
    }

    pub fn session_file_stats(&self) -> Vec<SessionFileStat> {
        let Ok(entries) = std::fs::read_dir(self.dir()) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let session_id = finished_session_id(&path)?;
                let metadata = entry.metadata().ok().filter(Metadata::is_file)?;
                Some(stat_of(session_id, &metadata))
            })
            .collect()
    }

    pub fn session_file_stat(&self, session_id: &str) -> Option<SessionFileStat> {
        let path = self.path_for(session_id).ok()?;
        let metadata = std::fs::metadata(path).ok().filter(Metadata::is_file)?;
        Some(stat_of(session_id.to_string(), &metadata))
    }
}

fn finished_session_id(path: &Path) -> Option<String> {
    if path.extension().and_then(|extension| extension.to_str()) != Some(SESSION_EXT) {
        return None;
    }
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .map(str::to_string)
}

fn stat_of(session_id: String, metadata: &Metadata) -> SessionFileStat {
    SessionFileStat {
        session_id,
        len: metadata.len(),
        modified_ms: modified_ms(metadata),
    }
}

fn modified_ms(metadata: &Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|since_epoch| u64::try_from(since_epoch.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "store_changes_tests.rs"]
mod tests;
