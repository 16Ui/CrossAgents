use std::{collections::HashMap, sync::Mutex, time::Duration};

use eventsource_stream::Eventsource;
use futures_util::{
    StreamExt,
    future::{AbortHandle, Abortable},
};
use reqwest::{Client, StatusCode};
use tauri::{AppHandle, Manager, State, ipc::Channel};

use crate::remote::{Remote, client_headers};

#[derive(Default)]
pub struct TerminalRelays(Mutex<HashMap<String, AbortHandle>>);

#[tauri::command]
pub fn terminal_start(
    app: AppHandle,
    client: State<'_, Client>,
    relays: State<'_, TerminalRelays>,
    id: String,
    connection: String,
    frames: Channel<String>,
) -> Result<(), String> {
    if ![&id, &connection].into_iter().all(|value| {
        value.len() == 36
            && value
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
    }) {
        return Err("Invalid terminal or connection ID".into());
    }
    let (abort, registration) = AbortHandle::new_pair();
    if let Some(previous) = relays
        .0
        .lock()
        .expect("terminal relays")
        .insert(id.clone(), abort)
    {
        previous.abort();
    }
    let client = client.inner().clone();
    tauri::async_runtime::spawn(Abortable::new(
        async move {
            loop {
                if matches!(
                    stream(&client, &app.state::<Remote>(), &id, &connection, &frames).await,
                    Ok(true)
                ) || frames.send(r#"{"type":"reconnecting"}"#.into()).is_err()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        },
        registration,
    ));
    Ok(())
}

#[tauri::command]
pub fn terminal_stop(relays: State<'_, TerminalRelays>, id: String) {
    if let Some(abort) = relays.0.lock().expect("terminal relays").remove(&id) {
        abort.abort();
    }
}

async fn stream(
    client: &Client,
    remote: &Remote,
    id: &str,
    connection: &str,
    frames: &Channel<String>,
) -> Result<bool, String> {
    let hub = remote.hub().await?;
    let mut url = hub
        .base
        .join(&format!("/v1/terminals/{id}/output"))
        .map_err(|err| err.to_string())?;
    url.query_pairs_mut().append_pair("connection", connection);
    let request = client
        .get(url)
        .bearer_auth(&hub.token)
        .header("accept", "text/event-stream");
    let request = client_headers(request, &hub);
    let response = tokio::time::timeout(Duration::from_secs(10), request.send())
        .await
        .map_err(|err| err.to_string())?
        .map_err(|err| err.without_url().to_string())?;
    if response.status() == StatusCode::NOT_FOUND {
        let _ = frames.send(r#"{"type":"gone"}"#.into());
        return Ok(true);
    }
    if response.status() == StatusCode::UNAUTHORIZED {
        remote.unauthorized(&hub);
    }
    if !response.status().is_success() {
        return Ok(false);
    }
    let mut events = response.bytes_stream().eventsource();
    while let Some(event) = events.next().await {
        let data = event.map_err(|err| err.to_string())?.data;
        let exited = serde_json::from_str::<serde_json::Value>(&data)
            .is_ok_and(|frame| frame["type"] == "exit");
        if frames.send(data).is_err() || exited {
            return Ok(true);
        }
    }
    Ok(false)
}
