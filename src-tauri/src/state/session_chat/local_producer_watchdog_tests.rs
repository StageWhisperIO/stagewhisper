use super::*;
use sw_notes::UiChunk;

fn temp_store() -> Arc<sw_notes::SessionStore> {
    let dir = std::env::temp_dir().join(format!(
        "sw-free-local-producer-watchdog-tests-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    Arc::new(sw_notes::SessionStore::new(dir, [9u8; 32]).unwrap())
}

fn recording_sink() -> (
    impl Fn(UiChunk) + Send + Sync + 'static,
    Arc<std::sync::Mutex<Vec<UiChunk>>>,
) {
    let received = Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorder = received.clone();
    (
        move |chunk: UiChunk| recorder.lock().unwrap().push(chunk),
        received,
    )
}

#[tokio::test(start_paused = true)]
async fn a_local_turn_whose_model_startup_never_completes_terminates_with_an_error_and_is_deregistered(
) {
    let store = temp_store();
    let registry = TurnRegistry::new();
    let (sink, received) = recording_sink();
    let _handle = registry
        .register(
            "turn-1",
            "sess-1",
            "user-1",
            "turn-1",
            Some(LOCAL_IDENTITY_BINDING.to_string()),
            sink,
        )
        .expect("fresh turn id registers");
    received.lock().unwrap().clear();

    let activity = LocalGenerationActivity::new();
    let generation: JoinHandle<Result<String, String>> = tokio::spawn(std::future::pending());

    let call = await_local_generation(
        generation,
        activity,
        Duration::from_secs(5),
        Duration::from_secs(60),
    );
    tokio::pin!(call);
    tokio::time::advance(Duration::from_secs(6)).await;
    let result = call.await;

    assert_eq!(result, Err(LOCAL_ENGINE_STARTUP_ERROR_TEXT.to_string()));

    let persistence = StorePersistence {
        store: store.as_ref(),
    };
    let terminal = finalize_local_reply(
        &registry,
        &persistence,
        "turn-1",
        TurnOutcome::Failure(result.unwrap_err()),
    );

    assert!(matches!(terminal, TerminalResult::Terminated(_)));
    assert!(!registry.contains("turn-1"));
}

#[tokio::test(start_paused = true)]
async fn a_local_turn_whose_generation_stalls_mid_stream_terminates_with_an_error_and_is_deregistered(
) {
    let store = temp_store();
    let registry = TurnRegistry::new();
    let (sink, received) = recording_sink();
    let _handle = registry
        .register(
            "turn-1",
            "sess-1",
            "user-1",
            "turn-1",
            Some(LOCAL_IDENTITY_BINDING.to_string()),
            sink,
        )
        .expect("fresh turn id registers");
    received.lock().unwrap().clear();

    let activity = LocalGenerationActivity::new();
    activity.touch();
    let generation: JoinHandle<Result<String, String>> = tokio::spawn(std::future::pending());

    let call = await_local_generation(
        generation,
        activity,
        Duration::from_secs(120),
        Duration::from_secs(5),
    );
    tokio::pin!(call);
    tokio::time::advance(Duration::from_secs(6)).await;
    let result = call.await;

    assert_eq!(result, Err(LOCAL_GENERATION_STALLED_ERROR_TEXT.to_string()));

    let persistence = StorePersistence {
        store: store.as_ref(),
    };
    let terminal = finalize_local_reply(
        &registry,
        &persistence,
        "turn-1",
        TurnOutcome::Failure(result.unwrap_err()),
    );

    assert!(matches!(terminal, TerminalResult::Terminated(_)));
    assert!(!registry.contains("turn-1"));
}

#[tokio::test(start_paused = true)]
async fn a_stalled_local_turn_releases_the_queue_so_the_next_queued_local_turn_still_runs() {
    let queue = crate::state::local_llm::LocalTurnQueue::new();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();

    queue
        .enqueue(|_reason: String| {}, async {
            let activity = LocalGenerationActivity::new();
            let generation: JoinHandle<Result<String, String>> =
                tokio::spawn(std::future::pending());
            let _ = await_local_generation(
                generation,
                activity,
                Duration::from_secs(5),
                Duration::from_secs(60),
            )
            .await;
        })
        .unwrap();

    queue
        .enqueue(|_reason: String| {}, async move {
            let _ = tx.send(());
        })
        .unwrap();

    tokio::time::advance(Duration::from_secs(6)).await;
    let received = rx.await;
    assert!(
        received.is_ok(),
        "the turn queued after a stalled local turn never ran"
    );
}

#[tokio::test(start_paused = true)]
async fn a_healthy_local_generation_that_takes_longer_than_the_idle_timeout_between_start_and_finish_is_not_killed_while_it_is_still_producing_tokens(
) {
    let idle_timeout = Duration::from_secs(5);
    let tick = Duration::from_secs(2);
    let activity = LocalGenerationActivity::new();
    let generation_activity = activity.clone();

    let generation: JoinHandle<Result<String, String>> = tokio::spawn(async move {
        let mut content = String::new();
        for step in 0..5 {
            tokio::time::sleep(tick).await;
            generation_activity.touch();
            content.push_str(&format!("token-{step} "));
        }
        Ok(content)
    });

    let call = await_local_generation(generation, activity, Duration::from_secs(120), idle_timeout);
    tokio::pin!(call);

    for _ in 0..5 {
        tokio::time::advance(tick).await;
    }
    tokio::time::advance(Duration::from_millis(500)).await;
    let result = call.await;

    assert!(result.is_ok());
    assert!(result.unwrap().contains("token-4"));
}
