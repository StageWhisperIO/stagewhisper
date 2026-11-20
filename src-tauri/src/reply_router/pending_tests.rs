use super::*;

const TEST_TASK_ID: &str = "11111111-2222-3333-4444-555555555555";

#[test]
fn reserve_then_complete_is_one_shot() {
    let pending = PendingReplies::default();
    pending.register(TEST_TASK_ID.to_string(), "sess-1".to_string());
    assert_eq!(
        pending.reserve(TEST_TASK_ID, "sess-1"),
        ReserveResult::Reserved
    );
    assert_eq!(
        pending.reserve(TEST_TASK_ID, "sess-1"),
        ReserveResult::Duplicate
    );
    pending.complete(TEST_TASK_ID);
    assert_eq!(
        pending.reserve(TEST_TASK_ID, "sess-1"),
        ReserveResult::Duplicate
    );
}

#[test]
fn reserve_rejects_unregistered_task() {
    let pending = PendingReplies::default();
    assert_eq!(
        pending.reserve(TEST_TASK_ID, "sess-1"),
        ReserveResult::Unregistered
    );
    pending.register(TEST_TASK_ID.to_string(), "sess-1".to_string());
    assert_eq!(
        pending.reserve(TEST_TASK_ID, "sess-1"),
        ReserveResult::Reserved
    );
    pending.complete(TEST_TASK_ID);
    assert_eq!(
        pending.reserve(TEST_TASK_ID, "sess-1"),
        ReserveResult::Duplicate
    );
}

#[test]
fn reserve_rejects_session_mismatch() {
    let pending = PendingReplies::default();
    pending.register(TEST_TASK_ID.to_string(), "sess-1".to_string());
    assert_eq!(
        pending.reserve(TEST_TASK_ID, "sess-2"),
        ReserveResult::SessionMismatch
    );
    assert_eq!(
        pending.reserve(TEST_TASK_ID, "sess-1"),
        ReserveResult::Reserved
    );
}

#[test]
fn release_returns_task_to_pending_for_retry() {
    let pending = PendingReplies::default();
    pending.register(TEST_TASK_ID.to_string(), "sess-1".to_string());
    assert_eq!(
        pending.reserve(TEST_TASK_ID, "sess-1"),
        ReserveResult::Reserved
    );
    pending.release(TEST_TASK_ID);
    assert_eq!(
        pending.reserve(TEST_TASK_ID, "sess-1"),
        ReserveResult::Reserved
    );
    pending.complete(TEST_TASK_ID);
    assert_eq!(
        pending.reserve(TEST_TASK_ID, "sess-1"),
        ReserveResult::Duplicate
    );
}

#[test]
fn durable_pending_survives_restart() {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path =
        std::env::temp_dir().join(format!("sw-pending-{}-{}.json", std::process::id(), unique));
    let _ = fs::remove_file(&path);

    {
        let pending = PendingReplies::load(path.clone());
        pending.register(TEST_TASK_ID.to_string(), "sess-1".to_string());
    }

    let reloaded = PendingReplies::load(path.clone());
    assert_eq!(
        reloaded.reserve(TEST_TASK_ID, "sess-2"),
        ReserveResult::SessionMismatch
    );
    assert_eq!(
        reloaded.reserve(TEST_TASK_ID, "sess-1"),
        ReserveResult::Reserved
    );
    reloaded.complete(TEST_TASK_ID);
    assert_eq!(
        reloaded.reserve(TEST_TASK_ID, "sess-1"),
        ReserveResult::Duplicate
    );

    let after_complete = PendingReplies::load(path.clone());
    assert_eq!(
        after_complete.reserve(TEST_TASK_ID, "sess-1"),
        ReserveResult::Duplicate
    );

    let _ = fs::remove_file(&path);
}

#[test]
fn pending_task_ids_for_session_returns_only_tasks_bound_to_that_session() {
    let pending = PendingReplies::default();
    pending.register("task-a".to_string(), "sess-1".to_string());
    pending.register("task-b".to_string(), "sess-1".to_string());
    pending.register("task-c".to_string(), "sess-2".to_string());

    let mut ids = pending.pending_task_ids_for_session("sess-1");
    ids.sort();

    assert_eq!(ids, vec!["task-a".to_string(), "task-b".to_string()]);
    assert!(pending.pending_task_ids_for_session("sess-3").is_empty());
}

#[test]
fn unique_tmp_path_never_returns_the_same_path_twice() {
    let base = std::path::PathBuf::from("/does/not/matter/pending.json");
    let mut seen = std::collections::HashSet::new();
    for _ in 0..64 {
        assert!(seen.insert(unique_tmp_path(&base)));
    }
}

#[test]
fn failed_persist_removes_the_leftover_temp_file() {
    let dir = std::env::temp_dir().join(format!(
        "sw-free-pending-failed-persist-{}-{}",
        std::process::id(),
        {
            let mut suffix = [0u8; 8];
            getrandom::fill(&mut suffix).expect("OS RNG failed");
            suffix
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        }
    ));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("pending");
    fs::create_dir_all(&path).unwrap();

    let pending = PendingReplies::load(path.clone());
    pending.register(TEST_TASK_ID.to_string(), "sess-1".to_string());

    let leftovers: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|p| p.to_string_lossy().contains(".json.tmp."))
        .collect();
    assert!(leftovers.is_empty());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn concurrent_registers_across_multiple_stores_targeting_the_same_file_leave_no_leftovers() {
    let dir = std::env::temp_dir().join(format!(
        "sw-free-pending-concurrency-{}-{}",
        std::process::id(),
        {
            let mut suffix = [0u8; 8];
            getrandom::fill(&mut suffix).expect("OS RNG failed");
            suffix
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        }
    ));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("pending.json");

    let handles: Vec<_> = (0..16)
        .map(|i| {
            let path = path.clone();
            std::thread::spawn(move || {
                let pending = PendingReplies::load(path);
                pending.register(format!("task-{i}"), format!("sess-{i}"));
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }

    let raw = fs::read_to_string(&path).unwrap();
    let _: PersistState = serde_json::from_str(&raw).unwrap();
    let leftovers: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|p| p.to_string_lossy().contains(".json.tmp."))
        .collect();
    assert!(leftovers.is_empty());
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn reserved_task_is_pending_again_after_restart() {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "sw-pending-reserved-{}-{}.json",
        std::process::id(),
        unique
    ));
    let _ = fs::remove_file(&path);

    {
        let pending = PendingReplies::load(path.clone());
        pending.register(TEST_TASK_ID.to_string(), "sess-1".to_string());
        assert_eq!(
            pending.reserve(TEST_TASK_ID, "sess-1"),
            ReserveResult::Reserved
        );
    }

    let reloaded = PendingReplies::load(path.clone());
    assert_eq!(
        reloaded.reserve(TEST_TASK_ID, "sess-1"),
        ReserveResult::Reserved
    );

    let _ = fs::remove_file(&path);
}
