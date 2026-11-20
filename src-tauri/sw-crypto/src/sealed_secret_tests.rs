use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::*;

#[derive(Default)]
struct MemoryKeyStore {
    key: Mutex<Option<Vec<u8>>>,
    loads: AtomicUsize,
    stores: AtomicUsize,
    refuse_loads: AtomicBool,
    refuse_stores: AtomicBool,
}

impl MemoryKeyStore {
    fn refusing_loads() -> Self {
        Self {
            refuse_loads: AtomicBool::new(true),
            ..Self::default()
        }
    }

    fn loads(&self) -> usize {
        self.loads.load(Ordering::SeqCst)
    }

    fn stores(&self) -> usize {
        self.stores.load(Ordering::SeqCst)
    }
}

impl SealingKeyStore for MemoryKeyStore {
    fn load(&self) -> Result<Option<Vec<u8>>, String> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        if self.refuse_loads.load(Ordering::SeqCst) {
            return Err("the user denied access".to_string());
        }
        Ok(self.key.lock().unwrap().clone())
    }

    fn store(&self, key: &[u8]) -> Result<(), String> {
        self.stores.fetch_add(1, Ordering::SeqCst);
        if self.refuse_stores.load(Ordering::SeqCst) {
            return Err("the user denied access".to_string());
        }
        *self.key.lock().unwrap() = Some(key.to_vec());
        Ok(())
    }
}

struct ManualClock {
    now: Arc<Mutex<Instant>>,
}

impl ManualClock {
    fn start() -> Self {
        Self {
            now: Arc::new(Mutex::new(Instant::now())),
        }
    }

    fn clock(&self) -> Clock {
        let now = self.now.clone();
        Arc::new(move || *now.lock().unwrap())
    }

    fn advance(&self, by: Duration) {
        *self.now.lock().unwrap() += by;
    }
}

#[test]
fn a_sealed_secret_opens_again_after_a_relaunch_and_hides_its_plaintext() {
    let keys = Arc::new(MemoryKeyStore::default());
    let sealed = SecretSealer::new(keys.clone())
        .seal_text("relay-token-123")
        .unwrap();

    let reopened = SecretSealer::new(keys).unseal_text(&sealed).unwrap();

    assert_eq!(reopened.as_str(), "relay-token-123");
    assert!(!sealed.sealed.contains("relay-token-123"));
    assert_eq!(sealed.version, SEALED_SECRET_VERSION);
}

#[test]
fn every_secret_shares_one_key_created_once() {
    let keys = Arc::new(MemoryKeyStore::default());
    let sealer = Arc::new(SecretSealer::new(keys.clone()));

    let writers: Vec<_> = (0..8)
        .map(|i| {
            let sealer = sealer.clone();
            thread::spawn(move || sealer.seal_text(&format!("secret-{i}")).unwrap())
        })
        .collect();
    let sealed: Vec<_> = writers.into_iter().map(|w| w.join().unwrap()).collect();

    assert_eq!(keys.stores(), 1);
    let relaunched = SecretSealer::new(keys);
    for (i, secret) in sealed.iter().enumerate() {
        assert_eq!(
            relaunched.unseal_text(secret).unwrap().as_str(),
            format!("secret-{i}")
        );
    }
}

#[test]
fn opening_never_creates_a_key() {
    let keys = Arc::new(MemoryKeyStore::default());
    let other = SecretSealer::new(Arc::new(MemoryKeyStore::default()))
        .seal_text("secret")
        .unwrap();

    let opened = SecretSealer::new(keys.clone()).unseal_text(&other);

    assert_eq!(opened.unwrap_err(), UnsealError::KeyMissing);
    assert_eq!(keys.stores(), 0);
}

#[test]
fn a_keychain_that_refuses_to_load_never_gets_a_new_key_written_over_it() {
    let keys = Arc::new(MemoryKeyStore::refusing_loads());

    assert!(SecretSealer::new(keys.clone()).seal_text("secret").is_err());

    assert_eq!(keys.stores(), 0);
}

#[test]
fn a_secret_sealed_under_another_key_or_format_reports_why_without_its_contents() {
    let sealed = SecretSealer::new(Arc::new(MemoryKeyStore::default()))
        .seal_text("secret-value")
        .unwrap();
    let other_keys = Arc::new(MemoryKeyStore::default());
    other_keys.store(&[7u8; 32]).unwrap();
    let other = SecretSealer::new(other_keys);

    let mismatched = other.unseal_text(&sealed).unwrap_err();
    let newer = other
        .unseal_text(&SealedSecret {
            version: SEALED_SECRET_VERSION + 1,
            ..sealed.clone()
        })
        .unwrap_err();
    let garbled = other
        .unseal_text(&SealedSecret {
            version: SEALED_SECRET_VERSION,
            sealed: "not base64 at all".to_string(),
        })
        .unwrap_err();

    assert_eq!(mismatched, UnsealError::WrongKey);
    assert_eq!(newer, UnsealError::NewerFormat);
    assert_eq!(garbled, UnsealError::Unreadable);
    for reason in [mismatched, newer, garbled] {
        assert!(!reason.to_string().contains("secret-value"));
    }
}

