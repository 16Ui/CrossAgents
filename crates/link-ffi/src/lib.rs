//! C-ABI bridge embedding [`crossagents_link`] directly into iOS / mobile apps so they can redeem
//! one-time `ca1-...` pairing codes and run an in-process loopback tunnel (`127.0.0.1:<port>`)
//! over iroh QUIC without spawning subprocesses.
#![allow(unsafe_code)]

use std::ffi::{CStr, CString, c_char, c_void};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crossagents_link::{Event, Invite, Target};
use serde::Deserialize;
use serde_json::json;
use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;

pub type CaEventCallback = extern "C" fn(event_json: *const c_char, user_data: *mut c_void);

type CallbackSink = Arc<Mutex<Option<(CaEventCallback, SendPtr)>>>;

pub struct CaTunnelHandle {
    cancel: CancellationToken,
    sink: CallbackSink,
}

#[derive(Clone, Copy)]
struct SendPtr(*mut c_void);
unsafe impl Send for SendPtr {}
unsafe impl Sync for SendPtr {}

fn runtime() -> Result<&'static Runtime, String> {
    static RT: OnceLock<Result<Runtime, String>> = OnceLock::new();
    match RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("crossagents-link")
            .build()
            .map_err(|e| format!("cannot start async runtime: {e}"))
    }) {
        Ok(rt) => Ok(rt),
        Err(err) => Err(err.clone()),
    }
}

fn cstr_to_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .ok()
        .map(str::to_owned)
}

fn into_c_json(value: serde_json::Value) -> *mut c_char {
    let text = value.to_string().replace('\0', "");
    CString::new(text)
        .unwrap_or_else(|_| CString::new("{\"ok\":false,\"error\":\"encoding error\"}").unwrap())
        .into_raw()
}

fn emit_event(callback: CaEventCallback, user_data: SendPtr, value: serde_json::Value) {
    let text = value.to_string().replace('\0', "");
    if let Ok(cstr) = CString::new(text) {
        callback(cstr.as_ptr(), user_data.0);
    }
}

