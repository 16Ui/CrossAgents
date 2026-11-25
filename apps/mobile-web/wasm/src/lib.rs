//! CrossAgents browser WASM iroh transport (`crossagents-link-wasm`).
//!
//! Connects an iPhone/desktop browser PWA directly to a CrossAgents hub over end-to-end encrypted
//! iroh QUIC streams via relay WebSockets — reusing the exact `ca1-...` QR pairing codes and
//! `crossagents/link/1` protocol from `crates/link`.

pub mod http1;
pub mod protocol;

#[cfg(target_arch = "wasm32")]
mod wasm_api {
    use std::cell::RefCell;

    use iroh::endpoint::{Connection, RecvStream, SendStream, presets};
    use iroh::{Endpoint, RelayMode, RelayUrl};
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
    use tokio::sync::oneshot;
    use wasm_bindgen::prelude::*;

    use crate::http1::{build_http1_request, read_http1_response, stream_http1_sse};
    use crate::protocol::{ALPN, Hello, Invite, Reply, TunnelConfig};

    const MAX_HELLO_LINE: usize = 16 * 1024;

    #[wasm_bindgen(start)]
    pub fn init_wasm() {
        console_error_panic_hook::set_once();
    }

    /// Locally decodes and previews a `ca1-...` pairing code without network access.
    #[wasm_bindgen]
    pub fn decode_invite(code: &str) -> String {
        match Invite::decode(code) {
            Ok(inv) => json!({
                "ok": true,
                "host": inv.host,
                "expires": inv.expires,
                "has_relay": inv.relay.is_some(),
                "custom_relay": inv.custom_relay,
                "lan_addrs": inv.addrs.len(),
            })
            .to_string(),
            Err(error) => json!({ "ok": false, "error": error }).to_string(),
        }
    }

    /// Redeems a one-time `ca1-...` pairing code over an E2EE iroh relay connection.
    #[wasm_bindgen]
    pub async fn redeem_invite(code: &str, device_name: &str, os: &str) -> String {
        match redeem_inner(code, device_name, os).await {
            Ok(paired) => json!({ "ok": true, "paired": paired }).to_string(),
            Err(err) => json!({ "ok": false, "error": err }).to_string(),
        }
    }

    async fn bind_browser_endpoint(custom_relay: Option<RelayUrl>) -> Result<Endpoint, String> {
        let mut builder = Endpoint::builder(presets::N0);
        if let Some(relay) = custom_relay {
            builder = builder.relay_mode(RelayMode::custom([relay]));
        }
        builder
            .bind()
            .await
            .map_err(|e| format!("无法初始化浏览器 iroh 节点（可能是中继不可达）：{e}"))
    }

    async fn write_json_line(send: &mut SendStream, value: &Hello) -> Result<(), String> {
        let mut bytes =
            serde_json::to_vec(value).map_err(|e| format!("序列化握手消息失败：{e}"))?;
        bytes.push(b'\n');
        send.write_all(&bytes)
            .await
            .map_err(|e| format!("发送握手消息失败：{e}"))?;
        Ok(())
    }

    async fn read_reply_line(recv: &mut BufReader<RecvStream>) -> Result<Reply, String> {
        let mut line = Vec::new();
        (&mut *recv)
            .take(MAX_HELLO_LINE as u64)
            .read_until(b'\n', &mut line)
            .await
            .map_err(|e| format!("读取中心握手应答失败：{e}"))?;
        if line.last() != Some(&b'\n') {
            return Err("中心在完成握手前关闭了连接".into());
        }
        serde_json::from_slice(&line).map_err(|e| format!("中心返回了无效的 JSON 应答：{e}"))
    }