#[test]
fn only_a_keychain_that_refused_reads_as_a_failure_worth_retrying() {
    let sealed = SecretSealer::new(Arc::new(MemoryKeyStore::default()))
        .seal_text("secret-value")
        .unwrap();
    let refused = SecretSealer::new(Arc::new(MemoryKeyStore::refusing_loads()))
        .unseal_text(&sealed)
        .unwrap_err();
    let missing = SecretSealer::new(Arc::new(MemoryKeyStore::default()))
        .unseal_text(&sealed)
        .unwrap_err();

    assert!(refused.is_transient());
    assert!(!missing.is_transient());
    for permanent in [
        UnsealError::KeyMissing,
        UnsealError::NewerFormat,
        UnsealError::Unreadable,
        UnsealError::WrongKey,
    ] {
        assert!(!permanent.is_transient(), "{permanent:?}");
    }
}

#[test]
fn a_refused_keychain_is_not_asked_again_until_the_cool_down_ends_and_then_is() {
    let origin = Arc::new(MemoryKeyStore::default());
    let sealed = SecretSealer::new(origin.clone())
        .seal_text("secret-value")
        .unwrap();
    let keys = Arc::new(MemoryKeyStore::refusing_loads());
    let clock = ManualClock::start();
    let sealer = SecretSealer::with_clock(keys.clone(), clock.clock());

    assert!(sealer.unseal_text(&sealed).unwrap_err().is_transient());
    assert_eq!(keys.loads(), 1);

    keys.refuse_loads.store(false, Ordering::SeqCst);
    *keys.key.lock().unwrap() = origin.key.lock().unwrap().clone();
    clock.advance(KEYCHAIN_RETRY_AFTER - Duration::from_secs(1));
    assert!(sealer.unseal_text(&sealed).unwrap_err().is_transient());
    assert_eq!(keys.loads(), 1);

    clock.advance(Duration::from_secs(1));
    assert_eq!(
        sealer.unseal_text(&sealed).unwrap().as_str(),
        "secret-value"
    );
    assert_eq!(keys.loads(), 2);
}

#[test]
fn a_save_right_after_a_refused_prompt_asks_the_keychain_again() {
    let keys = Arc::new(MemoryKeyStore::default());
    keys.refuse_stores.store(true, Ordering::SeqCst);
    let clock = ManualClock::start();
    let sealer = SecretSealer::with_clock(keys.clone(), clock.clock());
    assert!(sealer.seal_text("secret").is_err());

    keys.refuse_stores.store(false, Ordering::SeqCst);
    let sealed = sealer.seal_text("secret").unwrap();

    assert_eq!((keys.loads(), keys.stores()), (2, 2));
    assert_eq!(sealer.unseal_text(&sealed).unwrap().as_str(), "secret");
}

#[test]
fn a_successful_save_clears_the_cool_down() {
    let origin = Arc::new(MemoryKeyStore::default());
    let sealed = SecretSealer::new(origin.clone())
        .seal_text("older-secret")
        .unwrap();
    let keys = Arc::new(MemoryKeyStore::refusing_loads());
    let clock = ManualClock::start();
    let sealer = SecretSealer::with_clock(keys.clone(), clock.clock());
    assert!(sealer.unseal_text(&sealed).unwrap_err().is_transient());

    keys.refuse_loads.store(false, Ordering::SeqCst);
    *keys.key.lock().unwrap() = origin.key.lock().unwrap().clone();
    let fresh = sealer.seal_text("new-secret").unwrap();

    assert_eq!(
        sealer.unseal_text(&sealed).unwrap().as_str(),
        "older-secret"
    );
    assert_eq!(sealer.unseal_text(&fresh).unwrap().as_str(), "new-secret");
    assert_eq!(keys.loads(), 2);
}

#[test]
fn a_background_save_waits_out_the_cool_down_a_refused_prompt_started() {
    let keys = Arc::new(MemoryKeyStore::refusing_loads());
    let clock = ManualClock::start();
    let sealer = SecretSealer::with_clock(keys.clone(), clock.clock());
    assert!(sealer.seal_text_in_background("secret").is_err());
    assert_eq!(keys.loads(), 1);

    keys.refuse_loads.store(false, Ordering::SeqCst);
    clock.advance(KEYCHAIN_RETRY_AFTER - Duration::from_secs(1));
    assert!(sealer.seal_text_in_background("secret").is_err());
    assert_eq!(keys.loads(), 1);

    clock.advance(Duration::from_secs(1));
    let sealed = sealer.seal_text_in_background("secret").unwrap();
    assert_eq!(keys.loads(), 2);
    assert_eq!(sealer.unseal_text(&sealed).unwrap().as_str(), "secret");
}
