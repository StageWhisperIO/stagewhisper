use super::*;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex as StdMutex;
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::reply_router::{ChatMessagePayload, ReserveResult};

#[derive(Default)]
struct FakeSink {
    session_id: StdMutex<Option<String>>,
    finalized: StdMutex<std::collections::HashSet<String>>,
    appended: StdMutex<Vec<ChatMessagePayload>>,
}

impl FakeSink {
    fn new(session_id: &str) -> Self {
        Self {
            session_id: StdMutex::new(Some(session_id.to_string())),
            ..Default::default()
        }
    }
}

impl ReplySink for FakeSink {
    fn current_session_id(&self) -> Option<String> {
        self.session_id.lock().unwrap().clone()
    }
    fn session_known(&self, _session_id: &str) -> bool {
        true
    }
    fn append_message(&self, message: ChatMessagePayload) -> bool {
        self.appended.lock().unwrap().push(message);
        true
    }
    fn emit_created(&self, _payload: &ChatMessagePayload) {}
    fn emit_errored(&self, _payload: &Value) {}
    fn reserve_terminal(&self, task_id: &str, _session_id: &str) -> ReserveResult {
        if self.finalized.lock().unwrap().contains(task_id) {
            ReserveResult::Duplicate
        } else {
            ReserveResult::Reserved
        }
    }
    fn release_terminal(&self, _task_id: &str) {}
    fn complete_terminal(&self, task_id: &str) {
        self.finalized.lock().unwrap().insert(task_id.to_string());
    }
}

const TASK_ID: &str = "11111111-2222-3333-4444-555555555555";

#[test]
fn a_streamed_reply_uses_the_same_router_as_a_callback_reply() {
    let via_stream = FakeSink::new("sess-1");
    let via_callback = FakeSink::new("sess-1");
    let payload = json!({
        "task_id": TASK_ID,
        "session_id": "sess-1",
        "status": "message",
        "reply_text": "hello from the stream",
    });

    let stream_disposition = route_reply_payload(&via_stream, payload.clone());
    let body: ReplyBody = serde_json::from_value(payload).unwrap();
    let callback_disposition = route_reply(&via_callback, TASK_ID, body);

    assert_eq!(stream_disposition, Some(callback_disposition));
    assert_eq!(via_stream.appended.lock().unwrap().len(), 1);
}

#[test]
fn a_malformed_streamed_reply_does_not_poison_the_next_frame() {
    let sink = FakeSink::new("sess-1");
    assert_eq!(
        route_reply_payload(&sink, json!({"unexpected": true})),
        None
    );

    let valid = json!({
        "task_id": TASK_ID,
        "session_id": "sess-1",
        "status": "message",
        "reply_text": "still routable",
    });
    assert_eq!(
        route_reply_payload(&sink, valid),
        Some(ReplyDisposition::Accepted)
    );
}

#[test]
fn a_disposition_that_durably_recorded_the_task_outcome_is_settled() {
    assert!(reply_task_is_settled(&ReplyDisposition::Accepted));
    assert!(reply_task_is_settled(&ReplyDisposition::AlreadyFinalized));
    assert!(reply_task_is_settled(&ReplyDisposition::ProbeResolved));
}

#[test]
fn a_disposition_that_rejected_or_failed_to_persist_the_frame_is_not_settled() {
    assert!(!reply_task_is_settled(&ReplyDisposition::UnregisteredTask));
    assert!(!reply_task_is_settled(&ReplyDisposition::SessionMismatch));
    assert!(!reply_task_is_settled(
        &ReplyDisposition::EmptyMessageIgnored
    ));
    assert!(!reply_task_is_settled(&ReplyDisposition::Dropped(
        crate::reply_router::DropReason::SessionEnded
    )));
    assert!(!reply_task_is_settled(&ReplyDisposition::PersistFailed));
    assert!(!reply_task_is_settled(&ReplyDisposition::InvalidTaskId));
    assert!(!reply_task_is_settled(&ReplyDisposition::TaskIdMismatch));
    assert!(!reply_task_is_settled(&ReplyDisposition::SessionIdRequired));
    assert!(!reply_task_is_settled(&ReplyDisposition::InvalidStatus));
}

struct HoldOpenServer {
    addr: SocketAddr,
    connections: std::sync::Arc<AtomicUsize>,
}

impl HoldOpenServer {
    fn settings(&self) -> RelaySettings {
        RelaySettings {
            relay_url: format!("http://{}", self.addr),
            relay_token: "tok".to_string(),
            paired_verified: true,
        }
    }

    fn connection_count(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}

async fn spawn_hold_open_server() -> HoldOpenServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let connections = std::sync::Arc::new(AtomicUsize::new(0));
    let observed = connections.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            observed.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer).await;
                let response = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\nconnection: keep-alive\r\n\r\n";
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.flush().await;
                tokio::time::sleep(Duration::from_secs(10)).await;
            });
        }
    });
    HoldOpenServer { addr, connections }
}

async fn wait_until(mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if condition() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn pending_tasks_from_two_sessions_keep_two_independent_subscriptions() {
    let server = spawn_hold_open_server().await;
    let settings = server.settings();
    let manager = ReplyStreamManager::default();

    manager
        .acquire(
            &settings,
            "sess-a".to_string(),
            "task-a".to_string(),
            |_| {},
        )
        .await;
    manager
        .acquire(
            &settings,
            "sess-b".to_string(),
            "task-b".to_string(),
            |_| {},
        )
        .await;

    assert!(wait_until(|| server.connection_count() >= 2).await);
    assert_eq!(manager.session_count().await, 2);
    manager.cancel_all().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_last_task_owner_releases_its_shared_session_subscription() {
    let server = spawn_hold_open_server().await;
    let settings = server.settings();
    let manager = ReplyStreamManager::default();

    manager
        .acquire(
            &settings,
            "sess-a".to_string(),
            "task-a".to_string(),
            |_| {},
        )
        .await;
    manager
        .acquire(
            &settings,
            "sess-a".to_string(),
            "task-b".to_string(),
            |_| {},
        )
        .await;

    assert!(wait_until(|| server.connection_count() >= 1).await);
    assert_eq!(manager.session_count().await, 1);
    assert_eq!(manager.owner_count("sess-a").await, 2);

    manager.release("sess-a", "task-a").await;
    assert_eq!(manager.session_count().await, 1);
    manager.release("sess-a", "task-b").await;
    assert_eq!(manager.session_count().await, 0);
}
