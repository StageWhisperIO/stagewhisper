use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

use sw_crypto::{Clock, SealingKeyStore, SecretSealer};

#[derive(Default)]
pub struct CountingVault {
    key: Mutex<Option<Vec<u8>>>,
    calls: AtomicUsize,
    refuses_to_store: bool,
    refuses_access: AtomicBool,
}

impl CountingVault {
    pub fn refusing_to_store() -> Self {
        Self {
            refuses_to_store: true,
            ..Self::default()
        }
    }

    pub fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    pub fn holds_key(&self) -> bool {
        self.key.lock().unwrap().is_some()
    }

    pub fn forget_key(&self) {
        *self.key.lock().unwrap() = None;
    }

    pub fn put_key(&self, key: &[u8]) {
        *self.key.lock().unwrap() = Some(key.to_vec());
    }

    pub fn refuse_access(&self, refused: bool) {
        self.refuses_access.store(refused, Ordering::SeqCst);
    }

    pub fn sealer(self: &Arc<Self>) -> Arc<SecretSealer> {
        Arc::new(SecretSealer::new(self.clone()))
    }

    pub fn sealer_on(self: &Arc<Self>, clock: &ManualClock) -> Arc<SecretSealer> {
        Arc::new(SecretSealer::with_clock(self.clone(), clock.clock()))
    }
}

impl SealingKeyStore for CountingVault {
    fn load(&self) -> Result<Option<Vec<u8>>, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.refuses_access.load(Ordering::SeqCst) {
            return Err("Keychain read failed: the user dismissed the prompt".to_string());
        }
        Ok(self.key.lock().unwrap().clone())
    }

    fn store(&self, key: &[u8]) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.refuses_to_store {
            return Err("Keychain store failed: the user denied access".to_string());
        }
        self.put_key(key);
        Ok(())
    }
}

pub struct ManualClock {
    now: Arc<Mutex<Instant>>,
}

impl ManualClock {
    pub fn start() -> Self {
        Self {
            now: Arc::new(Mutex::new(Instant::now())),
        }
    }

    pub fn clock(&self) -> Clock {
        let now = self.now.clone();
        Arc::new(move || *now.lock().unwrap())
    }

    pub fn advance(&self, by: Duration) {
        *self.now.lock().unwrap() += by;
    }
}

static CAPTURED_LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());
static INSTALL_CAPTURE: Once = Once::new();
static LINE_CAPTURE: LineCapture = LineCapture;

struct LineCapture;

impl log::Log for LineCapture {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        if let Ok(mut lines) = CAPTURED_LINES.lock() {
            lines.push(record.args().to_string());
        }
    }

    fn flush(&self) {}
}

pub fn capture_log_lines() {
    INSTALL_CAPTURE.call_once(|| {
        log::set_logger(&LINE_CAPTURE).expect("another logger is already installed");
        log::set_max_level(log::LevelFilter::Info);
    });
}

pub fn captured_log_lines() -> Vec<String> {
    CAPTURED_LINES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

fn random_hex() -> String {
    let mut suffix = [0u8; 8];
    getrandom::fill(&mut suffix).expect("OS RNG failed");
    suffix.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unique_token(kind: &str) -> String {
    format!("{kind}-{}", random_hex())
}

pub fn scratch_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "sw-free-sealed-{label}-{}-{}",
        std::process::id(),
        random_hex()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub fn files_mention(dir: &Path, needle: &str) -> bool {
    std::fs::read_dir(dir).unwrap().any(|entry| {
        let contents = std::fs::read(entry.unwrap().path()).unwrap_or_default();
        String::from_utf8_lossy(&contents).contains(needle)
    })
}
