use crate::download::resolve_gguf_path;
use crate::types::{GenerationChunk, InferenceParams, LlmError, ModelEntry};
use futures_util::StreamExt;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const CONTEXT_TOKENS: &str = "8192";
const GPU_LAYERS: &str = "999";
const HEALTH_ATTEMPTS: usize = 600;
const HEALTH_INTERVAL: Duration = Duration::from_millis(500);
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);

pub struct SidecarPaths {
    pub server_bin: PathBuf,
    pub lib_dir: PathBuf,
}

pub struct LocalLlmEngine {
    child: Child,
    base_url: String,
    client: reqwest::Client,
    label: String,
}

impl LocalLlmEngine {
    pub async fn load(
        sidecar: &SidecarPaths,
        model_dir: &Path,
        entry: &ModelEntry,
    ) -> Result<Self, LlmError> {
        let gguf = resolve_gguf_path(model_dir, entry)
            .ok_or_else(|| LlmError::ModelNotFound(model_dir.display().to_string()))?;
        if !sidecar.server_bin.exists() {
            return Err(LlmError::Load(format!(
                "llama-server not found at {}",
                sidecar.server_bin.display()
            )));
        }

        let port = free_port()?;
        let mut command = Command::new(&sidecar.server_bin);
        command
            .arg("-m")
            .arg(&gguf)
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .arg("-c")
            .arg(CONTEXT_TOKENS)
            .arg("-ngl")
            .arg(GPU_LAYERS)
            .arg("--no-webui")
            .env("DYLD_LIBRARY_PATH", &sidecar.lib_dir)
            .env("DYLD_FALLBACK_LIBRARY_PATH", &sidecar.lib_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        let child = command
            .spawn()
            .map_err(|e| LlmError::Load(format!("failed to start llama-server: {e}")))?;

        let mut engine = Self {
            child,
            base_url: format!("http://127.0.0.1:{port}"),
            client: reqwest::Client::new(),
            label: entry.label.clone(),
        };
        engine.await_ready().await?;
        Ok(engine)
    }

    async fn await_ready(&mut self) -> Result<(), LlmError> {
        let health = format!("{}/health", self.base_url);
        for _ in 0..HEALTH_ATTEMPTS {
            if let Some(status) = self
                .child
                .try_wait()
                .map_err(|e| LlmError::Load(e.to_string()))?
            {
                return Err(LlmError::Load(format!(
                    "llama-server exited during startup ({status})"
                )));
            }
            if let Ok(response) = self.client.get(&health).send().await {
                if response.status().is_success() {
                    return Ok(());
                }
            }
            tokio::time::sleep(HEALTH_INTERVAL).await;
        }
        Err(LlmError::Load(
            "llama-server did not become ready in time".to_string(),
        ))
    }

    pub async fn infer<F>(
        &self,
        system: Option<&str>,
        prompt: &str,
        params: &InferenceParams,
        on_token: F,
    ) -> Result<String, LlmError>
    where
        F: FnMut(GenerationChunk) + Send,
    {
        let mut messages = Vec::new();
        if let Some(system) = system {
            if !system.trim().is_empty() {
                messages.push(serde_json::json!({ "role": "system", "content": system }));
            }
        }
        messages.push(serde_json::json!({ "role": "user", "content": prompt }));

        let body = serde_json::json!({
            "model": "local",
            "stream": true,
            "max_tokens": params.max_tokens,
            "temperature": params.temperature,
            "top_p": params.top_p,
            "messages": messages,
        });

        let response = self
            .client
            .post(format!("{}/v1/chat/completions", self.base_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| LlmError::Inference(e.to_string()))?;

        if !response.status().is_success() {
            let detail = response.text().await.unwrap_or_default();
            return Err(LlmError::Inference(format!("llama-server error: {detail}")));
        }

        let stream = response
            .bytes_stream()
            .map(|chunk| chunk.map_err(|e| LlmError::Inference(e.to_string())));
        read_completion_stream(Box::pin(stream), STREAM_IDLE_TIMEOUT, on_token).await
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

impl Drop for LocalLlmEngine {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn read_completion_stream<S, B, F>(
    mut stream: S,
    idle_timeout: Duration,
    mut on_token: F,
) -> Result<String, LlmError>
where
    S: futures_util::Stream<Item = Result<B, LlmError>> + Unpin,
    B: AsRef<[u8]>,
    F: FnMut(GenerationChunk),
{
    let mut pending: Vec<u8> = Vec::new();
    let mut full = String::new();
    let mut saw_terminal = false;

    loop {
        let next = match tokio::time::timeout(idle_timeout, stream.next()).await {
            Ok(item) => item,
            Err(_) => {
                return Err(LlmError::Timeout(
                    "local model stopped responding".to_string(),
                ))
            }
        };
        let Some(chunk) = next else {
            break;
        };
        pending.extend_from_slice(chunk?.as_ref());

        while let Some(pos) = pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = pending.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let parsed = parse_sse_line(&line);
            if let Some(error) = parsed.error {
                return Err(LlmError::Inference(format!("llama-server error: {error}")));
            }
            if let Some(token) = parsed.token {
                if !token.is_empty() {
                    full.push_str(&token);
                    on_token(GenerationChunk {
                        text: token,
                        done: false,
                    });
                }
            }
            if parsed.finished {
                saw_terminal = true;
            }
        }
    }

    if !saw_terminal {
        return Err(LlmError::Inference(
            "local model stream ended before completion".to_string(),
        ));
    }

    on_token(GenerationChunk {
        text: String::new(),
        done: true,
    });
    Ok(full)
}

#[derive(Default)]
struct SseLine {
    token: Option<String>,
    finished: bool,
    error: Option<String>,
}

fn parse_sse_line(line: &str) -> SseLine {
    let Some(data) = line.trim().strip_prefix("data:") else {
        return SseLine::default();
    };
    let data = data.trim();
    if data.is_empty() {
        return SseLine::default();
    }
    if data == "[DONE]" {
        return SseLine {
            finished: true,
            ..SseLine::default()
        };
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
        return SseLine::default();
    };
    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(|message| message.as_str())
            .map(|message| message.to_string())
            .unwrap_or_else(|| error.to_string());
        return SseLine {
            error: Some(message),
            ..SseLine::default()
        };
    }
    let choice = value.get("choices").and_then(|choices| choices.get(0));
    let token = choice
        .and_then(|choice| choice.get("delta"))
        .and_then(|delta| delta.get("content"))
        .and_then(|content| content.as_str())
        .map(|content| content.to_string());
    let finished = choice
        .and_then(|choice| choice.get("finish_reason"))
        .map(|reason| !reason.is_null())
        .unwrap_or(false);
    SseLine {
        token,
        finished,
        error: None,
    }
}

fn free_port() -> Result<u16, LlmError> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| LlmError::Load(format!("failed to allocate a port: {e}")))?;
    listener
        .local_addr()
        .map(|addr| addr.port())
        .map_err(|e| LlmError::Load(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::parse_sse_line;

    #[test]
    fn parses_content_token_without_terminating() {
        let parsed = parse_sse_line(
            r#"data: {"choices":[{"delta":{"content":"hello"},"finish_reason":null}]}"#,
        );
        assert_eq!(parsed.token.as_deref(), Some("hello"));
        assert!(!parsed.finished);
        assert!(parsed.error.is_none());
    }

    #[test]
    fn done_marker_is_terminal() {
        let parsed = parse_sse_line("data: [DONE]");
        assert!(parsed.finished);
        assert!(parsed.token.is_none());
        assert!(parsed.error.is_none());
    }

    #[test]
    fn finish_reason_is_terminal() {
        let parsed =
            parse_sse_line(r#"data: {"choices":[{"delta":{},"finish_reason":"stop"}]}"#);
        assert!(parsed.finished);
    }

    #[test]
    fn error_payload_is_surfaced() {
        let parsed = parse_sse_line(r#"data: {"error":{"message":"context overflow"}}"#);
        assert_eq!(parsed.error.as_deref(), Some("context overflow"));
        assert!(!parsed.finished);
    }

    #[test]
    fn keepalive_and_malformed_lines_are_ignored() {
        let comment = parse_sse_line(": ping");
        assert!(comment.token.is_none() && !comment.finished && comment.error.is_none());
        let malformed = parse_sse_line("data: not-json");
        assert!(malformed.token.is_none() && !malformed.finished && malformed.error.is_none());
    }

    fn chunk(line: &str) -> Result<Vec<u8>, super::LlmError> {
        Ok(line.as_bytes().to_vec())
    }

    #[tokio::test]
    async fn stream_assembles_tokens_until_terminal() {
        let chunks = vec![
            chunk("data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n"),
            chunk("data: {\"choices\":[{\"delta\":{\"content\":\"lo\"},\"finish_reason\":\"stop\"}]}\n"),
            chunk("data: [DONE]\n"),
        ];
        let mut text = String::new();
        let result = super::read_completion_stream(
            futures_util::stream::iter(chunks),
            std::time::Duration::from_secs(5),
            |chunk| text.push_str(&chunk.text),
        )
        .await;
        assert_eq!(result.unwrap(), "Hello");
        assert_eq!(text, "Hello");
    }

    #[tokio::test]
    async fn stream_without_terminal_marker_is_error() {
        let chunks = vec![chunk("data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n")];
        let result = super::read_completion_stream(
            futures_util::stream::iter(chunks),
            std::time::Duration::from_secs(5),
            |_| {},
        )
        .await;
        assert!(matches!(result, Err(super::LlmError::Inference(_))));
    }

    #[tokio::test]
    async fn stream_idle_timeout_is_timeout_error() {
        let stream = futures_util::stream::pending::<Result<Vec<u8>, super::LlmError>>();
        let result = super::read_completion_stream(
            stream,
            std::time::Duration::from_millis(20),
            |_| {},
        )
        .await;
        assert!(matches!(result, Err(super::LlmError::Timeout(_))));
    }
}
