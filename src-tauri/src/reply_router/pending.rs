use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

const PENDING_CAPACITY: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReserveResult {
    Reserved,
    Duplicate,
    Unregistered,
    SessionMismatch,
}

#[derive(Default)]
struct PendingInner {
    pending: HashMap<String, String>,
    order: VecDeque<String>,
    reserved: HashSet<String>,
    finalized: HashSet<String>,
    finalized_order: VecDeque<String>,
    last_activity: HashMap<String, Instant>,
    timeout_claimed: HashSet<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeoutCheck {
    StillFresh { remaining: Duration },
    Claimed,
    NoLongerTracked,
}

#[derive(Serialize, Deserialize, Default)]
struct PersistState {
    pending: Vec<(String, String)>,
    finalized: Vec<String>,
}

pub struct PendingReplies {
    inner: std::sync::Mutex<PendingInner>,
    path: Option<PathBuf>,
}

impl Default for PendingReplies {
    fn default() -> Self {
        Self {
            inner: std::sync::Mutex::new(PendingInner::default()),
            path: None,
        }
    }
}

impl PendingReplies {
    pub fn load(path: PathBuf) -> Self {
        let mut inner = PendingInner::default();
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(state) = serde_json::from_str::<PersistState>(&raw) {
                for (task_id, session_id) in state.pending {
                    if !inner.pending.contains_key(&task_id) {
                        inner.pending.insert(task_id.clone(), session_id);
                        inner.order.push_back(task_id);
                    }
                }
                for task_id in state.finalized {
                    if inner.finalized.insert(task_id.clone()) {
                        inner.finalized_order.push_back(task_id);
                    }
                }
            }
        }
        Self {
            inner: std::sync::Mutex::new(inner),
            path: Some(path),
        }
    }

    pub fn register(&self, task_id: String, session_id: String) {
        if let Ok(mut guard) = self.inner.lock() {
            if !guard.pending.contains_key(&task_id) {
                guard.pending.insert(task_id.clone(), session_id);
                guard.last_activity.insert(task_id.clone(), Instant::now());
                guard.order.push_back(task_id);
                while guard.order.len() > PENDING_CAPACITY {
                    if let Some(old) = guard.order.pop_front() {
                        guard.pending.remove(&old);
                        guard.reserved.remove(&old);
                        guard.last_activity.remove(&old);
                    }
                }
            }
            self.persist(&guard);
        }
    }

    pub fn touch(&self, task_id: &str) {
        if let Ok(mut guard) = self.inner.lock() {
            if guard.timeout_claimed.contains(task_id) {
                return;
            }
            if guard.pending.contains_key(task_id) {
                guard
                    .last_activity
                    .insert(task_id.to_string(), Instant::now());
            }
        }
    }

    #[cfg(test)]
    pub fn since_last_activity(&self, task_id: &str) -> Option<Duration> {
        let guard = self.inner.lock().ok()?;
        guard.last_activity.get(task_id).map(|t| t.elapsed())
    }

    pub fn check_or_claim_timeout(&self, task_id: &str, timeout: Duration) -> TimeoutCheck {
        let Ok(mut guard) = self.inner.lock() else {
            return TimeoutCheck::NoLongerTracked;
        };
        let Some(last_activity) = guard.last_activity.get(task_id).copied() else {
            return TimeoutCheck::NoLongerTracked;
        };
        let elapsed = last_activity.elapsed();
        if elapsed < timeout {
            return TimeoutCheck::StillFresh {
                remaining: timeout - elapsed,
            };
        }
        guard.timeout_claimed.insert(task_id.to_string());
        guard.last_activity.remove(task_id);
        TimeoutCheck::Claimed
    }

    fn check_binding(guard: &PendingInner, task_id: &str, session_id: &str) -> ReserveResult {
        if guard.finalized.contains(task_id) {
            return ReserveResult::Duplicate;
        }
        match guard.pending.get(task_id) {
            None => ReserveResult::Unregistered,
            Some(expected) if expected != session_id => ReserveResult::SessionMismatch,
            Some(_) => ReserveResult::Reserved,
        }
    }

    pub fn reserve(&self, task_id: &str, session_id: &str) -> ReserveResult {
        if let Ok(mut guard) = self.inner.lock() {
            match Self::check_binding(&guard, task_id, session_id) {
                ReserveResult::Reserved => {
                    if guard.reserved.insert(task_id.to_string()) {
                        return ReserveResult::Reserved;
                    }
                    return ReserveResult::Duplicate;
                }
                other => return other,
            }
        }
        ReserveResult::Unregistered
    }

    pub fn validate(&self, task_id: &str, session_id: &str) -> ReserveResult {
        match self.inner.lock() {
            Ok(guard) => Self::check_binding(&guard, task_id, session_id),
            Err(_) => ReserveResult::Unregistered,
        }
    }

    #[cfg(test)]
    pub fn pending_task_ids_for_session(&self, session_id: &str) -> Vec<String> {
        match self.inner.lock() {
            Ok(guard) => guard
                .pending
                .iter()
                .filter(|(_, sid)| sid.as_str() == session_id)
                .map(|(task_id, _)| task_id.clone())
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    pub fn session_for(&self, task_id: &str) -> Option<String> {
        self.inner.lock().ok()?.pending.get(task_id).cloned()
    }

    pub fn release(&self, task_id: &str) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.reserved.remove(task_id);
        }
    }

    pub fn complete(&self, task_id: &str) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.reserved.remove(task_id);
            guard.last_activity.remove(task_id);
            guard.timeout_claimed.remove(task_id);
            let was_pending = guard.pending.remove(task_id).is_some();
            if was_pending {
                guard.order.retain(|t| t != task_id);
            }
            let newly_finalized = guard.finalized.insert(task_id.to_string());
            if newly_finalized {
                guard.finalized_order.push_back(task_id.to_string());
                while guard.finalized_order.len() > PENDING_CAPACITY {
                    if let Some(old) = guard.finalized_order.pop_front() {
                        guard.finalized.remove(&old);
                    }
                }
            }
            if was_pending || newly_finalized {
                self.persist(&guard);
            }
        }
    }

    fn persist(&self, inner: &PendingInner) {
        let Some(path) = &self.path else {
            return;
        };
        let state = PersistState {
            pending: inner
                .order
                .iter()
                .filter_map(|task_id| {
                    inner
                        .pending
                        .get(task_id)
                        .map(|session_id| (task_id.clone(), session_id.clone()))
                })
                .collect(),
            finalized: inner.finalized_order.iter().cloned().collect(),
        };
        let Ok(raw) = serde_json::to_string(&state) else {
            return;
        };
        let tmp = unique_tmp_path(path);
        if fs::write(&tmp, raw).is_err() {
            let _ = fs::remove_file(&tmp);
            return;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = fs::metadata(&tmp) {
                let mut perms = meta.permissions();
                perms.set_mode(0o600);
                let _ = fs::set_permissions(&tmp, perms);
            }
        }
        if fs::rename(&tmp, path).is_err() {
            let _ = fs::remove_file(&tmp);
        }
    }
}

fn unique_tmp_path(path: &Path) -> PathBuf {
    let mut suffix = [0u8; 8];
    getrandom::fill(&mut suffix).expect("OS RNG failed while generating temp file suffix");
    let random_hex = suffix
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    path.with_extension(format!("json.tmp.{}.{random_hex}", std::process::id()))
}

#[cfg(test)]
#[path = "pending_tests.rs"]
mod tests;
