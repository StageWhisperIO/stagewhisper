use std::collections::HashMap;

use tokio::sync::oneshot;

#[derive(Clone, Debug)]
pub struct ProbeOutcome {
    pub status: String,
    pub reply_text: Option<String>,
    pub error_message: Option<String>,
}

#[derive(Default)]
pub struct ProbeRegistry {
    inner: std::sync::Mutex<HashMap<String, oneshot::Sender<ProbeOutcome>>>,
}

impl ProbeRegistry {
    pub fn register(&self, task_id: String) -> oneshot::Receiver<ProbeOutcome> {
        let (tx, rx) = oneshot::channel();
        if let Ok(mut guard) = self.inner.lock() {
            guard.insert(task_id, tx);
        }
        rx
    }

    pub fn cancel(&self, task_id: &str) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.remove(task_id);
        }
    }

    pub(crate) fn take(&self, task_id: &str) -> Option<oneshot::Sender<ProbeOutcome>> {
        self.inner.lock().ok().and_then(|mut g| g.remove(task_id))
    }
}

#[cfg(test)]
#[path = "probe_tests.rs"]
mod tests;
