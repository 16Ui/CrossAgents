//! Pure-Rust Web Push (RFC 8291 `aes128gcm` + RFC 8292 VAPID ES256) so iOS Home-Screen PWAs
//! receive free native lock-screen approval alerts directly from the hub without an Apple
//! Developer Program membership.
//!
//! Payloads pushed through Apple/Google/Mozilla push relays are intentionally redacted: they carry
//! only a generic reminder + opaque item id, never commands, diffs, or workspace paths.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use crossagents_core::{EventBus, EventKind};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use web_push_native::jwt_simple::algorithms::ES256KeyPair;
use web_push_native::p256::elliptic_curve::sec1::ToEncodedPoint;
use web_push_native::p256::{PublicKey, SecretKey};
use web_push_native::{Auth, WebPushBuilder};

use crate::access::Grant;
use crate::api::{ApiResult, AppError, AppState};

const VAPID_SUBJECT: &str = "mailto:crossagents-hub@localhost.invalid";
const MAX_SUBSCRIPTIONS: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushSubscriptionRecord {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct VapidKeyResponse {
    pub public_key: String,
}

#[derive(Debug, Deserialize)]
pub struct SubscribeRequest {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
}

#[derive(Debug, Serialize)]
pub struct SubscribeResponse {
    pub ok: bool,
    pub count: usize,
}

#[derive(Debug, Deserialize)]
pub struct UnsubscribeRequest {
    pub endpoint: String,
}

#[derive(Debug, Default, Deserialize)]
pub struct TestPushRequest {
    #[serde(default)]
    pub endpoint: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TestPushResponse {
    pub sent: usize,
    pub failed: usize,
}

pub struct WebPushService {
    home: PathBuf,
    key_pair: ES256KeyPair,
    public_key: String,
    subs: Mutex<Vec<PushSubscriptionRecord>>,
    client: reqwest::Client,
}

impl WebPushService {
    pub fn new(home: &Path) -> Self {
        let (key_pair, public_key) = load_or_create_vapid_key(&home.join("web-push-vapid.key"))
            .unwrap_or_else(|err| {
                tracing::warn!(error = %err, "web_push: falling back to ephemeral VAPID key");
                generate_vapid_pair()
            });
        let subs = load_subscriptions(&home.join("web-push-subscriptions.json"));
        let client = build_http_client();
        Self {
            home: home.to_path_buf(),
            key_pair,
            public_key,
            subs: Mutex::new(subs),
            client,
        }
    }

    pub fn public_key(&self) -> &str {
        &self.public_key
    }

    fn subs_path(&self) -> PathBuf {
        self.home.join("web-push-subscriptions.json")
    }

    fn save_subs(&self, subs: &[PushSubscriptionRecord]) {
        if let Ok(bytes) = serde_json::to_vec_pretty(subs)
            && let Err(err) = crate::paths::write_atomic(&self.subs_path(), &bytes)
        {
            tracing::warn!(error = %err, "web_push: cannot persist subscriptions");
        }
    }

    pub async fn subscribe(
        &self,
        device_id: Option<String>,
        endpoint: String,
        p256dh: String,
        auth: String,
    ) -> Result<usize, String> {
        let endpoint = endpoint.trim().to_owned();
        if !endpoint.starts_with("https://") {
            return Err("Web Push endpoint 必须是 https:// 地址".into());
        }
        parse_subscription_keys(&endpoint, &p256dh, &auth)?;

        let mut subs = self.subs.lock().await;
        if let Some(existing) = subs.iter_mut().find(|s| s.endpoint == endpoint) {
            existing.p256dh = p256dh;
            existing.auth = auth;
            existing.device_id = device_id;
            existing.created_at = Utc::now();
        } else {
            subs.push(PushSubscriptionRecord {
                endpoint,
                p256dh,
                auth,
                device_id,
                created_at: Utc::now(),
            });
            if subs.len() > MAX_SUBSCRIPTIONS {
                let overflow = subs.len() - MAX_SUBSCRIPTIONS;
                subs.drain(0..overflow);
            }
        }
        let count = subs.len();
        self.save_subs(&subs);
        Ok(count)
    }

    pub async fn unsubscribe(&self, endpoint: &str) -> usize {
        let mut subs = self.subs.lock().await;
        let before = subs.len();
        subs.retain(|s| s.endpoint != endpoint.trim());
        if subs.len() != before {
            self.save_subs(&subs);
        }
        subs.len()
    }

    pub async fn remove_device(&self, device_id: &str) {
        let mut subs = self.subs.lock().await;
        let before = subs.len();
        subs.retain(|s| s.device_id.as_deref() != Some(device_id));
        if subs.len() != before {
            self.save_subs(&subs);
        }
    }

    /// Starts the background event watcher dispatching redacted iOS lock-screen push reminders
    /// whenever an agent asks a question or requests approval.
    pub fn watch(self: Arc<Self>, bus: Arc<EventBus>, shutdown: CancellationToken) {
        let mut rx = bus.subscribe();
        tokio::spawn(async move {
            loop {
                let event = tokio::select! {
                    _ = shutdown.cancelled() => break,
                    msg = rx.recv() => match msg {
                        Ok(ev) => ev,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                };
                match event.kind {
                    EventKind::HumanAsked { item } => {
                        let body = match item.action {
                            Some(crossagents_core::HumanAction::Approval) => {
                                "有一项新的审批请求等待你确认"
                            }
                            Some(crossagents_core::HumanAction::Budget) => {
                                "有任务触达预算阈值，等待你决定"
                            }
                            Some(crossagents_core::HumanAction::Work) => {
                                "有新的 Work 提案等待你确认"
                            }
                            None => "有 Agent 发来一条待处理提问",
                        };
                        let payload = serde_json::json!({
                            "title": "CrossAgents",
                            "body": body,
                            "tag": format!("human-{}", item.id),
                            "question_id": item.id,
                            "agent_id": item.from.to_string(),
                        });
                        let svc = Arc::clone(&self);
                        tokio::spawn(async move {
                            let _ = svc.broadcast(&payload.to_string(), None).await;
                        });
                    }
                    EventKind::DeviceRevoked { device } => {
                        self.remove_device(&device.id).await;
                    }
                    _ => {}
                }
            }
        });
    }

    pub async fn send_test(&self, endpoint: Option<&str>) -> Result<(usize, usize), String> {
        let payload = serde_json::json!({
            "title": "CrossAgents",
            "body": "推送通道已就绪：之后有待办审批会第一时间在锁屏提醒你。",
            "tag": "crossagents-test",
        });
        let (sent, failed) = self.broadcast(&payload.to_string(), endpoint).await;
        if sent == 0 && failed == 0 {
            return Err("当前没有已注册的推送订阅，请先开启通知开关".into());
        }
        Ok((sent, failed))
    }

    fn build_request(
        &self,
        sub: &PushSubscriptionRecord,
        payload: &str,
    ) -> Result<axum::http::Request<Vec<u8>>, String> {
        let (uri, pub_key, auth) = parse_subscription_keys(&sub.endpoint, &sub.p256dh, &sub.auth)?;
        WebPushBuilder::new(uri, pub_key, auth)
            .with_vapid(&self.key_pair, VAPID_SUBJECT)
            .build(payload)
            .map_err(|e| format!("构造 Web Push 密文失败：{e}"))
    }

    async fn broadcast(&self, payload: &str, only_endpoint: Option<&str>) -> (usize, usize) {
        let targets: Vec<PushSubscriptionRecord> = {
            let subs = self.subs.lock().await;
            subs.iter()
                .filter(|s| only_endpoint.is_none_or(|ep| s.endpoint == ep))
                .cloned()
                .collect()
        };
        if targets.is_empty() {
            return (0, 0);
        }

        let mut sent = 0usize;
        let mut failed = 0usize;
        let mut gone_endpoints = Vec::<String>::new();

        for sub in targets {
            let req = match self.build_request(&sub, payload) {
                Ok(r) => r,
                Err(err) => {
                    tracing::warn!(endpoint = %sub.endpoint, error = %err, "web_push: build failed");
                    failed += 1;
                    continue;
                }
            };
            let mut builder = self
                .client
                .post(req.uri().to_string())
                .header("TTL", "3600")
                .header("Urgency", "high");
            for (name, val) in req.headers() {
                if let Ok(v) = val.to_str() {
                    builder = builder.header(name.as_str(), v);
                }
            }
            match builder.body(req.into_body()).send().await {
                Ok(resp) if resp.status().is_success() => {
                    sent += 1;
                }
                Ok(resp) if resp.status().as_u16() == 404 || resp.status().as_u16() == 410 => {
                    tracing::info!(endpoint = %sub.endpoint, status = %resp.status(), "web_push: subscription expired, removing");
                    gone_endpoints.push(sub.endpoint);
                    failed += 1;
                }
                Ok(resp) => {
                    let status = resp.status();
                    let text = resp.text().await.unwrap_or_default();
                    tracing::warn!(endpoint = %sub.endpoint, %status, body = %text, "web_push: delivery rejected");
                    failed += 1;
                }
                Err(err) => {
                    tracing::warn!(endpoint = %sub.endpoint, error = %err, "web_push: delivery error");
                    failed += 1;
                }
            }
        }

        if !gone_endpoints.is_empty() {
            let mut subs = self.subs.lock().await;
            subs.retain(|s| !gone_endpoints.contains(&s.endpoint));
            self.save_subs(&subs);
        }

        (sent, failed)
    }
}

fn build_http_client() -> reqwest::Client {
    let mut builder = reqwest::Client::builder().timeout(Duration::from_secs(15));
    if let Some(proxy_url) = crate::link::hub_proxy()
        && let Ok(proxy) = reqwest::Proxy::all(&proxy_url)
    {
        builder = builder.proxy(proxy);
    }
    builder.build().unwrap_or_else(|_| reqwest::Client::new())
}

fn generate_vapid_pair() -> (ES256KeyPair, String) {
    let key_pair = ES256KeyPair::generate();
    let public_key = derive_uncompressed_public_key(&key_pair.to_bytes())
        .expect("freshly generated ES256KeyPair yields valid P-256 secret");
    (key_pair, public_key)
}

fn derive_uncompressed_public_key(secret_bytes: &[u8]) -> Result<String, String> {
    let sk = SecretKey::from_slice(secret_bytes)
        .map_err(|e| format!("无效的 P-256 私钥字节：{e}"))?;
    let point = sk.public_key().to_encoded_point(false);
    Ok(URL_SAFE_NO_PAD.encode(point.as_bytes()))
}

fn load_or_create_vapid_key(path: &Path) -> Result<(ES256KeyPair, String), String> {
    if let Ok(raw) = std::fs::read_to_string(path) {
        let trimmed = raw.trim();
        if let Ok(bytes) = URL_SAFE_NO_PAD.decode(trimmed)
            && let Ok(kp) = ES256KeyPair::from_bytes(&bytes)
            && let Ok(pub_b64) = derive_uncompressed_public_key(&bytes)
        {
            return Ok((kp, pub_b64));
        }
    }
    let (kp, pub_b64) = generate_vapid_pair();
    let encoded = URL_SAFE_NO_PAD.encode(kp.to_bytes());
    crate::paths::write_atomic(path, encoded.as_bytes())
        .map_err(|e| format!("写入 VAPID 密钥失败：{e}"))?;
    Ok((kp, pub_b64))
}

fn load_subscriptions(path: &Path) -> Vec<PushSubscriptionRecord> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn decode_b64_flex(raw: &str) -> Result<Vec<u8>, String> {
    let trimmed = raw.trim().trim_end_matches('=');
    URL_SAFE_NO_PAD
        .decode(trimmed)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(trimmed))
        .map_err(|e| format!("base64 解码失败：{e}"))
}

fn parse_subscription_keys(
    endpoint: &str,
    p256dh: &str,
    auth: &str,
) -> Result<(axum::http::Uri, PublicKey, Auth), String> {
    let uri: axum::http::Uri = endpoint
        .parse()
        .map_err(|e| format!("无效的 endpoint URL：{e}"))?;
    let p256dh_bytes = decode_b64_flex(p256dh).map_err(|e| format!("无效的 p256dh：{e}"))?;
    let pub_key = PublicKey::from_sec1_bytes(&p256dh_bytes)
        .map_err(|e| format!("无效的 P-256 公钥：{e}"))?;
    let auth_bytes = decode_b64_flex(auth).map_err(|e| format!("无效的 auth：{e}"))?;
    let auth_arr: [u8; 16] = auth_bytes
        .try_into()
        .map_err(|v: Vec<u8>| format!("auth 密钥长度应为 16 字节，实际 {}", v.len()))?;
    let auth = Auth::from(auth_arr);
    Ok((uri, pub_key, auth))
}

// ---------------------------------------------------------------------------------------
// HTTP Routes

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/v1/push/vapid-key", get(vapid_key))
        .route(
            "/v1/push/subscriptions",
            post(subscribe).delete(unsubscribe),
        )
        .route("/v1/push/test", post(send_test))
}

async fn vapid_key(State(state): State<Arc<AppState>>) -> Json<VapidKeyResponse> {
    Json(VapidKeyResponse {
        public_key: state.web_push.public_key().to_owned(),
    })
}

async fn subscribe(
    State(state): State<Arc<AppState>>,
    Extension(grant): Extension<Grant>,
    Json(req): Json<SubscribeRequest>,
) -> ApiResult<Json<SubscribeResponse>> {
    let device_id = match grant {
        Grant::Device { id, .. } => Some(id),
        _ => None,
    };
    let count = state
        .web_push
        .subscribe(device_id, req.endpoint, req.p256dh, req.auth)
        .await
        .map_err(AppError::bad_request)?;
    Ok(Json(SubscribeResponse { ok: true, count }))
}

async fn unsubscribe(
    State(state): State<Arc<AppState>>,
    Json(req): Json<UnsubscribeRequest>,
) -> Json<SubscribeResponse> {
    let count = state.web_push.unsubscribe(&req.endpoint).await;
    Json(SubscribeResponse { ok: true, count })
}

async fn send_test(
    State(state): State<Arc<AppState>>,
    body: Option<Json<TestPushRequest>>,
) -> ApiResult<Json<TestPushResponse>> {
    let ep = body.and_then(|Json(b)| b.endpoint);
    let (sent, failed) = state
        .web_push
        .send_test(ep.as_deref())
        .await
        .map_err(AppError::bad_request)?;
    Ok(Json(TestPushResponse { sent, failed }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn vapid_key_persists_and_builds_encrypted_request() {
        let dir = tempfile::tempdir().unwrap();
        let svc1 = WebPushService::new(dir.path());
        let pub1 = svc1.public_key().to_owned();
        let pub_bytes = URL_SAFE_NO_PAD.decode(&pub1).unwrap();
        assert_eq!(pub_bytes.len(), 65, "uncompressed SEC1 P-256 key is 65 bytes");
        assert_eq!(pub_bytes[0], 0x04);

        // Reloading from the same home reuses the persisted VAPID key.
        let svc2 = WebPushService::new(dir.path());
        assert_eq!(svc2.public_key(), pub1);

        // Construct a valid dummy client subscription and verify AES-128-GCM + VAPID request build.
        let client_kp = ES256KeyPair::generate();
        let client_pub = derive_uncompressed_public_key(&client_kp.to_bytes()).unwrap();
        let client_auth = URL_SAFE_NO_PAD.encode([7u8; 16]);
        let endpoint = "https://web.push.apple.com/QExampleTestEndpoint123".to_owned();

        let count = svc2
            .subscribe(
                Some("dev-iphone".into()),
                endpoint.clone(),
                client_pub.clone(),
                client_auth.clone(),
            )
            .await
            .unwrap();
        assert_eq!(count, 1);

        let sub = PushSubscriptionRecord {
            endpoint: endpoint.clone(),
            p256dh: client_pub,
            auth: client_auth,
            device_id: Some("dev-iphone".into()),
            created_at: Utc::now(),
        };
        let req = svc2
            .build_request(&sub, "{\"title\":\"CrossAgents\",\"body\":\"test\"}")
            .unwrap();
        let auth_header = req
            .headers()
            .get("authorization")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(auth_header.starts_with("vapid "));
        assert!(auth_header.contains(&pub1));
        assert_eq!(
            req.headers().get("content-encoding").unwrap(),
            "aes128gcm"
        );
        assert!(!req.body().is_empty());

        svc2.remove_device("dev-iphone").await;
        assert_eq!(svc2.unsubscribe(&endpoint).await, 0);
    }
}