    async fn redeem_inner(
        code: &str,
        device_name: &str,
        os: &str,
    ) -> Result<serde_json::Value, String> {
        let invite = Invite::decode(code)?;
        let now_secs = (js_sys::Date::now() / 1000.0) as i64;
        if invite.expires <= now_secs {
            return Err("这个配对码已经过期，请在中心电脑重新生成一个".into());
        }
        let (addr, custom_relay) = invite.endpoint_addr()?;
        let endpoint = bind_browser_endpoint(custom_relay).await?;
        let name = device_name.trim();
        let name = if name.is_empty() { "iPhone" } else { name };
        let os = os.trim();
        let os = if os.is_empty() { "ios" } else { os };

        let outcome = async {
            let conn = endpoint
                .connect(addr, ALPN)
                .await
                .map_err(|e| format!("无法连接到中心（请确认中心电脑在线且已开启内置打洞）：{e}"))?;
            let (mut send, recv) = conn
                .open_bi()
                .await
                .map_err(|e| format!("无法打开加密流：{e}"))?;
            write_json_line(
                &mut send,
                &Hello::Redeem {
                    secret: invite.secret.clone(),
                    name: name.to_owned(),
                    os: Some(os.to_owned()),
                },
            )
            .await?;
            let mut recv = BufReader::new(recv);
            let reply = read_reply_line(&mut recv).await?;
            conn.close(0u32.into(), b"redeem-done");
            if !reply.ok {
                return Err(reply
                    .error
                    .unwrap_or_else(|| "中心拒绝了该配对码".into()));
            }
            let mut paired = reply
                .paired
                .ok_or_else(|| "中心配对回复中缺少设备凭据".to_owned())?;
            paired["host"] = invite.host.clone().into();
            paired["endpoint"] = invite.endpoint.clone().into();
            if let Some(relay) = &invite.relay {
                paired["relay"] = relay.clone().into();
            }
            paired["custom_relay"] = invite.custom_relay.into();
            paired["addrs"] = invite.addrs.clone().into();
            Ok(paired)
        }
        .await;

        endpoint.close().await;
        outcome
    }

    #[wasm_bindgen]
    pub struct WasmEventSubscription {
        cancel_tx: RefCell<Option<oneshot::Sender<()>>>,
    }

    #[wasm_bindgen]
    impl WasmEventSubscription {
        pub fn cancel(&self) {
            if let Some(tx) = self.cancel_tx.borrow_mut().take() {
                let _ = tx.send(());
            }
        }
    }

    #[wasm_bindgen]
    pub struct WasmLinkSession {
        endpoint: Endpoint,
        conn: Connection,
        token: String,
        client_os: String,
    }

    #[wasm_bindgen]
    impl WasmLinkSession {
        /// Opens a persistent end-to-end encrypted iroh connection to the hub.
        pub async fn connect(config_json: &str) -> Result<WasmLinkSession, JsValue> {
            let cfg: TunnelConfig = serde_json::from_str(config_json)
                .map_err(|e| js_err("bad_config", &format!("无效的连接配置：{e}")))?;
            let (addr, custom_relay) = cfg
                .endpoint_addr()
                .map_err(|e| js_err("bad_config", &e))?;
            let endpoint = bind_browser_endpoint(custom_relay)
                .await
                .map_err(|e| js_err("relay_unreachable", &e))?;
            let conn = match endpoint.connect(addr, ALPN).await {
                Ok(c) => c,
                Err(e) => {
                    endpoint.close().await;
                    return Err(js_err(
                        "hub_offline",
                        &format!("无法连到中心电脑（可能正在休眠、离线，或中继线路波动）：{e}"),
                    ));
                }
            };
            let client_os = cfg
                .os
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("ios")
                .to_owned();
            Ok(WasmLinkSession {
                endpoint,
                conn,
                token: cfg.token,
                client_os,
            })
        }

