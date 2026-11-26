//! Wire protocol types matching `crates/link/src/lib.rs` (`ALPN = crossagents/link/1`).

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use iroh::{EndpointAddr, EndpointId, RelayUrl};
use serde::{Deserialize, Serialize};
use url::Url;

pub const ALPN: &[u8] = b"crossagents/link/1";
pub const CODE_PREFIX: &str = "ca1-";

/// Browser-safe n0 production relays WITHOUT trailing dots in the hostname.
///
/// Desktop `iroh` formats default relay hostnames as absolute FQDNs ending in `.`
/// (e.g. `usw1-1.relay.n0.iroh.link.`). Apple WebKit / iOS Safari can fail or stall
/// TLS SNI / WebSocket upgrades when a trailing dot is present, while the relay servers
/// accept the clean hostname identically and route strictly by `EndpointId`.
pub const DEFAULT_N0_RELAYS_NO_DOT: &[&str] = &[
    "https://usw1-1.relay.n0.iroh.link/",
    "https://euc1-1.relay.n0.iroh.link/",
    "https://aps1-1.relay.n0.iroh.link/",
    "https://use1-1.relay.n0.iroh.link/",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Invite {
    #[serde(rename = "e")]
    pub endpoint: String,
    #[serde(rename = "r", default, skip_serializing_if = "Option::is_none")]
    pub relay: Option<String>,
    #[serde(rename = "c", default, skip_serializing_if = "std::ops::Not::not")]
    pub custom_relay: bool,
    #[serde(rename = "a", default, skip_serializing_if = "Vec::is_empty")]
    pub addrs: Vec<String>,
    #[serde(rename = "s")]
    pub secret: String,
    #[serde(rename = "h", default)]
    pub host: String,
    #[serde(rename = "x")]
    pub expires: i64,
}

impl Invite {
    pub fn decode(code: &str) -> Result<Self, String> {
        let cleaned: String = code.chars().filter(|c| !c.is_whitespace()).collect();
        let Some(body) = cleaned.strip_prefix(CODE_PREFIX) else {
            return Err("不是有效的 CrossAgents 配对码（应以 ca1- 开头）".into());
        };
        let bytes = URL_SAFE_NO_PAD
            .decode(body)
            .map_err(|e| format!("配对码内容损坏（base64url）：{e}"))?;
        let invite: Invite =
            serde_json::from_slice(&bytes).map_err(|e| format!("配对码格式损坏：{e}"))?;
        invite
            .endpoint
            .parse::<EndpointId>()
            .map_err(|e| format!("配对码中不包含有效的中心公钥：{e}"))?;
        Ok(invite)
    }

    pub fn parsed_target(&self) -> Result<(EndpointId, Option<RelayUrl>, bool), String> {
        parse_target(&self.endpoint, self.relay.as_deref(), self.custom_relay)
    }

    pub fn sanitized_relay_string(&self) -> Option<String> {
        self.relay
            .as_deref()
            .and_then(|r| sanitize_relay_url(r).ok())
            .map(|u| u.to_string())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TunnelConfig {
    pub endpoint: String,
    #[serde(default)]
    pub relay: Option<String>,
    #[serde(default)]
    pub custom_relay: bool,
    pub token: String,
    #[serde(default)]
    pub os: Option<String>,
}

impl TunnelConfig {
    pub fn parsed_target(&self) -> Result<(EndpointId, Option<RelayUrl>, bool), String> {
        parse_target(&self.endpoint, self.relay.as_deref(), self.custom_relay)
    }
}

/// Parses a relay URL and removes any trailing dot from its domain name so iOS Safari / WebKit
/// `fetch()` and `new WebSocket()` never trip over absolute FQDN trailing dots.
pub fn sanitize_relay_url(raw: &str) -> Result<RelayUrl, String> {
    let trimmed = raw.trim();
    let mut parsed =
        Url::parse(trimmed).map_err(|e| format!("无效的中继地址 {trimmed}：{e}"))?;
    if let Some(host) = parsed.host_str() {
        let clean_host = host.trim_end_matches('.');
        if !clean_host.is_empty() && clean_host.len() != host.len() {
            let owned = clean_host.to_owned();
            parsed
                .set_host(Some(&owned))
                .map_err(|e| format!("无法规范化中继域名 {trimmed}：{e}"))?;
        }
    }
    Ok(RelayUrl::from(parsed))
}

pub fn parse_target(
    endpoint: &str,
    relay: Option<&str>,
    custom_relay: bool,
) -> Result<(EndpointId, Option<RelayUrl>, bool), String> {
    let id = endpoint
        .trim()
        .parse::<EndpointId>()
        .map_err(|e| format!("无效的中心公钥：{e}"))?;
    let relay_url = match relay.map(str::trim).filter(|r| !r.is_empty()) {
        Some(raw) => Some(sanitize_relay_url(raw)?),
        None => None,
    };
    Ok((id, relay_url, custom_relay))
}

pub fn endpoint_addr_for(id: EndpointId, relay: &RelayUrl) -> EndpointAddr {
    EndpointAddr::new(id).with_relay_url(relay.clone())
}

/// Builds the ordered list of relays to try in the browser:
/// - Custom relay: ONLY the hub's custom relay.
/// - Public n0 relay: the hub's advertised relay first (sanitized), followed by the remaining
///   n0 relays without trailing dots in case the hub switched home relays after pairing.
pub fn candidate_relays(
    preferred: Option<RelayUrl>,
    custom_relay: bool,
) -> Result<Vec<RelayUrl>, String> {
    if custom_relay {
        let relay = preferred.ok_or_else(|| {
            "中心配置了自建中继，但配对信息里缺少中继地址，请重新扫码配对".to_owned()
        })?;
        return Ok(vec![relay]);
    }
    let mut list = Vec::with_capacity(DEFAULT_N0_RELAYS_NO_DOT.len() + 1);
    if let Some(pref) = preferred {
        list.push(pref);
    }
    for raw in DEFAULT_N0_RELAYS_NO_DOT {
        if let Ok(url) = sanitize_relay_url(raw)
            && !list.contains(&url)
        {
            list.push(url);
        }
    }
    Ok(list)
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Hello {
    Tunnel {
        token: String,
    },
    Redeem {
        secret: String,
        name: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        os: Option<String>,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct Reply {
    pub ok: bool,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub paired: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_trailing_dot_and_preserves_prefix() {
        let a = sanitize_relay_url("https://usw1-1.relay.n0.iroh.link./").unwrap();
        assert_eq!(a.as_str(), "https://usw1-1.relay.n0.iroh.link/");

        let b = sanitize_relay_url("https://relay.example.com./custom").unwrap();
        assert_eq!(b.as_str(), "https://relay.example.com/custom/");

        let c = sanitize_relay_url("https://aps1-1.relay.n0.iroh.link").unwrap();
        assert_eq!(c.as_str(), "https://aps1-1.relay.n0.iroh.link/");
    }

    #[test]
    fn decodes_valid_invite_and_orders_candidate_relays() {
        assert!(Invite::decode("bad-code").is_err());
        let inv = Invite {
            endpoint: "6f1758b74080c9b676b8987e6aa84e4dff382096d2c2dd375c670d75e2d12d5a".into(),
            relay: Some("https://euc1-1.relay.n0.iroh.link./".into()),
            custom_relay: false,
            addrs: vec!["192.168.1.5:5555".into()],
            secret: "abc123".into(),
            host: "DESKTOP-TEST".into(),
            expires: 4_000_000_000,
        };
        let encoded = format!(
            "{CODE_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&inv).unwrap())
        );
        let decoded = Invite::decode(&encoded).unwrap();
        assert_eq!(decoded, inv);
        let (id, relay, custom) = decoded.parsed_target().unwrap();
        assert_eq!(id.to_string(), inv.endpoint);
        assert!(!custom);
        assert_eq!(
            relay.as_ref().unwrap().as_str(),
            "https://euc1-1.relay.n0.iroh.link/"
        );

        let candidates = candidate_relays(relay, custom).unwrap();
        assert_eq!(candidates.len(), 4);
        assert_eq!(candidates[0].as_str(), "https://euc1-1.relay.n0.iroh.link/");
        assert!(candidates.iter().all(|u| !u.host_str().unwrap().ends_with('.')));

        let custom_only = candidate_relays(
            Some(sanitize_relay_url("https://my.relay.org./iroh").unwrap()),
            true,
        )
        .unwrap();
        assert_eq!(custom_only.len(), 1);
        assert_eq!(custom_only[0].as_str(), "https://my.relay.org/iroh/");
    }
}
