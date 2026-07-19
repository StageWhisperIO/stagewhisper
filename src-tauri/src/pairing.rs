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
    validate_relay_url(url)?;
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

pub fn validate_relay_url(raw: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(raw.trim())
        .map_err(|_| "The relay URL is invalid.".to_string())?;
    match parsed.scheme() {
        "https" => Ok(()),
        "http" => {
            let host = parsed
                .host_str()
                .ok_or_else(|| "The relay URL is invalid.".to_string())?;
            if host_is_private(host) {
                Ok(())
            } else {
                Err("Plain http is only allowed for local and private-network addresses. Use https for hosts on the public internet, or point at your Tailscale address.".to_string())
            }
        }
        _ => Err("The relay URL must start with http or https.".to_string()),
    }
}

fn host_is_private(host: &str) -> bool {
    let normalized = host
        .trim_end_matches('.')
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    if let Ok(ip) = normalized.parse::<std::net::IpAddr>() {
        return ip_is_private(ip);
    }
    normalized == "localhost"
        || normalized.ends_with(".localhost")
        || !normalized.contains('.')
        || normalized.ends_with(".local")
        || normalized.ends_with(".internal")
        || normalized.ends_with(".ts.net")
}

fn ip_is_private(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let octets = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || (octets[0] == 100 && (octets[1] & 0b1100_0000) == 64)
        }
        std::net::IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            v6.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    }
}

pub fn relay_is_loopback(raw: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(raw.trim()) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let normalized = host
        .trim_end_matches('.')
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    if let Ok(ip) = normalized.parse::<std::net::IpAddr>() {
        return ip.is_loopback();
    }
    normalized == "localhost" || normalized.ends_with(".localhost")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_is_loopback_detects_local_assistants() {
        assert!(relay_is_loopback("http://127.0.0.1:8765"));
        assert!(relay_is_loopback("http://localhost:8765"));
        assert!(relay_is_loopback("http://[::1]:8765"));
    }

    #[test]
    fn relay_is_loopback_rejects_remote_assistants() {
        assert!(!relay_is_loopback("https://host.tail34b074.ts.net"));
        assert!(!relay_is_loopback("http://192.168.1.5:8765"));
    }

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

    #[test]
    fn rejects_http_public_domain() {
        let code = encode(r#"{"url":"http://example.com:8765","token":"t0ken"}"#);
        let err = parse_pairing_code(&code).unwrap_err();
        assert!(err.contains("Plain http"));
    }

    #[test]
    fn rejects_http_public_ip() {
        let code = encode(r#"{"url":"http://203.0.113.10:8765","token":"t0ken"}"#);
        assert!(parse_pairing_code(&code).is_err());
    }

    #[test]
    fn accepts_https_public_domain() {
        let code = encode(r#"{"url":"https://relay.example.com","token":"t0ken"}"#);
        assert!(parse_pairing_code(&code).is_ok());
    }

    #[test]
    fn accepts_http_loopback() {
        assert!(validate_relay_url("http://127.0.0.1:8765").is_ok());
        assert!(validate_relay_url("http://localhost:8765").is_ok());
        assert!(validate_relay_url("http://[::1]:8765").is_ok());
    }

    #[test]
    fn accepts_http_private_ranges() {
        assert!(validate_relay_url("http://192.168.1.20:8765").is_ok());
        assert!(validate_relay_url("http://10.0.0.5:8765").is_ok());
        assert!(validate_relay_url("http://172.16.0.9:8765").is_ok());
    }

    #[test]
    fn accepts_http_tailscale_addresses() {
        assert!(validate_relay_url("http://100.101.102.103:8765").is_ok());
        assert!(validate_relay_url("http://my-host.tailnet-name.ts.net:8765").is_ok());
    }

    #[test]
    fn rejects_http_cgnat_lookalike_outside_range() {
        assert!(validate_relay_url("http://100.20.30.40:8765").is_err());
        assert!(validate_relay_url("http://100.128.0.1:8765").is_err());
    }

    #[test]
    fn accepts_http_single_label_and_local_suffixes() {
        assert!(validate_relay_url("http://hermes-box:8765").is_ok());
        assert!(validate_relay_url("http://studio.local:8765").is_ok());
        assert!(validate_relay_url("http://relay.internal:8765").is_ok());
    }

    #[test]
    fn rejects_non_http_schemes() {
        assert!(validate_relay_url("ftp://127.0.0.1:8765").is_err());
        assert!(validate_relay_url("ws://127.0.0.1:8765").is_err());
    }
}
