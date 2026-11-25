//! Wire protocol types matching `crates/link/src/lib.rs` (`ALPN = crossagents/link/1`).

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use iroh::{EndpointAddr, EndpointId, RelayUrl};
use serde::{Deserialize, Serialize};

pub const ALPN: &[u8] = b"crossagents/link/1";
pub const CODE_PREFIX: &str = "ca1-";

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

    pub fn endpoint_addr(&self) -> Result<(EndpointAddr, Option<RelayUrl>), String> {
        build_endpoint_addr(&self.endpoint, self.relay.as_deref(), self.custom_relay)
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
    pub fn endpoint_addr(&self) -> Result<(EndpointAddr, Option<RelayUrl>), String> {
        build_endpoint_addr(&self.endpoint, self.relay.as_deref(), self.custom_relay)
    }
}

pub fn build_endpoint_addr(
    endpoint: &str,
    relay: Option<&str>,
    custom_relay: bool,
) -> Result<(EndpointAddr, Option<RelayUrl>), String> {
    let id = endpoint
        .trim()
        .parse::<EndpointId>()
        .map_err(|e| format!("无效的中心公钥：{e}"))?;
    let relay_url = match relay.map(str::trim).filter(|r| !r.is_empty()) {
        Some(raw) => Some(
            raw.parse::<RelayUrl>()
                .map_err(|e| format!("无效的中继地址 {raw}：{e}"))?,
        ),
        None => None,
    };
    let mut addr = EndpointAddr::new(id);
    if let Some(url) = &relay_url {
        addr = addr.with_relay_url(url.clone());
    }
    let custom = relay_url.filter(|_| custom_relay);
    Ok((addr, custom))
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
    fn decodes_valid_invite_and_rejects_bad_prefix() {
        assert!(Invite::decode("bad-code").is_err());
        let inv = Invite {
            endpoint: "6f1758b74080c9b676b8987e6aa84e4dff382096d2c2dd375c670d75e2d12d5a".into(),
            relay: Some("https://usw1-1.relay.n0.iroh.link./".into()),
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
        let (addr, custom) = decoded.endpoint_addr().unwrap();
        assert_eq!(addr.id.to_string(), inv.endpoint);
        assert!(custom.is_none());
    }
}
