//! DeepSeek / 词元工坊 (`dsh`) accounts: one config directory per login (`DSH_HOME`; the
//! default is `~/.dsh`, shared with the 词元工坊 desktop app, while `deepseek-harness` uses
//! `~/.dsh-harness`). Login state comes from `<home>/.credentials.yaml`; interactive sign-in
//! runs `dsh web`. `dsh` does not currently expose a quota/usage endpoint.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use crossagents_core::{
    Account, AccountProvider, AccountStatus, CoreError, LoginCommand, ModelInfo, SessionUsage,
};
use crossagents_drivers::locate::Cli;
use crossagents_drivers::process::explicit_proxy_env;
use serde_json::Value;

const LIVE_TTL: Duration = Duration::from_secs(60);
const MODELS_URL: &str = "https://api.deepseek.com/models";
const MODELS_TIMEOUT: Duration = Duration::from_secs(5);
const NOT_INSTALLED: &str = "没有找到 dsh CLI（npm install -g @deepseek-ai/dsh@0.2.2-rc.2）";

enum CliLookup {
    Live {
        cli: Option<Cli>,
        locator: Option<crossagents_drivers::locate::Locator>,
    },
    Fixed(Option<Vec<String>>),
}

pub struct DeepseekAccounts {
    cli: CliLookup,
    models: Vec<String>,
    models_url: Option<String>,
    live: Mutex<Option<(Instant, PathBuf, AccountStatus)>>,
    models_cache: Mutex<Option<(Instant, PathBuf, Vec<ModelInfo>)>>,
}

impl Default for DeepseekAccounts {
    fn default() -> Self {
        Self {
            cli: CliLookup::Live {
                cli: crate::builtin::cli("tokenworks"),
                locator: crate::builtin::locator("tokenworks"),
            },
            models: crate::builtin::config("tokenworks")
                .map(|c| c.models.clone())
                .unwrap_or_default(),
            models_url: Some(MODELS_URL.to_owned()),
            live: Mutex::default(),
            models_cache: Mutex::default(),
        }
    }
}

impl DeepseekAccounts {
    pub fn new(cli: Option<Vec<String>>, models: Vec<String>) -> Self {
        Self {
            cli: CliLookup::Fixed(cli),
            models,
            models_url: None,
            live: Mutex::default(),
            models_cache: Mutex::default(),
        }
    }

    pub fn harness_home() -> Option<PathBuf> {
        user_home().map(|home| home.join(".dsh-harness"))
    }

    fn installed(&self) -> bool {
        match &self.cli {
            CliLookup::Live { cli, .. } => cli.as_ref().and_then(Cli::command).is_some(),
            CliLookup::Fixed(cmd) => cmd.is_some(),
        }
    }

    fn web_command(&self) -> Vec<String> {
        match &self.cli {
            CliLookup::Live { locator, .. } => {
                if let Some(found) = locator.as_ref().and_then(|l| l.locate())
                    && !found.argv.is_empty()
                {
                    let mut cmd = found.argv;
                    cmd.push("web".to_owned());
                    return cmd;
                }
                vec!["dsh".to_owned(), "web".to_owned()]
            }
            CliLookup::Fixed(cmd) => {
                let mut out = cmd.clone().unwrap_or_else(|| vec!["dsh".to_owned()]);
                out.push("web".to_owned());
                out
            }
        }
    }

    fn is_default_home(&self, account: &Account) -> bool {
        self.default_home()
            .is_some_and(|home| same_path(&home, &account.home))
    }

    /// Sets `DSH_HOME` only for non-default homes (e.g. `~/.dsh-harness` or a custom added
    /// directory); leaving it unset shares `~/.dsh` with the 词元工坊 desktop app.
    fn env_for(&self, account: &Account) -> Vec<(String, String)> {
        if self.is_default_home(account) {
            return Vec::new();
        }
        vec![("DSH_HOME".to_owned(), account.home.display().to_string())]
    }

    fn standing(&self, account: &Account) -> (bool, Option<String>) {
        let Ok(text) = std::fs::read_to_string(account.home.join(".credentials.yaml")) else {
            return (false, None);
        };
        parse_credentials_yaml(&text)
    }

