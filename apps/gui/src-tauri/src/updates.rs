use std::sync::Mutex;

use serde_json::{Value, json};
use tauri::{AppHandle, Manager, State, ipc::Channel};
use tauri_plugin_updater::{Update, UpdaterExt};

#[derive(Default)]
pub struct Updates {
    current: Mutex<Value>,
    ready: Mutex<Option<(Update, Vec<u8>)>>,
    operation: tokio::sync::Mutex<()>,
}

fn configured(app: &AppHandle) -> bool {
    app.config().plugins.0.get("updater").is_some_and(|config| {
        config["pubkey"].as_str().is_some_and(|key| !key.is_empty())
            && config["endpoints"]
                .as_array()
                .is_some_and(|endpoints| !endpoints.is_empty())
    })
}

fn now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

fn state(app: &AppHandle, updates: &Updates) -> Value {
    let version = app.package_info().version.to_string();
    if !configured(app) {
        return json!({ "state": "manual", "current": version, "reason": "此预览版使用手动更新" });
    }
    let saved = updates.current.lock().expect("update state");
    if saved.is_null() {
        json!({ "state": "idle", "current": version })
    } else {
        saved.clone()
    }
}

pub fn current_state(app: &AppHandle) -> Value {
    state(app, &app.state::<Updates>())
}

fn publish(app: &AppHandle, updates: &Updates, events: Option<&Channel<Value>>, value: Value) -> Value {
    *updates.current.lock().expect("update state") = value.clone();
    if let Some(channel) = events {
        let _ = channel.send(value.clone());
    }
    crate::desktop::refresh_tray(app);
    value
}

#[tauri::command]
pub fn updates_get(app: AppHandle, updates: State<'_, Updates>) -> Value {
    state(&app, &updates)
}

async fn run_check(app: &AppHandle, updates: &Updates, events: Option<&Channel<Value>>) -> Value {
    if !configured(app) {
        return state(app, updates);
    }
    let Ok(_operation) = updates.operation.try_lock() else {
        return state(app, updates);
    };
    if updates.ready.lock().expect("downloaded update").is_some() {
        return state(app, updates);
    }
    let current = app.package_info().version.to_string();
    publish(
        app,
        updates,
        events,
        json!({ "state": "checking", "current": current }),
    );
    let result = async {
        let Some(update) = app.updater()?.check().await? else {
            return Ok(json!({ "state": "latest", "current": current, "checked_at": now() }));
        };
        let version = update.version.clone();
        publish(
            app,
            updates,
            events,
            json!({ "state": "downloading", "current": current, "version": version, "percent": 0 }),
        );
        let mut downloaded = 0_u64;
        let mut previous = 0;
        let bytes = update
            .download(
                |length, total| {
                    downloaded += length as u64;
                    if let Some(total) = total.filter(|total| *total > 0) {
                        let percent = (100 * downloaded / total).min(100);
                        if percent != previous {
                            previous = percent;
                            publish(
                                app,
                                updates,
                                events,
                                json!({ "state": "downloading", "current": current, "version": version, "percent": percent }),
                            );
                        }
                    }
                },
                || {},
            )
            .await?;
        *updates.ready.lock().expect("downloaded update") = Some((update, bytes));
        Ok::<_, tauri_plugin_updater::Error>(
            json!({ "state": "ready", "current": current, "version": version }),
        )
    }
    .await;
    let next = result.unwrap_or_else(|error| {
        json!({
            "state": "error", "current": current, "message": error.to_string(), "checked_at": now(),
        })
    });
    publish(app, updates, events, next)
}

async fn run_install(
    app: &AppHandle,
    updates: &Updates,
    events: Option<&Channel<Value>>,
) -> Result<(), String> {
    let _operation = updates
        .operation
        .try_lock()
        .map_err(|_| "正在处理另一个更新")?;
    let (update, bytes) = updates
        .ready
        .lock()
        .expect("downloaded update")
        .take()
        .ok_or("还没有下载可安装的更新")?;
    let result = tauri::async_runtime::spawn_blocking(move || update.install(bytes))
        .await
        .map_err(|err| err.to_string())?;
    if let Err(error) = result {
        publish(
            app,
            updates,
            events,
            json!({
                "state": "error", "current": app.package_info().version.to_string(), "message": error.to_string(), "checked_at": now(),
            }),
        );
        return Err(error.to_string());
    }
    app.restart();
}

#[tauri::command]
pub async fn updates_check(
    app: AppHandle,
    updates: State<'_, Updates>,
    events: Channel<Value>,
) -> Result<Value, String> {
    Ok(run_check(&app, &updates, Some(&events)).await)
}

#[tauri::command]
pub async fn updates_install(
    app: AppHandle,
    updates: State<'_, Updates>,
    events: Channel<Value>,
) -> Result<(), String> {
    run_install(&app, &updates, Some(&events)).await
}

pub fn check_from_tray(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let updates = app.state::<Updates>();
        let _ = run_check(&app, &updates, None).await;
    });
}

pub fn install_from_tray(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let updates = app.state::<Updates>();
        let _ = run_install(&app, &updates, None).await;
    });
}
