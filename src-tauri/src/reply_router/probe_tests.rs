use super::*;

const TEST_TASK_ID: &str = "11111111-2222-3333-4444-555555555555";

#[tokio::test]
async fn probe_registry_register_and_take_round_trip() {
    let registry = ProbeRegistry::default();
    let rx = registry.register(TEST_TASK_ID.to_string());
    let tx = registry.take(TEST_TASK_ID).expect("registered probe");
    tx.send(ProbeOutcome {
        status: "completed".to_string(),
        reply_text: Some("ok".to_string()),
        error_message: None,
    })
    .unwrap();
    let outcome = rx.await.unwrap();
    assert_eq!(outcome.status, "completed");
    assert_eq!(outcome.reply_text.as_deref(), Some("ok"));
    assert!(registry.take(TEST_TASK_ID).is_none());
}