fn emit_to_sink(sink: &CallbackSink, value: serde_json::Value) {
    let text = value.to_string().replace('\0', "");
    let Ok(cstr) = CString::new(text) else {
        return;
    };
    let guard = sink.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((callback, ud)) = *guard {
        callback(cstr.as_ptr(), ud.0);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ca_link_decode_invite(code: *const c_char) -> *mut c_char {
    let Some(raw) = cstr_to_string(code) else {
        return into_c_json(json!({ "ok": false, "error": "配对码为空" }));
    };
    match Invite::decode(&raw) {
        Ok(invite) => into_c_json(json!({
            "ok": true,
            "host": invite.host,
            "expires": invite.expires,
            "has_relay": invite.relay.is_some(),
            "custom_relay": invite.custom_relay,
            "lan_addrs": invite.addrs.len(),
        })),
        Err(err) => into_c_json(json!({ "ok": false, "error": format!("{err:#}") })),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ca_link_redeem(
    code: *const c_char,
    device_name: *const c_char,
    os: *const c_char,
    proxy: *const c_char,
) -> *mut c_char {
    let Some(raw) = cstr_to_string(code) else {
        return into_c_json(json!({ "ok": false, "error": "配对码为空" }));
    };
    let name = cstr_to_string(device_name)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "iPhone".into());
    let os = cstr_to_string(os)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "ios".into());
    let proxy = cstr_to_string(proxy)
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());

    let invite = match Invite::decode(&raw) {
        Ok(inv) => inv,
        Err(err) => return into_c_json(json!({ "ok": false, "error": format!("{err:#}") })),
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    if invite.expires <= now {
        return into_c_json(json!({
            "ok": false,
            "error": "这个配对码已经过期，请在中心电脑重新生成一个"
        }));
    }
    let rt = match runtime() {
        Ok(rt) => rt,
        Err(err) => return into_c_json(json!({ "ok": false, "error": err })),
    };
    let result = rt.block_on(async {
        let mut paired = crossagents_link::redeem(&invite, &name, Some(&os), proxy)
            .await
            .map_err(|err| format!("{err:#}"))?;
        paired["host"] = invite.host.clone().into();
        paired["endpoint"] = invite.endpoint.clone().into();
        if let Some(relay) = &invite.relay {
            paired["relay"] = relay.clone().into();
        }
        paired["custom_relay"] = invite.custom_relay.into();
        paired["addrs"] = invite.addrs.clone().into();
        Ok::<_, String>(paired)
    });
    match result {
        Ok(paired) => into_c_json(json!({ "ok": true, "paired": paired })),
        Err(err) => into_c_json(json!({ "ok": false, "error": err })),
    }
}

#[derive(Deserialize)]
struct TunnelConfig {
    endpoint: String,
    #[serde(default)]
    relay: Option<String>,
    #[serde(default)]
    custom_relay: bool,
    #[serde(default)]
    addrs: Vec<String>,
    token: String,
    #[serde(default)]
    proxy: Option<String>,
}

#[unsafe(no_mangle)]
pub extern "C" fn ca_link_start(
    config_json: *const c_char,
    callback: CaEventCallback,
    user_data: *mut c_void,
) -> *mut CaTunnelHandle {
    let ud = SendPtr(user_data);
    let Some(raw) = cstr_to_string(config_json) else {
        emit_event(
            callback,
            ud,
            json!({ "event": "unreachable", "error": "missing tunnel config" }),
        );
        return std::ptr::null_mut();
    };
    let cfg: TunnelConfig = match serde_json::from_str(&raw) {
        Ok(c) => c,
        Err(err) => {
            emit_event(
                callback,
                ud,
                json!({ "event": "unreachable", "error": format!("invalid tunnel config: {err}") }),
            );
            return std::ptr::null_mut();
        }
    };
    let mut target = match Target::new(&cfg.endpoint, cfg.relay.as_deref()) {
        Ok(t) => t,
        Err(err) => {
            emit_event(
                callback,
                ud,
                json!({ "event": "unreachable", "error": format!("{err:#}") }),
            );
            return std::ptr::null_mut();
        }
    };
    target.custom_relay = cfg.custom_relay && target.relay.is_some();
    target.addrs = cfg.addrs.iter().filter_map(|a| a.parse().ok()).collect();
    let proxy = cfg.proxy.filter(|p| !p.trim().is_empty());

    let rt = match runtime() {
        Ok(rt) => rt,
        Err(err) => {
            emit_event(
                callback,
                ud,
                json!({ "event": "unreachable", "error": err }),
            );
            return std::ptr::null_mut();
        }
    };

    let cancel = CancellationToken::new();
    let task_cancel = cancel.clone();
    let sink: CallbackSink = Arc::new(Mutex::new(Some((callback, ud))));
    let task_sink = Arc::clone(&sink);
    let listen: SocketAddr = "127.0.0.1:0".parse().expect("valid loopback addr");

    rt.spawn(async move {
        let cb_sink = Arc::clone(&task_sink);
        let res = crossagents_link::forward_until(
            target,
            cfg.token,
            listen,
            proxy,
            task_cancel.clone(),
            move |event: Event| {
                if let Ok(val) = serde_json::to_value(&event) {
                    emit_to_sink(&cb_sink, val);
                }
            },
        )
        .await;
        if let Err(err) = res
            && !task_cancel.is_cancelled()
        {
            emit_to_sink(
                &task_sink,
                json!({ "event": "unreachable", "error": format!("{err:#}") }),
            );
        }
    });

    Box::into_raw(Box::new(CaTunnelHandle { cancel, sink }))
}

#[unsafe(no_mangle)]
pub extern "C" fn ca_link_stop(handle: *mut CaTunnelHandle) {
    if handle.is_null() {
        return;
    }
    let handle = unsafe { Box::from_raw(handle) };
    // Clear the callback sink under the mutex before returning so no background event can ever
    // dereference `user_data` after `ca_link_stop` completes.
    if let Ok(mut guard) = handle.sink.lock() {
        *guard = None;
    } else if let Err(poisoned) = handle.sink.lock() {
        *poisoned.into_inner() = None;
    }
    handle.cancel.cancel();
}

#[unsafe(no_mangle)]
pub extern "C" fn ca_link_string_free(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        let _ = CString::from_raw(ptr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn decode_invite_handles_valid_invalid_and_null() {
        let null_ptr = ca_link_decode_invite(std::ptr::null());
        let null_str = unsafe { CStr::from_ptr(null_ptr) }.to_str().unwrap().to_owned();
        ca_link_string_free(null_ptr);
        let v: serde_json::Value = serde_json::from_str(&null_str).unwrap();
        assert_eq!(v["ok"], false);

        let bad = CString::new("not-a-code").unwrap();
        let bad_ptr = ca_link_decode_invite(bad.as_ptr());
        let bad_str = unsafe { CStr::from_ptr(bad_ptr) }.to_str().unwrap().to_owned();
        ca_link_string_free(bad_ptr);
        let v: serde_json::Value = serde_json::from_str(&bad_str).unwrap();
        assert_eq!(v["ok"], false);

        // Construct via Invite directly with a valid 32-byte public key hex string.
        let endpoint_id = "6f1758b74080c9b676b8987e6aa84e4dff382096d2c2dd375c670d75e2d12d5a";
        let code = Invite {
            endpoint: endpoint_id.into(),
            relay: Some("https://usw1-1.relay.n0.iroh.link./".into()),
            custom_relay: false,
            addrs: vec!["192.168.1.10:5555".into()],
            secret: "secret123".into(),
            host: "DESKTOP-TEST".into(),
            expires: 4_000_000_000,
        }
        .encode();
        let c_code = CString::new(code).unwrap();
        let ok_ptr = ca_link_decode_invite(c_code.as_ptr());
        let ok_str = unsafe { CStr::from_ptr(ok_ptr) }.to_str().unwrap().to_owned();
        ca_link_string_free(ok_ptr);
        let v: serde_json::Value = serde_json::from_str(&ok_str).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["host"], "DESKTOP-TEST");
        assert_eq!(v["has_relay"], true);
        assert_eq!(v["lan_addrs"], 1);
    }

    extern "C" fn capture_cb(event_json: *const c_char, user_data: *mut c_void) {
        let s = unsafe { CStr::from_ptr(event_json) }
            .to_str()
            .unwrap()
            .to_owned();
        let log = unsafe { &*(user_data as *const Mutex<Vec<String>>) };
        log.lock().unwrap().push(s);
    }

    #[test]
    fn tunnel_starts_listens_and_stops_cleanly() {
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let cfg = json!({
            "endpoint": "6f1758b74080c9b676b8987e6aa84e4dff382096d2c2dd375c670d75e2d12d5a",
            "relay": null,
            "custom_relay": false,
            "addrs": [],
            "token": "cad_testtoken"
        })
        .to_string();
        let c_cfg = CString::new(cfg).unwrap();
        let raw_ptr = Arc::as_ptr(&events) as *mut c_void;
        let handle = ca_link_start(c_cfg.as_ptr(), capture_cb, raw_ptr);
        assert!(!handle.is_null());

        let mut listening = false;
        for _ in 0..40 {
            if events
                .lock()
                .unwrap()
                .iter()
                .any(|e| e.contains("\"listening\""))
            {
                listening = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(listening, "expected listening event from ca_link_start");
        ca_link_stop(handle);
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