    fn credential_token(&self, account: &Account) -> Option<String> {
        let text = std::fs::read_to_string(account.home.join(".credentials.yaml")).ok()?;
        parse_credentials_token(&text)
    }

    async fn fetch_models(&self, url: &str, token: &str) -> Option<Vec<ModelInfo>> {
        let client = reqwest::Client::builder()
            .timeout(MODELS_TIMEOUT)
            .build()
            .ok()?;
        let body: Value = client
            .get(url)
            .header("x-dsh-auth-token", token)
            .header("accept", "application/json")
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?
            .json()
            .await
            .ok()?;
        let parsed = parse_models_listing(&body);
        (!parsed.is_empty()).then_some(parsed)
    }
}

#[async_trait]
impl AccountProvider for DeepseekAccounts {
    fn name(&self) -> &str {
        "deepseek"
    }

    fn default_home(&self) -> Option<PathBuf> {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            let desktop = PathBuf::from(appdata).join("tokenworks-desktop-harness");
            if desktop.join(".credentials.yaml").is_file() {
                return Some(desktop);
            }
        }
        user_home().map(|home| home.join(".dsh"))
    }

    async fn status(&self, account: &Account) -> AccountStatus {
        let installed = self.installed();
        if let Some((at, home, cached)) = self.live.lock().unwrap().as_ref()
            && at.elapsed() < LIVE_TTL
            && same_path(home, &account.home)
            && !(cached.note.as_deref() == Some(NOT_INSTALLED) && installed)
        {
            return cached.clone();
        }
        let (logged_in, auth_mode) = self.standing(account);
        let note = if !installed {
            Some(NOT_INSTALLED.to_owned())
        } else if !logged_in {
            Some(
                "尚未登录：点击「登录」将启动浏览器登录 DeepSeek；也可以先在终端运行 `dsh web` 完成登录"
                    .to_owned(),
            )
        } else if self.is_default_home(account) {
            Some("与 词元工坊 桌面端共用 ~/.dsh 登录状态；DeepSeek CLI 暂未提供额度查询接口".into())
        } else {
            Some(format!(
                "使用独立目录 DSH_HOME={}；DeepSeek CLI 暂未提供额度查询接口",
                account.home.display()
            ))
        };
        let status = AccountStatus {
            logged_in,
            auth_mode,
            email: None,
            plan: None,
            subscription_until: None,
            rate_limits: Vec::new(),
            limit_reached: false,
            credits: None,
            limits: Vec::new(),
            reset_credits: Vec::new(),
            source: None,
            as_of: None,
            note,
        };
        *self.live.lock().unwrap() = Some((Instant::now(), account.home.clone(), status.clone()));
        status
    }

    async fn sessions(&self, _account: &Account) -> Result<Vec<SessionUsage>, CoreError> {
        Ok(Vec::new())
    }

    async fn models(&self, account: &Account) -> Vec<ModelInfo> {
        if let Some((at, home, cached)) = self.models_cache.lock().unwrap().as_ref()
            && at.elapsed() < LIVE_TTL
            && same_path(home, &account.home)
        {
            return cached.clone();
        }
        if let Some(url) = &self.models_url
            && let Some(token) = self.credential_token(account)
            && let Some(fetched) = self.fetch_models(url, &token).await
        {
            *self.models_cache.lock().unwrap() =
                Some((Instant::now(), account.home.clone(), fetched.clone()));
            return fetched;
        }
        self.models
            .iter()
            .cloned()
            .map(|id| ModelInfo {
                name: id.clone(),
                id,
                description: None,
                context_window: None,
                credit_multiplier: None,
            })
            .collect()
    }

    fn login_command(&self, account: &Account) -> LoginCommand {
        LoginCommand {
            command: self.web_command(),
            env: self.env_for(account).into_iter().collect(),
            env_remove: Vec::new(),
            serves: None,
            opens_browser: true,
        }
    }

    fn forget_status(&self, _account: &Account) {
        *self.live.lock().unwrap() = None;
        *self.models_cache.lock().unwrap() = None;
    }

    fn agent_env(&self, account: &Account) -> Vec<(String, String)> {
        let mut env = self.env_for(account);
        env.extend(explicit_proxy_env(account.proxy.as_deref()));
        env
    }
}

