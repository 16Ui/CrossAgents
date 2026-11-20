#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::time::Duration;

use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use reqwest::{Client, Method, Url, redirect::Policy};
use serde::{Deserialize, Serialize};
use tauri::{State, WebviewUrl, WebviewWindowBuilder, ipc::Channel};

struct Hub {
    base: Url,
    token: String,
}

#[derive(Deserialize)]
struct HubFile {
    port: u16,
}

impl Hub {
    async fn discover() -> Result<Self, String> {
        let home = std::env::var_os("CROSSAGENTS_HOME")
            .filter(|value| !value.is_empty())
            .map(std::path::PathBuf::from)
            .or_else(|| dirs::home_dir().map(|home| home.join(".crossagents")))
            .ok_or("Cannot determine the CrossAgents home directory")?;
        let file = tauri::async_runtime::spawn_blocking(move || {
            let hub = std::fs::read(home.join("hub.json"))
                .map_err(|_| "Local Hub is unavailable. Start it with `crossagents hub start`.")?;
            let token = std::fs::read_to_string(home.join("token"))
                .map_err(|_| "Cannot read the local Hub token")?;
            let hub: HubFile =
                serde_json::from_slice(&hub).map_err(|_| "Cannot read the local Hub port")?;
            let base = Url::parse(&format!("http://127.0.0.1:{}", hub.port))
                .map_err(|_| "Invalid local Hub port")?;
            if token.trim().is_empty() {
                return Err("The local Hub token is empty");
            }
            Ok(Self {
                base,
                token: token.trim().into(),
            })
        })
        .await
        .map_err(|_| "Cannot read the local Hub configuration")?;
        file.map_err(str::to_owned)
    }

    fn endpoint(&self, method: &Method, path: &str) -> Result<Url, String> {
        let url = self.base.join(path).map_err(|_| "Invalid Hub API path")?;
        if !path.starts_with("/v1/") || url.origin() != self.base.origin() {
            return Err("Only local Hub API routes are available".into());
        }
        let segments: Vec<_> = url.path().split('/').collect();
        let allowed = match *method {
            Method::GET => url.path().starts_with("/v1/") && url.path() != "/v1/events",
            Method::POST => {
                matches!(segments.as_slice(), ["", "v1", "agents", agent, "messages"] if !agent.is_empty())
            }
            _ => false,
        };
        if !allowed {
            return Err("This operation is not available in the Tauri prototype".into());
        }
        Ok(url)
    }
}

#[derive(Serialize)]
struct HubResponse {
    status: u16,
    body: String,
}

#[tauri::command]
async fn hub_request(
    client: State<'_, Client>,
    method: String,
    path: String,
    body: Option<String>,
) -> Result<HubResponse, String> {
    let method = Method::from_bytes(method.as_bytes()).map_err(|_| "Invalid HTTP method")?;
    let hub = Hub::discover().await?;
    let url = hub.endpoint(&method, &path)?;
    let mut request = client
        .request(method, url)
        .bearer_auth(&hub.token)
        .timeout(Duration::from_secs(30));
    if let Some(body) = body {
        request = request
            .header("content-type", "application/json")
            .body(body);
    }
    let response = request
        .send()
        .await
        .map_err(|err| err.without_url().to_string())?;
    let status = response.status().as_u16();
    let body = response
        .text()
        .await
        .map_err(|err| err.without_url().to_string())?;
    Ok(HubResponse { status, body })
}

#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum StreamFrame {
    Connected,
    Event { data: String, id: String },
}

#[tauri::command]
async fn hub_events(
    client: State<'_, Client>,
    last_id: Option<String>,
    frames: Channel<StreamFrame>,
) -> Result<(), String> {
    let hub = Hub::discover().await?;
    let mut url = hub.base.join("/v1/events").map_err(|err| err.to_string())?;
    if last_id.is_none() {
        url.set_query(Some("live=true"));
    }
    let mut request = client
        .get(url)
        .bearer_auth(&hub.token)
        .header("accept", "text/event-stream");
    if let Some(id) = last_id {
        request = request.header("last-event-id", id);
    }
    // Bound the wait for headers while letting the SSE response body stay open.
    let response = tokio::time::timeout(Duration::from_secs(10), request.send())
        .await
        .map_err(|_| "Timed out opening the Hub event stream")?
        .and_then(reqwest::Response::error_for_status)
        .map_err(|err| err.without_url().to_string())?;
    frames
        .send(StreamFrame::Connected)
        .map_err(|err| err.to_string())?;
    let mut events = response.bytes_stream().eventsource();
    while let Some(event) = events.next().await {
        let event = event.map_err(|err| err.to_string())?;
        if frames
            .send(StreamFrame::Event {
                data: event.data,
                id: event.id,
            })
            .is_err()
        {
            break;
        }
    }
    Ok(())
}

fn main() {
    let client = Client::builder()
        .no_proxy()
        .redirect(Policy::none())
        .connect_timeout(Duration::from_secs(3))
        .build()
        .expect("build local Hub client");
    tauri::Builder::default()
        .manage(client)
        .invoke_handler(tauri::generate_handler![hub_request, hub_events])
        .setup(|app| {
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("CrossAgents · Tauri 预览")
                .inner_size(1280.0, 900.0)
                .on_navigation(|url| {
                    url.port().is_none()
                        && url.username().is_empty()
                        && url.password().is_none()
                        && matches!(
                            (url.scheme(), url.host_str()),
                            ("http", Some("tauri.localhost")) | ("tauri", Some("localhost"))
                        )
                })
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("run CrossAgents Tauri prototype");
}
