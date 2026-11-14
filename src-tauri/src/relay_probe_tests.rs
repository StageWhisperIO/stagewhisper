use super::*;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Instant;

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::reply_router::{ChatMessagePayload, ReserveResult};

struct ProbeOnlySink {
    registry: Arc<ProbeRegistry>,
}

impl ReplySink for ProbeOnlySink {
    fn current_session_id(&self) -> Option<String> {
        None
    }
    fn session_known(&self, _session_id: &str) -> bool {
        false
    }
    fn append_message(&self, _message: ChatMessagePayload) -> bool {
        true
    }
    fn emit_created(&self, _payload: &ChatMessagePayload) {}
    fn emit_errored(&self, _payload: &Value) {}
    fn reserve_terminal(&self, _task_id: &str, _session_id: &str) -> ReserveResult {
        ReserveResult::Reserved
    }
    fn release_terminal(&self, _task_id: &str) {}
    fn complete_terminal(&self, _task_id: &str) {}
    fn resolve_probe(&self, task_id: &str, outcome: ProbeOutcome) -> bool {
        match self.registry.take(task_id) {
            Some(tx) => {
                let _ = tx.send(outcome);
                true
            }
            None => false,
        }
    }
}

struct StubStreamServer {
    addr: SocketAddr,
    connections: Arc<AtomicUsize>,
    closed: Arc<AtomicBool>,
}

impl StubStreamServer {
    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }
    fn connection_count(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

async fn spawn_reply_frame_server(frame: Option<String>) -> StubStreamServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let connections = Arc::new(AtomicUsize::new(0));
    let closed = Arc::new(AtomicBool::new(false));
    let connections_for_task = connections.clone();
    let closed_for_task = closed.clone();

    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            connections_for_task.fetch_add(1, Ordering::SeqCst);
            let frame = frame.clone();
            let closed = closed_for_task.clone();
            tokio::spawn(async move {
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer).await;
                let head = "HTTP/1.1 200 OK\r\n\
                             content-type: text/event-stream\r\n\
                             transfer-encoding: chunked\r\n\
                             cache-control: no-cache\r\n\
                             connection: keep-alive\r\n\
                             \r\n";
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream.flush().await;
                if let Some(frame) = frame {
                    let chunk = format!("{:x}\r\n{}\r\n", frame.len(), frame);
                    let _ = stream.write_all(chunk.as_bytes()).await;
                    let _ = stream.flush().await;
                }
                loop {
                    match stream.read(&mut buffer).await {
                        Ok(0) | Err(_) => {
                            closed.store(true, Ordering::SeqCst);
                            return;
                        }
                        Ok(_) => continue,
                    }
                }
            });
        }
    });

    StubStreamServer {
        addr,
        connections,
        closed,
    }
}

async fn wait_until(mut condition: impl FnMut() -> bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
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
async fn a_probe_resolves_when_a_reply_arrives_on_its_dedicated_stream() {
    let task_id = Uuid::new_v4();
    let frame = format!(
        "id: 1\ndata: {}\n\n",
        json!({
            "task_id": task_id.to_string(),
            "session_id": "probe-sess",
            "status": "message",
            "reply_text": "ok",
        })
    );
    let server = spawn_reply_frame_server(Some(frame)).await;
    let registry = Arc::new(ProbeRegistry::default());
    let sink = Arc::new(ProbeOnlySink {
        registry: registry.clone(),
    });
    let config = ReplyStreamConfig {
        base_url: server.base_url(),
        token: "tok".to_string(),
        session_id: "probe-sess".to_string(),
    };

    let outcome = probe_over_stream(
        sink,
        config,
        registry.as_ref(),
        task_id,
        Duration::from_secs(5),
        || async { Ok(()) },
    )
    .await
    .expect("probe resolves from the streamed reply");

    assert_eq!(outcome.status, "message");
    assert_eq!(outcome.reply_text.as_deref(), Some("ok"));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_probe_subscription_is_cancelled_once_the_probe_resolves() {
    let task_id = Uuid::new_v4();
    let frame = format!(
        "id: 1\ndata: {}\n\n",
        json!({
            "task_id": task_id.to_string(),
            "session_id": "probe-sess",
            "status": "message",
            "reply_text": "ok",
        })
    );
    let server = spawn_reply_frame_server(Some(frame)).await;
    let registry = Arc::new(ProbeRegistry::default());
    let sink = Arc::new(ProbeOnlySink {
        registry: registry.clone(),
    });
    let config = ReplyStreamConfig {
        base_url: server.base_url(),
        token: "tok".to_string(),
        session_id: "probe-sess".to_string(),
    };

    let outcome = probe_over_stream(
        sink,
        config,
        registry.as_ref(),
        task_id,
        Duration::from_secs(5),
        || async { Ok(()) },
    )
    .await;
    assert!(outcome.is_ok());

    assert!(
        wait_until(|| server.is_closed(), Duration::from_secs(2)).await,
        "the probe's stream connection must be closed once the probe resolves"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_probe_that_never_gets_a_reply_times_out_with_a_user_facing_message() {
    let task_id = Uuid::new_v4();
    let server = spawn_reply_frame_server(None).await;
    let registry = Arc::new(ProbeRegistry::default());
    let sink = Arc::new(ProbeOnlySink {
        registry: registry.clone(),
    });
    let config = ReplyStreamConfig {
        base_url: server.base_url(),
        token: "tok".to_string(),
        session_id: "probe-sess".to_string(),
    };

    let outcome = probe_over_stream(
        sink,
        config,
        registry.as_ref(),
        task_id,
        Duration::from_millis(200),
        || async { Ok(()) },
    )
    .await;

    match outcome {
        Err(message) => assert_eq!(message, PROBE_TIMEOUT_MESSAGE),
        Ok(_) => panic!("expected the probe to time out"),
    }
    assert!(server.connection_count() >= 1);
}