fn user_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn same_path(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(a) == canon(b)
}

fn clean_yaml_scalar(raw: &str) -> &str {
    let trimmed = raw.trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    }
}

fn has_value(raw: &str) -> bool {
    let val = clean_yaml_scalar(raw);
    !val.is_empty() && !matches!(val, "null" | "~" | "false")
}

fn parse_credentials_token(text: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(val) = trimmed.strip_prefix("token:")
            && has_value(val)
        {
            return Some(clean_yaml_scalar(val).to_owned());
        }
    }
    None
}

/// Inspects `dsh`'s `.credentials.yaml` without needing a full YAML dependency: a real
/// sign-in carries a non-empty `token:` grant (or API key), whereas a pre-login device /
/// browser-session record alone does not mark the account signed in.
fn parse_credentials_yaml(text: &str) -> (bool, Option<String>) {
    if parse_credentials_token(text).is_some() {
        return (true, Some("oauth".to_owned()));
    }
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(val) = trimmed
            .strip_prefix("apiKey:")
            .or_else(|| trimmed.strip_prefix("api_key:"))
            && has_value(val)
        {
            return (true, Some("api-key".to_owned()));
        }
    }
    (false, None)
}

fn parse_models_listing(body: &Value) -> Vec<ModelInfo> {
    let mut out = Vec::new();
    let Some(items) = body.get("data").and_then(Value::as_array) else {
        return out;
    };
    for item in items {
        let Some(id) = item
            .get("name")
            .or_else(|| item.get("id"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        if out.iter().any(|existing: &ModelInfo| existing.id == id) {
            continue;
        }
        out.push(ModelInfo {
            id: id.to_owned(),
            name: id.to_owned(),
            description: item
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_owned),
            context_window: item.get("context_window").and_then(Value::as_u64),
            credit_multiplier: None,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn account(home: &Path) -> Account {
        Account {
            id: "tokenworks".into(),
            provider: "deepseek".into(),
            label: "词元工坊".into(),
            home: home.to_path_buf(),
            created_at: Utc::now(),
            proxy: None,
            egress: None,
            codebuddy_site: None,
        }
    }

    #[tokio::test]
    async fn detects_login_from_credentials_yaml_and_sets_dsh_home() {
        let dir = tempfile::tempdir().unwrap();
        let acc = account(dir.path());
        let provider = DeepseekAccounts::new(Some(vec!["dsh".into()]), vec!["DeepSeek-V4-Pro".into()]);

        // Device grant without a token must stay logged out.
        std::fs::write(
            dir.path().join(".credentials.yaml"),
            "version: 1\nrecords:\n  deepseek-account-platform/device:\n    kind: grant\n    payload:\n      id: dev-1\n",
        )
        .unwrap();
        let status = provider.status(&acc).await;
        assert!(!status.logged_in);

        provider.forget_status(&acc);
        std::fs::write(
            dir.path().join(".credentials.yaml"),
            "version: 1\nrecords:\n  deepseek-account-platform/default:\n    kind: grant\n    payload:\n      version: 1\n      token: secret-token\n",
        )
        .unwrap();
        let status = provider.status(&acc).await;
        assert!(status.logged_in);
        assert_eq!(status.auth_mode.as_deref(), Some("oauth"));

        let env = provider.agent_env(&acc);
        assert!(env.iter().any(|(k, v)| k == "DSH_HOME" && Path::new(v) == dir.path()));

        assert_eq!(
            parse_credentials_token(
                "records:\n  deepseek-account-platform/default:\n    payload:\n      token: \"tok-123\"\n"
            )
            .as_deref(),
            Some("tok-123")
        );
        let parsed = parse_models_listing(&serde_json::json!({
            "object": "list",
            "data": [
                {"id": "deepseek-flash", "name": "DeepSeek-V4.1-Flash", "context_window": 1048576},
                {"id": "deepseek-v4-pro", "name": "DeepSeek-V4-Pro", "context_window": 1048576}
            ]
        }));
        assert_eq!(
            parsed.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["DeepSeek-V4.1-Flash", "DeepSeek-V4-Pro"]
        );
        assert_eq!(parsed[0].context_window, Some(1_048_576));
    }
}
