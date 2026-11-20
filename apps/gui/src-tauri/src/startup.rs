use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use reqwest::Client;
use semver::Version;
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::{Hub, hub_home};

#[derive(Default)]
pub struct BundledHub {
    last_attempt: Mutex<Option<(Instant, Result<(), String>)>>,
}

impl BundledHub {
    pub async fn ensure_running(&self, client: &Client, shipped: &Version) -> Result<(), String> {
        let mut last_attempt = self.last_attempt.lock().await;
        if let Some((when, result)) = &*last_attempt
            && when.elapsed() < Duration::from_secs(if result.is_ok() { 5 } else { 30 })
        {
            return result.clone();
        }
        let healthy = if let Ok(hub) = Hub::discover().await
            && let Ok(url) = hub.base.join("/v1/info")
            && client
                .get(url)
                .bearer_auth(&hub.token)
                .timeout(Duration::from_secs(3))
                .send()
                .await
                .is_ok_and(|response| response.status().is_success())
        {
            true
        } else {
            false
        };
        let shipped = shipped.clone();
        let result = tauri::async_runtime::spawn_blocking(move || {
            let replace = if healthy {
                let home = hub_home()?;
                let Some(upgrade) = running(&home).filter(|hub| {
                    inside(&hub.exe, &home.join("bin"))
                        && Version::parse(&hub.version).is_ok_and(|version| version < shipped)
                }) else {
                    return Ok(());
                };
                Some(upgrade)
            } else {
                None
            };
            start(replace)
        })
        .await
        .map_err(|err| err.to_string())
        .and_then(|result| result);
        *last_attempt = Some((Instant::now(), result.clone()));
        result
    }

    pub async fn use_shipped(&self) -> Result<(), String> {
        let mut last_attempt = self.last_attempt.lock().await;
        let result = tauri::async_runtime::spawn_blocking(|| {
            let home = hub_home()?;
            let hub = running(&home)
                .filter(|hub| inside(&hub.exe, &home.join("shadow")))
                .ok_or("当前运行的 Hub 不属于开发版本")?;
            start(Some(hub))
        })
        .await
        .map_err(|err| err.to_string())?;
        *last_attempt = None;
        result
    }

    pub async fn stop_all(&self) -> Result<(), String> {
        let mut last_attempt = self.last_attempt.lock().await;
        let result = tauri::async_runtime::spawn_blocking(|| {
            let home = hub_home()?;
            let shipped = shipped_executable()?;
            let output = run(&shipped, &["hub", "stop", "--agents"], Some(&home))
                .map_err(|err| err.to_string())?;
            if output.status.success() {
                Ok(())
            } else {
                Err(String::from_utf8_lossy(&output.stderr).trim().into())
            }
        })
        .await
        .map_err(|err| err.to_string())?;
        // Keep relays from restarting a Hub while the desktop process exits.
        *last_attempt = Some((Instant::now(), Err("正在退出 CrossAgents".into())));
        result
    }
}

#[derive(Deserialize, PartialEq, Eq)]
struct Running {
    pid: u32,
    exe: PathBuf,
    version: String,
}

fn running(home: &Path) -> Option<Running> {
    serde_json::from_slice(&fs::read(home.join("hub.json")).ok()?).ok()
}

fn inside(file: &Path, base: &Path) -> bool {
    file.canonicalize()
        .ok()
        .zip(base.canonicalize().ok())
        .is_some_and(|(file, base)| file.starts_with(&base) && file != base)
}

pub async fn development_hub(shipped: &Version) -> Option<serde_json::Value> {
    let shipped = shipped.to_string();
    tauri::async_runtime::spawn_blocking(move || {
        let home = hub_home().ok()?;
        let hub = running(&home)?;
        inside(&hub.exe, &home.join("shadow")).then(|| {
            serde_json::json!({
                "exe": hub.exe, "running": hub.version, "shipped": shipped,
            })
        })
    })
    .await
    .ok()
    .flatten()
}

fn shipped_executable() -> Result<PathBuf, String> {
    Ok(std::env::current_exe()
        .map_err(|err| err.to_string())?
        .with_file_name(hub_executable()))
}

fn start(replace: Option<Running>) -> Result<(), String> {
    let home = hub_home()?;
    let shipped = shipped_executable()?;
    let result = (|| -> std::io::Result<_> {
        let version = match version_of(&shipped)? {
            Some(version) => version,
            None => return Err(std::io::Error::other("invalid bundled Hub version")),
        };
        let exe = install(&shipped, &home, &version)?;
        if replace
            .as_ref()
            .is_some_and(|expected| running(&home).as_ref() == Some(expected))
        {
            let stopped = run(&exe, &["hub", "stop"], Some(&home))?;
            if !stopped.status.success() {
                return Err(std::io::Error::other(
                    String::from_utf8_lossy(&stopped.stderr).trim(),
                ));
            }
        }
        run(&exe, &["hub", "start"], Some(&home))
    })()
    .map_err(|err| format!("无法启动随附的 Hub：{err}"))?;
    if result.status.success() {
        Ok(())
    } else {
        Err(format!(
            "无法启动随附的 Hub：{}",
            String::from_utf8_lossy(&result.stderr).trim()
        ))
    }
}

fn hub_executable() -> &'static str {
    if cfg!(windows) {
        "crossagents.exe"
    } else {
        "crossagents"
    }
}

fn version_of(exe: &Path) -> std::io::Result<Option<String>> {
    let output = run(exe, &["--version"], None)?;
    let text = String::from_utf8_lossy(&output.stdout);
    let version = text.trim().strip_prefix("crossagents ");
    Ok(version
        .filter(|value| {
            output.status.success()
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
                && value.as_bytes().first().is_some_and(u8::is_ascii_digit)
        })
        .map(str::to_owned))
}

fn install(shipped: &Path, home: &Path, version: &str) -> std::io::Result<PathBuf> {
    let directory = home.join("bin").join(version);
    let target = directory.join(hub_executable());
    if target.is_file() {
        return Ok(target);
    }
    fs::create_dir_all(&directory)?;
    let partial = tempfile::NamedTempFile::new_in(&directory)?;
    fs::copy(shipped, partial.path())?;
    match partial.persist_noclobber(&target) {
        Ok(_) => Ok(target),
        Err(err) if err.error.kind() == std::io::ErrorKind::AlreadyExists && target.is_file() => {
            Ok(target)
        }
        Err(err) => Err(err.error),
    }
}

fn run(exe: &Path, args: &[&str], home: Option<&Path>) -> std::io::Result<std::process::Output> {
    let mut command = Command::new(exe);
    command.args(args).stdin(Stdio::null());
    if let Some(home) = home {
        command.env("CROSSAGENTS_HOME", home);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    command.output()
}