        async fn open_tunnel_stream(&self) -> Result<(SendStream, BufReader<RecvStream>), JsValue> {
            let (mut send, recv) = self
                .conn
                .open_bi()
                .await
                .map_err(|e| js_err("disconnected", &format!("隧道连接已断开：{e}")))?;
            write_json_line(
                &mut send,
                &Hello::Tunnel {
                    token: self.token.clone(),
                },
            )
            .await
            .map_err(|e| js_err("disconnected", &e))?;
            let mut recv = BufReader::new(recv);
            let reply = read_reply_line(&mut recv)
                .await
                .map_err(|e| js_err("disconnected", &e))?;
            if !reply.ok {
                let code = reply.code.as_deref().unwrap_or("error");
                let msg = reply
                    .error
                    .unwrap_or_else(|| "中心拒绝了隧道请求".into());
                return Err(js_err(code, &msg));
            }
            Ok((send, recv))
        }

        /// Executes one HTTP/1.1 REST API request through the encrypted iroh tunnel.
        /// Resolves to JSON `{"status":200,"body":"..."}`.
        pub async fn request(
            &self,
            method: &str,
            path: &str,
            body: Option<String>,
        ) -> Result<String, JsValue> {
            let (mut send, recv) = self.open_tunnel_stream().await?;
            let req_bytes = build_http1_request(
                method,
                path,
                &self.token,
                &self.client_os,
                body.as_deref(),
                None,
            );
            send.write_all(&req_bytes)
                .await
                .map_err(|e| js_err("disconnected", &format!("发送请求失败：{e}")))?;
            let _ = send.finish();

            let response = read_http1_response(recv)
                .await
                .map_err(|e| js_err("transport", &e))?;
            if response.status == 401 {
                return Err(js_err(
                    "unauthorized",
                    "设备授权已失效或被中心吊销，请重新扫码配对",
                ));
            }
            Ok(json!({
                "status": response.status,
                "body": response.body,
            })
            .to_string())
        }

        /// Opens a live SSE `/v1/events` stream over its own multiplexed QUIC stream.
        pub async fn subscribe_events(
            &self,
            since: Option<f64>,
            on_event: js_sys::Function,
            on_error: js_sys::Function,
        ) -> Result<WasmEventSubscription, JsValue> {
            let (mut send, recv) = self.open_tunnel_stream().await?;
            let path = match since {
                Some(seq) if seq > 0.0 => format!("/v1/events?since={}", seq as u64),
                _ => "/v1/events?live=true".to_owned(),
            };
            let req_bytes = build_http1_request(
                "GET",
                &path,
                &self.token,
                &self.client_os,
                None,
                Some("text/event-stream"),
            );
            send.write_all(&req_bytes)
                .await
                .map_err(|e| js_err("disconnected", &format!("订阅事件流失败：{e}")))?;
            let _ = send.finish();

            let (cancel_tx, cancel_rx) = oneshot::channel();
            wasm_bindgen_futures::spawn_local(async move {
                let res = stream_http1_sse(recv, cancel_rx, |payload| {
                    let _ = on_event.call1(&JsValue::NULL, &JsValue::from_str(&payload));
                })
                .await;
                if let Err(err) = res {
                    let code = if err.status == 401 {
                        "unauthorized"
                    } else {
                        "stream_closed"
                    };
                    let err_json = json!({
                        "code": code,
                        "status": err.status,
                        "message": err.message,
                    })
                    .to_string();
                    let _ = on_error.call1(&JsValue::NULL, &JsValue::from_str(&err_json));
                }
            });

            Ok(WasmEventSubscription {
                cancel_tx: RefCell::new(Some(cancel_tx)),
            })
        }

        pub fn is_closed(&self) -> bool {
            self.conn.close_reason().is_some()
        }

        pub async fn close(&self) {
            self.conn.close(0u32.into(), b"client-close");
            self.endpoint.close().await;
        }
    }

    fn js_err(code: &str, message: &str) -> JsValue {
        JsValue::from_str(&json!({ "code": code, "message": message }).to_string())
    }
}

#[cfg(target_arch = "wasm32")]
pub use wasm_api::*;
