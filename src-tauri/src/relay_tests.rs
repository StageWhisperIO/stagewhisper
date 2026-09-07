use super::*;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn configured_settings(relay_url: String) -> RelaySettings {
    RelaySettings {
        relay_url,
        relay_token: "tok".to_string(),
        ..RelaySettings::default()
    }
}

struct CapturedRequest {
    body: Mutex<Option<Vec<u8>>>,
}

struct CaptureServer {
    addr: SocketAddr,
    captured: Arc<CapturedRequest>,
}

impl CaptureServer {
    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn body_json(&self) -> Value {
        let bytes = self
            .captured
            .body
            .lock()
            .unwrap()
            .clone()
            .expect("request body was captured");
        serde_json::from_slice(&bytes).expect("captured body is valid json")
    }
}

fn header_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        if key.trim().eq_ignore_ascii_case(name) {
            Some(value.trim())
        } else {
            None
        }
    })
}

async fn spawn_capture_server() -> CaptureServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let captured = Arc::new(CapturedRequest {
        body: Mutex::new(None),
    });
    let captured_for_task = captured.clone();

    tokio::spawn(async move {
        let Ok((mut stream, _)) = listener.accept().await else {
            return;
        };
        let mut buffer = Vec::new();
        let mut chunk = [0u8; 4096];
        let mut content_length: Option<usize> = None;
        let mut head_end: Option<usize> = None;
        loop {
            let read = stream.read(&mut chunk).await.unwrap_or(0);
            if read == 0 {
                break;
            }
            buffer.extend_from_slice(&chunk[..read]);
            if head_end.is_none() {
                if let Some(end) = buffer
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .map(|position| position + 4)
                {
                    let head = String::from_utf8_lossy(&buffer[..end]).into_owned();
                    content_length = header_value(&head, "content-length")
                        .and_then(|value| value.parse::<usize>().ok());
                    head_end = Some(end);
                }
            }
            if let (Some(end), Some(len)) = (head_end, content_length) {
                if buffer.len() >= end + len {
                    *captured_for_task.body.lock().unwrap() = Some(buffer[end..end + len].to_vec());
                    break;
                }
            }
        }
        let response = b"HTTP/1.1 202 Accepted\r\ncontent-length: 0\r\nconnection: close\r\n\r\n";
        let _ = stream.write_all(response).await;
        let _ = stream.flush().await;
    });

    CaptureServer { addr, captured }
}

#[tokio::test]
async fn an_outbound_chat_message_never_carries_a_callback_block() {
    let server = spawn_capture_server().await;
    let client = RelayClient::new().expect("relay client");

    let result = client
        .send_session_chat(
            &configured_settings(server.base_url()),
            Uuid::new_v4(),
            "hello".to_string(),
            None,
            Uuid::new_v4(),
        )
        .await;

    assert!(result.is_ok(), "send should succeed: {result:?}");
    let body = server.body_json();
    assert!(
        body.get("callback").is_none(),
        "outbound body must never carry a callback block, got: {body}"
    );
}

#[tokio::test]
async fn an_outbound_probe_style_chat_message_also_carries_no_callback_block() {
    let server = spawn_capture_server().await;
    let client = RelayClient::new().expect("relay client");

    let result = client
        .send_session_chat(
            &configured_settings(server.base_url()),
            Uuid::new_v4(),
            "StageWhisper connection check. Please reply with \"ok\".".to_string(),
            Some(Uuid::new_v4().to_string()),
            Uuid::new_v4(),
        )
        .await;

    assert!(result.is_ok(), "send should succeed: {result:?}");
    let body = server.body_json();
    assert!(
        body.get("callback").is_none(),
        "outbound body must never carry a callback block, got: {body}"
    );
}
