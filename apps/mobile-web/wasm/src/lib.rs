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

    use futures_util::stream::{FuturesUnordered, StreamExt};
    use iroh::endpoint::{Connection, QuicTransportConfig, RecvStream, SendStream, presets};
    use iroh::{Endpoint, EndpointId, RelayMode, RelayUrl, Watcher as _};
    use n0_future::time::{self, Duration};
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::sync::{oneshot, watch};
    use wasm_bindgen::prelude::*;

    use crate::http1::{build_http1_request, read_http1_response, stream_http1_sse};
    use crate::protocol::{
        ALPN, Hello, Invite, Reply, TunnelConfig, candidate_relays, endpoint_addr_for,
    };

    const MAX_HELLO_LINE: usize = 16 * 1024;
    /// How long to wait for `endpoint.online()` (HTTPS `/ping` + WebSocket upgrade) before
    /// starting the QUIC handshake. Slightly above iroh's 5s `OVERALL_REPORT_TIMEOUT` so a normal
    /// relay WebSocket finishes BEFORE the QUIC handshake timer begins.
    const ONLINE_WAIT_PRIMARY: Duration = Duration::from_millis(5_500);
    const ONLINE_WAIT_FALLBACK: Duration = Duration::from_millis(4_500);
    /// Happy-Eyeballs head start given to the last-known relay before racing the top backup
    /// n0 relays in parallel. High enough (4.5s) that a healthy primary relay (~3.5-4s) finishes
    /// cleanly with only 1 Endpoint in iOS Safari, yet fast enough to catch a hub relay switch
    /// (`usw1-1 <-> euc1-1`) without waiting out the full 18s serial timeout.
    const PRIMARY_HEAD_START: Duration = Duration::from_millis(4_500);
    /// Maximum backup relays launched in the initial Happy-Eyeballs fan-out (caps peak concurrent
    /// `iroh::Endpoint` instances at 3 in single-threaded mobile Safari WASM).
    const FALLBACK_WAVE_SIZE: usize = 2;
    /// Maximum time allowed for the QUIC handshake on a single relay (after `online()`).
    /// Bounds stale-relay detection so fallback across n0 relays completes promptly instead of
    /// waiting out QUIC's full 30s idle timeout.
    const QUIC_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(12);
    /// Browser-over-relay QUIC initial RTT (prevents premature PTO bursts across WAN relays).
    const BROWSER_INITIAL_RTT: Duration = Duration::from_millis(450);
    /// Periodic QUIC ping keeps the relay WebSocket + QUIC path warm while the user reads messages.
    const QUIC_KEEP_ALIVE: Duration = Duration::from_secs(15);
    /// Receive buffer size for pipelined tunnel HTTP/SSE reads (64 KiB reduces read syscalls on
    /// larger `/v1/agents` and `/v1/agents/{id}/transcript` JSON payloads).
    const TUNNEL_READ_BUF: usize = 64 * 1024;

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
                "relay": inv.sanitized_relay_string(),
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
            Err(err) => json!({ "ok": false, "error": err.message, "code": err.code }).to_string(),
        }
    }

    struct DialFailure {
        code: &'static str,
        message: String,
    }

    struct SingleAttemptErr {
        relay: RelayUrl,
        relay_connected: bool,
        relay_error: Option<String>,
        connect_error: String,
        elapsed_ms: u64,
    }

    impl SingleAttemptErr {
        fn short_label(&self) -> String {
            let host = self.relay.host_str().unwrap_or(self.relay.as_str());
            let ws_state = if self.relay_connected {
                "中继已连通".to_owned()
            } else if let Some(err) = &self.relay_error {
                format!("中继未连上({err})")
            } else {
                "中继未就绪".to_owned()
            };
            format!(
                "{host} [{ws_state}, {}ms]: {}",
                self.elapsed_ms, self.connect_error
            )
        }
    }

    async fn bind_single_relay_endpoint(relay: RelayUrl) -> Result<Endpoint, String> {
        let transport = QuicTransportConfig::builder()
            .initial_rtt(BROWSER_INITIAL_RTT)
            .keep_alive_interval(QUIC_KEEP_ALIVE)
            .build();
        // Use `presets::Minimal` instead of `presets::N0`:
        // 1. Skips `PkarrPublisher`/`PkarrResolver` calls to `https://dns.iroh.link/pkarr`,
        //    which fail with TLS EOF on restricted networks and add latency.
        // 2. Pins `RelayMode::custom([relay])` to a single trailing-dot-free relay so Safari
        //    only opens 1 WebSocket and never wastes time probing unreachable relays.
        Endpoint::builder(presets::Minimal)
            .relay_mode(RelayMode::custom([relay]))
            .transport_config(transport)
            .bind()
            .await
            .map_err(|e| format!("初始化浏览器 iroh 节点失败：{e}"))
    }

    fn inspect_relay_status(endpoint: &Endpoint) -> (bool, Option<String>) {
        let statuses = endpoint.home_relay_status().get();
        let connected = statuses.iter().any(|s| s.is_connected());
        let last_err = statuses
            .iter()
            .find_map(|s| s.last_error().map(|e| format!("{e:#}")));
        (connected, last_err)
    }

    async fn dial_one_relay(
        endpoint_id: EndpointId,
        relay: RelayUrl,
        online_wait: Duration,
        mut cancel_rx: Option<watch::Receiver<bool>>,
    ) -> Result<(Endpoint, Connection, RelayUrl), SingleAttemptErr> {
        let started_ms = js_sys::Date::now();
        let endpoint = match bind_single_relay_endpoint(relay.clone()).await {
            Ok(ep) => ep,
            Err(err) => {
                return Err(SingleAttemptErr {
                    relay,
                    relay_connected: false,
                    relay_error: Some(err.clone()),
                    connect_error: err,
                    elapsed_ms: (js_sys::Date::now() - started_ms).max(0.0) as u64,
                });
            }
        };

        // Wait for the home relay WebSocket to finish connecting before starting QUIC `connect`,
        // so slow TLS / WebSocket handshakes don't eat into `noq`'s 10s QUIC handshake budget.
        // If `net_report`'s 3s `/ping` probe times out, we still proceed to `endpoint.connect`,
        // because `ActiveRelayActor` dials the relay WebSocket directly on outgoing datagrams.
        let cancelled_before_connect = if let Some(rx) = cancel_rx.as_mut() {
            tokio::select! {
                biased;
                _ = rx.changed() => *rx.borrow(),
                _ = time::timeout(online_wait, endpoint.online()) => false,
            }
        } else {
            let _ = time::timeout(online_wait, endpoint.online()).await;
            false
        };

        if cancelled_before_connect {
            endpoint.close().await;
            return Err(SingleAttemptErr {
                relay,
                relay_connected: false,
                relay_error: None,
                connect_error: "cancelled".into(),
                elapsed_ms: (js_sys::Date::now() - started_ms).max(0.0) as u64,
            });
        }

        let addr = endpoint_addr_for(endpoint_id, &relay);
        let connect_fut = async {
            match time::timeout(QUIC_HANDSHAKE_TIMEOUT, endpoint.connect(addr, ALPN)).await {
                Ok(Ok(conn)) => Ok(conn),
                Ok(Err(e)) => Err(format!("{e}")),
                Err(_) => Err(format!("QUIC 握手超时（{}s）", QUIC_HANDSHAKE_TIMEOUT.as_secs())),
            }
        };
        let connect_res = if let Some(rx) = cancel_rx.as_mut() {
            tokio::select! {
                biased;
                _ = rx.changed() => Err("cancelled".to_owned()),
                res = connect_fut => res,
            }
        } else {
            connect_fut.await
        };

        match connect_res {
            Ok(conn) => Ok((endpoint, conn, relay)),
            Err(connect_error) => {
                let (relay_connected, relay_error) = inspect_relay_status(&endpoint);
                wasm_bindgen_futures::spawn_local(async move {
                    let _ = time::timeout(Duration::from_secs(3), endpoint.close()).await;
                });
                Err(SingleAttemptErr {
                    relay,
                    relay_connected,
                    relay_error,
                    connect_error,
                    elapsed_ms: (js_sys::Date::now() - started_ms).max(0.0) as u64,
                })
            }
        }
    }

    async fn dial_hub(
        endpoint_id: EndpointId,
        preferred_relay: Option<RelayUrl>,
        custom_relay: bool,
    ) -> Result<(Endpoint, Connection, RelayUrl), DialFailure> {
        let candidates = candidate_relays(preferred_relay, custom_relay).map_err(|message| {
            DialFailure {
                code: "bad_config",
                message,
            }
        })?;

        let Some((first, rest)) = candidates.split_first() else {
            return Err(DialFailure {
                code: "bad_config",
                message: "没有可用的中继地址".into(),
            });
        };

        // Happy Eyeballs across n0 relays: start the hub's last-known relay immediately, and if
        // it hasn't connected within `PRIMARY_HEAD_START` (e.g. hub switched `usw1-1 <-> euc1-1`
        // while the phone screen was locked), start racing the remaining relays in parallel while
        // keeping the primary attempt alive. First relay to finish wins and cancels all losers.
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let mut race = FuturesUnordered::new();
        race.push(dial_one_relay(
            endpoint_id,
            first.clone(),
            ONLINE_WAIT_PRIMARY,
            Some(cancel_rx.clone()),
        ));

        let mut launched_wave1 = custom_relay || rest.is_empty();
        let mut launched_tail = custom_relay || rest.len() <= FALLBACK_WAVE_SIZE;
        let (wave1, tail) = rest.split_at(rest.len().min(FALLBACK_WAVE_SIZE));
        let mut failures = Vec::new();

        while !race.is_empty() {
            let next_res = if !launched_wave1 {
                tokio::select! {
                    biased;
                    res = race.next() => Some(res),
                    _ = time::sleep(PRIMARY_HEAD_START) => None,
                }
            } else {
                Some(race.next().await)
            };

            match next_res {
                None => {
                    // Primary relay is taking longer than expected: fan out to top backup relays.
                    launched_wave1 = true;
                    for relay in wave1 {
                        race.push(dial_one_relay(
                            endpoint_id,
                            relay.clone(),
                            ONLINE_WAIT_FALLBACK,
                            Some(cancel_rx.clone()),
                        ));
                    }
                }
                Some(Some(Ok(connected))) => {
                    let _ = cancel_tx.send(true);
                    // Drain & close losing relay endpoints asynchronously so the user isn't
                    // delayed waiting for unreachable relays to finish shutting down.
                    wasm_bindgen_futures::spawn_local(async move {
                        while let Some(other) = race.next().await {
                            if let Ok((ep, conn, _)) = other {
                                conn.close(0u32.into(), b"fallback-loser");
                                let _ = time::timeout(Duration::from_secs(3), ep.close()).await;
                            }
                        }
                    });
                    return Ok(connected);
                }
                Some(Some(Err(err))) => {
                    if err.connect_error != "cancelled" {
                        failures.push(err);
                    }
                    if !launched_wave1 {
                        launched_wave1 = true;
                        for relay in wave1 {
                            race.push(dial_one_relay(
                                endpoint_id,
                                relay.clone(),
                                ONLINE_WAIT_FALLBACK,
                                Some(cancel_rx.clone()),
                            ));
                        }
                    } else if race.is_empty() && !launched_tail {
                        launched_tail = true;
                        for relay in tail {
                            race.push(dial_one_relay(
                                endpoint_id,
                                relay.clone(),
                                ONLINE_WAIT_FALLBACK,
                                Some(cancel_rx.clone()),
                            ));
                        }
                    }
                }
                Some(None) => break,
            }
        }

        let any_relay_connected = failures.iter().any(|f| f.relay_connected);
        let details = failures
            .iter()
            .map(SingleAttemptErr::short_label)
            .collect::<Vec<_>>()
            .join("；");

        if !any_relay_connected {
            Err(DialFailure {
                code: "relay_unreachable",
                message: format!(
                    "手机无法连上加密中继 WebSocket（可点下方「测试中继连通性」排查网络/iCloud 专用代理）：{details}"
                ),
            })
        } else {
            Err(DialFailure {
                code: "hub_offline",
                message: format!(
                    "已连上中继但未收到中心电脑响应（请确认中心在线、内置打洞为「在线」，或中心刚切换了中继）：{details}"
                ),
            })
        }
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
    ) -> Result<serde_json::Value, DialFailure> {
        let invite = Invite::decode(code).map_err(|message| DialFailure {
            code: "bad_config",
            message,
        })?;
        let now_secs = (js_sys::Date::now() / 1000.0) as i64;
        if invite.expires <= now_secs {
            return Err(DialFailure {
                code: "bad_config",
                message: "这个配对码已经过期，请在中心电脑重新生成一个".into(),
            });
        }
        let (endpoint_id, preferred_relay, custom_relay) =
            invite.parsed_target().map_err(|message| DialFailure {
                code: "bad_config",
                message,
            })?;
        let name = device_name.trim();
        let name = if name.is_empty() { "iPhone" } else { name };
        let os = os.trim();
        let os = if os.is_empty() { "ios" } else { os };

        let (endpoint, conn, active_relay) =
            dial_hub(endpoint_id, preferred_relay, custom_relay).await?;

        let outcome = async {
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
            paired["relay"] = active_relay.to_string().into();
            paired["custom_relay"] = invite.custom_relay.into();
            paired["addrs"] = invite.addrs.clone().into();
            Ok(paired)
        }
        .await;

        wasm_bindgen_futures::spawn_local(async move {
            let _ = time::timeout(Duration::from_secs(3), endpoint.close()).await;
        });
        outcome.map_err(|message| DialFailure {
            code: "transport",
            message,
        })
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
        active_relay: String,
        token: String,
        client_os: String,
    }

    #[wasm_bindgen]
    impl WasmLinkSession {
        /// Opens a persistent end-to-end encrypted iroh connection to the hub.
        pub async fn connect(config_json: &str) -> Result<WasmLinkSession, JsValue> {
            let cfg: TunnelConfig = serde_json::from_str(config_json)
                .map_err(|e| js_err("bad_config", &format!("无效的连接配置：{e}")))?;
            let (endpoint_id, preferred_relay, custom_relay) = cfg
                .parsed_target()
                .map_err(|e| js_err("bad_config", &e))?;
            let (endpoint, conn, active_relay) =
                dial_hub(endpoint_id, preferred_relay, custom_relay)
                    .await
                    .map_err(|err| js_err(err.code, &err.message))?;
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
                active_relay: active_relay.to_string(),
                token: cfg.token,
                client_os,
            })
        }

        pub fn connected_relay(&self) -> String {
            self.active_relay.clone()
        }

        /// Opens a multiplexed QUIC bi-stream and pipelines `Hello::Tunnel\n` + the HTTP/1.1
        /// request in a SINGLE flight — cutting 1 full WAN relay RTT (400–800ms) off every call.
        /// Hub `crates/link` wraps `recv` in `BufReader`, consumes only the JSON hello line, and
        /// forwards the remaining buffered HTTP bytes immediately into the local hub TCP socket.
        async fn open_pipelined_tunnel(
            &self,
            http_req: &[u8],
        ) -> Result<(SendStream, BufReader<RecvStream>), JsValue> {
            let (mut send, recv) = self
                .conn
                .open_bi()
                .await
                .map_err(|e| js_err("disconnected", &format!("隧道连接已断开：{e}")))?;
            let mut hello = serde_json::to_vec(&Hello::Tunnel {
                token: self.token.clone(),
            })
            .map_err(|e| js_err("disconnected", &format!("序列化握手消息失败：{e}")))?;
            hello.push(b'\n');
            hello.extend_from_slice(http_req);
            send.write_all(&hello)
                .await
                .map_err(|e| js_err("disconnected", &format!("发送隧道请求失败：{e}")))?;
            let _ = send.flush().await;

            let mut recv = BufReader::with_capacity(TUNNEL_READ_BUF, recv);
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

        /// Executes one HTTP/1.1 REST API request through the encrypted iroh tunnel in 1 RTT.
        /// Resolves to JSON `{"status":200,"body":"..."}`.
        pub async fn request(
            &self,
            method: &str,
            path: &str,
            body: Option<String>,
        ) -> Result<String, JsValue> {
            let req_bytes = build_http1_request(
                method,
                path,
                &self.token,
                &self.client_os,
                body.as_deref(),
                None,
            );
            let (mut send, mut recv) = self.open_pipelined_tunnel(&req_bytes).await?;

            // Keep `send` open until the hub finishes writing the HTTP response: finishing the
            // QUIC send stream early causes the hub's `copy_bidirectional` to half-close its
            // loopback TCP socket, which hyper/axum treats as a client disconnect.
            let response = read_http1_response(&mut recv)
                .await
                .map_err(|e| js_err("transport", &e))?;
            let _ = send.finish();
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

        /// Opens a live SSE `/v1/events` stream over its own multiplexed QUIC stream in 1 RTT.
        pub async fn subscribe_events(
            &self,
            since: Option<f64>,
            on_event: js_sys::Function,
            on_error: js_sys::Function,
        ) -> Result<WasmEventSubscription, JsValue> {
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
            let (send, recv) = self.open_pipelined_tunnel(&req_bytes).await?;

            let (cancel_tx, cancel_rx) = oneshot::channel();
            wasm_bindgen_futures::spawn_local(async move {
                // Keep `send` alive for the lifetime of the SSE stream so the hub's loopback
                // hyper connection isn't half-closed.
                let mut keep_send = send;
                let res = stream_http1_sse(recv, cancel_rx, |payload| {
                    let _ = on_event.call1(&JsValue::NULL, &JsValue::from_str(&payload));
                })
                .await;
                let _ = keep_send.finish();
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
            let _ = time::timeout(Duration::from_secs(2), self.endpoint.close()).await;
        }
    }

    fn js_err(code: &str, message: &str) -> JsValue {
        JsValue::from_str(&json!({ "code": code, "message": message }).to_string())
    }
}

#[cfg(target_arch = "wasm32")]
pub use wasm_api::*;
