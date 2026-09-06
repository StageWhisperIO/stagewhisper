use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::relay_settings::RelaySettings;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub enum RelayError {
    NotConfigured,
    InvalidUrl(String),
    Http(reqwest::Error),
    Status(StatusCode, String),
}

impl std::fmt::Display for RelayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RelayError::NotConfigured => write!(f, "relay url or token is empty"),
            RelayError::InvalidUrl(u) => write!(f, "invalid relay url: {u}"),
            RelayError::Http(e) => write!(f, "relay http error: {e}"),
            RelayError::Status(s, body) => write!(f, "relay rejected request: {s} {body}"),
        }
    }
}

impl std::error::Error for RelayError {}

pub struct RelayClient {
    http: Client,
    session_id: Uuid,
}

impl RelayClient {
    pub fn new() -> Result<Self> {
        let http = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .no_proxy()
            .build()
            .context("building relay http client")?;
        Ok(Self {
            http,
            session_id: Uuid::new_v4(),
        })
    }

    pub fn reset_session(&mut self) -> Uuid {
        self.session_id = Uuid::new_v4();
        self.session_id
    }

    pub async fn send_session_chat(
        &self,
        settings: &RelaySettings,
        relay_session_id: Uuid,
        text: String,
        parent_message_id: Option<String>,
        task_id: Uuid,
    ) -> std::result::Result<(), RelayError> {
        if !settings.has_relay() {
            return Err(RelayError::NotConfigured);
        }
        let mut payload = json!({ "text": text });
        if let Some(parent) = parent_message_id.clone() {
            payload["user_message_id"] = Value::String(parent.clone());
            payload["parent_message_id"] = Value::String(parent);
        }
        let body = json!({
            "task_id": task_id,
            "session_id": relay_session_id,
            "reason": "chat_message",
            "occurred_at": chrono::Utc::now().to_rfc3339(),
            "payload": payload,
        });
        self.post_incoming(settings, task_id, body).await
    }

    async fn post_incoming(
        &self,
        settings: &RelaySettings,
        task_id: Uuid,
        body: Value,
    ) -> std::result::Result<(), RelayError> {
        let url = build_url(&settings.relay_url, "/v1/incoming")?;
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&settings.relay_token)
            .header("Idempotency-Key", task_id.to_string())
            .json(&body)
            .send()
            .await
            .map_err(RelayError::Http)?;

        let status = resp.status();
        if status == StatusCode::ACCEPTED || status.is_success() {
            return Ok(());
        }
        if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::SERVICE_UNAVAILABLE {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let body = resp.text().await.unwrap_or_default();
            return Err(RelayError::Status(StatusCode::TOO_MANY_REQUESTS, body));
        }
        let text = resp.text().await.unwrap_or_default();
        Err(RelayError::Status(status, text))
    }
}

fn build_url(base: &str, relative_path: &str) -> std::result::Result<String, RelayError> {
    let trimmed_base = base.trim_end_matches('/');
    if trimmed_base.is_empty() {
        return Err(RelayError::InvalidUrl(base.to_string()));
    }
    let parsed =
        reqwest::Url::parse(trimmed_base).map_err(|_| RelayError::InvalidUrl(base.to_string()))?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(RelayError::InvalidUrl(base.to_string()));
    }
    let suffix = if relative_path.starts_with('/') {
        relative_path.to_string()
    } else {
        format!("/{relative_path}")
    };
    Ok(format!("{trimmed_base}{suffix}"))
}

#[cfg(test)]
#[path = "relay_tests.rs"]
mod tests;
