use base64::Engine;
use serde::Deserialize;

pub const PAIRING_CODE_PREFIX: &str = "stagewhisper-pair:v1:";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PairedRelay {
    pub url: String,
    pub token: String,
    #[serde(default)]
    pub label: Option<String>,
}

pub fn parse_pairing_code(code: &str) -> Result<PairedRelay, String> {
    let trimmed = code.trim();
    let payload = trimmed
        .strip_prefix(PAIRING_CODE_PREFIX)
        .ok_or_else(|| "This is not a StageWhisper pairing code.".to_string())?;

    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim())
        .map_err(|_| "Pairing code is malformed.".to_string())?;

    let relay: PairedRelay =
        serde_json::from_slice(&decoded).map_err(|_| "Pairing code is malformed.".to_string())?;

    let url = relay.url.trim();
    let parsed = reqwest::Url::parse(url)
        .map_err(|_| "Pairing code has an invalid relay URL.".to_string())?;
    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err("Pairing code has an invalid relay URL.".to_string());
    }
    if relay.token.trim().is_empty() {
        return Err("Pairing code is missing the relay token.".to_string());
    }

    Ok(PairedRelay {
        url: url.to_string(),
        token: relay.token.trim().to_string(),
        label: relay
            .label
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(json: &str) -> String {
        format!(
            "{PAIRING_CODE_PREFIX}{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json.as_bytes())
        )
    }

    #[test]
    fn parses_valid_code() {
        let code = encode(
            r#"{"url":"http://127.0.0.1:8765","token":"supersecrettoken","label":"OpenClaw"}"#,
        );
        let relay = parse_pairing_code(&code).unwrap();
        assert_eq!(relay.url, "http://127.0.0.1:8765");
        assert_eq!(relay.token, "supersecrettoken");
        assert_eq!(relay.label.as_deref(), Some("OpenClaw"));
    }

    #[test]
    fn parses_without_label() {
        let code = encode(r#"{"url":"https://relay.local","token":"t0ken"}"#);
        let relay = parse_pairing_code(&code).unwrap();
        assert_eq!(relay.label, None);
    }

    #[test]
    fn trims_surrounding_whitespace() {
        let code = encode(r#"{"url":"http://127.0.0.1:8765","token":"t0ken"}"#);
        let padded = format!("  {code}\n");
        assert!(parse_pairing_code(&padded).is_ok());
    }

    #[test]
    fn rejects_wrong_prefix() {
        let err = parse_pairing_code("totally-not-a-code").unwrap_err();
        assert!(err.contains("not a StageWhisper pairing code"));
    }

    #[test]
    fn rejects_wrong_version() {
        let code = format!(
            "stagewhisper-pair:v2:{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b"{}")
        );
        assert!(parse_pairing_code(&code).is_err());
    }

    #[test]
    fn rejects_bad_base64() {
        let code = format!("{PAIRING_CODE_PREFIX}!!!not base64!!!");
        assert!(parse_pairing_code(&code).is_err());
    }

    #[test]
    fn rejects_bad_json() {
        let code = encode("not json at all");
        assert!(parse_pairing_code(&code).is_err());
    }

    #[test]
    fn rejects_non_http_scheme() {
        let code = encode(r#"{"url":"ftp://127.0.0.1:8765","token":"t0ken"}"#);
        assert!(parse_pairing_code(&code).is_err());
    }

    #[test]
    fn accepts_code_emitted_by_hermes_cli() {
        let code = "stagewhisper-pair:v1:eyJ1cmwiOiJodHRwOi8vMTI3LjAuMC4xOjg3NjUiLCJ0b2tlbiI6InNla3JldHRva2VuMTIzIiwibGFiZWwiOiJIZXJtZXMifQ";
        let relay = parse_pairing_code(code).unwrap();
        assert_eq!(relay.url, "http://127.0.0.1:8765");
        assert_eq!(relay.token, "sekrettoken123");
        assert_eq!(relay.label.as_deref(), Some("Hermes"));
    }

    #[test]
    fn rejects_empty_token() {
        let code = encode(r#"{"url":"http://127.0.0.1:8765","token":"   "}"#);
        assert!(parse_pairing_code(&code).is_err());
    }
}
