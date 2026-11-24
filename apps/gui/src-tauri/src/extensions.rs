use std::{fs, path::PathBuf, sync::LazyLock, time::Duration};

use percent_encoding::percent_decode_str;
use regex::Regex;
use reqwest::Client;
use serde::Serialize;
use tauri::{
    AppHandle, Manager, State,
    http::{Method, Request, Response, StatusCode},
};

use crate::remote::{Remote, client_headers};

pub struct Extensions {
    marker: PathBuf,
    reason: Option<&'static str>,
}

impl Extensions {
    pub fn new(app: &AppHandle) -> tauri::Result<Self> {
        let marker = app.path().app_data_dir()?.join("extension-loading");
        let reason = if std::env::args_os().any(|arg| arg == "--safe-mode") {
            Some("通过 --safe-mode 启动，已跳过所有扩展。")
        } else if marker.exists() {
            Some("上次启动在加载扩展时未能就绪，已自动进入安全模式并跳过所有扩展。")
        } else {
            None
        };
        Ok(Self { marker, reason })
    }
}

#[derive(Serialize)]
pub struct SafeMode {
    enabled: bool,
    reason: Option<&'static str>,
}

#[tauri::command]
pub fn extension_safe_mode(state: State<'_, Extensions>) -> SafeMode {
    SafeMode {
        enabled: state.reason.is_some(),
        reason: state.reason,
    }
}

#[tauri::command]
pub fn extension_loading(state: State<'_, Extensions>) -> Result<(), String> {
    if state.reason.is_none() {
        if let Some(parent) = state.marker.parent() {
            fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        fs::write(&state.marker, "loading\n").map_err(|err| err.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn extension_ready(state: State<'_, Extensions>) -> Result<(), String> {
    match fs::remove_file(&state.marker) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.to_string()),
    }
}

fn asset_parts(raw: &str) -> Option<Vec<String>> {
    static NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new("^[a-z][a-z0-9-]*$").unwrap());
    static FINGERPRINT: LazyLock<Regex> = LazyLock::new(|| Regex::new("^[A-Za-z0-9_-]+$").unwrap());
    let mut parts = raw.strip_prefix('/')?.split('/');
    let name = parts.next().filter(|part| NAME.is_match(part))?;
    let fingerprint = parts.next().filter(|part| FINGERPRINT.is_match(part))?;
    let mut result = vec![
        "v1".to_owned(),
        "extensions".to_owned(),
        name.to_owned(),
        "assets".to_owned(),
        fingerprint.to_owned(),
    ];
    for part in parts {
        let decoded = percent_decode_str(part).decode_utf8().ok()?;
        if matches!(decoded.as_ref(), "" | "." | "..") || decoded.contains(['/', '\\', ':', '\0']) {
            return None;
        }
        result.push(decoded.into_owned());
    }
    (result.len() > 5).then_some(result)
}

pub async fn serve(app: AppHandle, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
    let reply = Response::builder()
        .header("access-control-allow-origin", "*")
        .header("access-control-allow-methods", "GET, HEAD, OPTIONS")
        .header("x-content-type-options", "nosniff");
    let (status, content_type, cache, body) =
        read(&app, request)
            .await
            .unwrap_or((StatusCode::NOT_FOUND, None, None, Vec::new()));
    let mut reply = reply.status(status);
    if let Some(value) = content_type {
        reply = reply.header("content-type", value);
    }
    if let Some(value) = cache {
        reply = reply.header("cache-control", value);
    }
    reply.body(body).expect("valid extension asset response")
}

type Asset = (
    StatusCode,
    Option<tauri::http::HeaderValue>,
    Option<tauri::http::HeaderValue>,
    Vec<u8>,
);

async fn read(app: &AppHandle, request: Request<Vec<u8>>) -> Option<Asset> {
    let empty = |status| Some((status, None, None, Vec::new()));
    if request.method() == Method::OPTIONS {
        return empty(StatusCode::NO_CONTENT);
    }
    if !matches!(*request.method(), Method::GET | Method::HEAD) {
        return empty(StatusCode::METHOD_NOT_ALLOWED);
    }
    if app.state::<Extensions>().reason.is_some() {
        return empty(StatusCode::FORBIDDEN);
    }
    let parts = asset_parts(request.uri().path())?;
    let remote = app.state::<Remote>();
    let hub = remote.hub().await.ok()?;
    let mut url = hub.base.clone();
    url.path_segments_mut().ok()?.clear().extend(parts);
    let head = request.method() == Method::HEAD;
    let request = app
        .state::<Client>()
        .request(request.method().clone(), url)
        .bearer_auth(&hub.token)
        .timeout(Duration::from_secs(30));
    let response = client_headers(request, &hub).send().await.ok()?;
    let status = response.status();
    if status == StatusCode::UNAUTHORIZED {
        remote.unauthorized(&hub);
    }
    let content_type = response.headers().get("content-type").cloned();
    let cache = response.headers().get("cache-control").cloned();
    let body = if head {
        Vec::new()
    } else {
        response.bytes().await.ok()?.to_vec()
    };
    Some((status, content_type, cache, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_asset_paths_remain_inside_the_named_bundle() {
        let parts = asset_parts("/demo/hash/gui/hello%20world.js").unwrap();
        assert_eq!(
            parts.join("/"),
            "v1/extensions/demo/assets/hash/gui/hello world.js"
        );
        for path in [
            "/demo/hash/%2e%2e/file",
            "/demo/hash/a%2fb",
            "/demo/hash/a%5cb",
            "/demo/hash/C%3A/a",
            "/demo/hash/",
        ] {
            assert!(asset_parts(path).is_none(), "{path}");
        }
    }
}
