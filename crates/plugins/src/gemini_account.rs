//! Gemini CLI accounts (docs/research/gemini-cli.md). The CLI keeps one login per home:
//! `~/.gemini/oauth_creds.json` (Google sign-in) or `~/.gemini/settings.json` naming an API
//! key. There is no quota API to read, so the account page shows who is signed in and which
//! models the CLI offers, and nothing about the plan.
//!
//! Signing in is the ACP round trip in `crossagents gemini-login`: the CLI has no `login`
//! subcommand, but its ACP mode takes `authenticate` and opens the browser itself. A second
//! account gets its own home (`GEMINI_CLI_HOME`), which is where the CLI reads `.gemini`
//! from — the machine login stays untouched.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use crossagents_core::{
    Account, AccountProvider, AccountStatus, CoreError, LoginCommand, ModelInfo, SessionUsage,
    mask_email,
};
use crossagents_drivers::gemini_native::wire::parse_models;
use crossagents_drivers::process::{hide_console, resolve_program};
use serde_json::Value;

/// The CLI is asked at most this often.
const LIVE_TTL: Duration = Duration::from_secs(60);
/// `--list-models` that takes longer is treated as failed: the description's list stands in.
const CLI_TIMEOUT: Duration = Duration::from_secs(20);
/// Where the CLI keeps its files inside a home.
const CONFIG_DIR: &str = ".gemini";

const NOT_INSTALLED: &str = "the Gemini CLI is not installed (npm install -g @google/gemini-cli)";

pub struct GeminiAccounts {
    /// How to run the CLI; `None` when it is not installed.
    cli: Option<Vec<String>>,
    /// How to start the login helper (`<crossagents> gemini-login`), when the hub has one.
    login: Option<Vec<String>>,
    /// The description's model list, for when the CLI cannot be asked.
    models: Vec<String>,
    live: Mutex<Option<(Instant, AccountStatus)>>,
}

impl Default for GeminiAccounts {
    fn default() -> Self {
        Self::new(
            crate::builtin::cli("gemini").and_then(|cli| cli.command()),
            None,
            Vec::new(),
        )
    }
}

impl GeminiAccounts {
    pub fn new(cli: Option<Vec<String>>, login: Option<Vec<String>>, models: Vec<String>) -> Self {
        Self {
            cli,
            login,
            models,
            live: Mutex::default(),
        }
    }

    /// `GEMINI_CLI_HOME` is the directory that *contains* `.gemini`, so an account's home is
    /// a home, not a config directory. The machine's own login is the default one.
    fn env_for(&self, account: &Account) -> Vec<(String, String)> {
        if self
            .default_home()
            .is_some_and(|home| same_path(&home, &account.home))
        {
            return Vec::new();
        }
        vec![(
            "GEMINI_CLI_HOME".to_owned(),
            account.home.display().to_string(),
        )]
    }

    fn config_dir(&self, account: &Account) -> PathBuf {
        account.home.join(CONFIG_DIR)
    }

    /// What the CLI wrote down about this account's sign-in, read without starting it.
    fn standing(&self, account: &Account) -> (bool, Option<String>, Option<String>) {
        let dir = self.config_dir(account);
        let read = |name: &str| {
            std::fs::read_to_string(dir.join(name))
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        };
        let mut logged_in = false;
        let mut auth_mode = None;
        if let Some(creds) = read("oauth_creds.json") {
            let token = |key: &str| {
                creds[key]
                    .as_str()
                    .filter(|value| !value.trim().is_empty())
                    .is_some()
            };
            if token("access_token") || token("refresh_token") {
                logged_in = true;
                auth_mode = Some("oauth".to_owned());
            }
        }
        if let Some(settings) = read("settings.json") {
            let selected = settings["security"]["auth"]["selectedType"].as_str();
            if let Some(kind) = selected {
                auth_mode.get_or_insert_with(|| kind.to_owned());
                if kind == "gemini-api-key" || kind == "vertex-ai" {
                    // The key itself lives in the settings or the environment; the CLI
                    // decides whether it works, so this only says which way it signs in.
                    logged_in = true;
                }
            }
        }
        let email = read("google_accounts.json")
            .and_then(|accounts| accounts["active"].as_str().map(str::to_owned))
            .filter(|email| !email.trim().is_empty());
        (logged_in, auth_mode, email)
    }

    /// The CLI's answer to `args`, as text.
    async fn output(&self, account: &Account, args: &[&str]) -> Result<String, String> {
        let Some(command) = &self.cli else {
            return Err(NOT_INSTALLED.into());
        };
        let (program, rest) = command
            .split_first()
            .ok_or_else(|| "empty Gemini command".to_owned())?;
        let mut child = tokio::process::Command::new(resolve_program(program));
        child.args(rest).args(args);
        for (key, value) in self.env_for(account) {
            child.env(key, value);
        }
        child
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        hide_console(&mut child);
        let output = tokio::time::timeout(CLI_TIMEOUT, child.output())
            .await
            .map_err(|_| {
                format!(
                    "`gemini {}` did not answer in {} s",
                    args.join(" "),
                    CLI_TIMEOUT.as_secs()
                )
            })?
            .map_err(|e| format!("cannot run the Gemini CLI: {e}"))?;
        if !output.status.success() {
            let err = String::from_utf8_lossy(&output.stderr);
            let err = err.trim();
            return Err(if err.is_empty() {
                format!("`gemini {}` failed ({})", args.join(" "), output.status)
            } else {
                format!("`gemini {}` failed: {err}", args.join(" "))
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

#[async_trait]
impl AccountProvider for GeminiAccounts {
    fn name(&self) -> &str {
        "gemini"
    }

    fn default_home(&self) -> Option<PathBuf> {
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
        Some(PathBuf::from(home))
    }

    async fn status(&self, account: &Account) -> AccountStatus {
        if let Some((at, status)) = self.live.lock().unwrap().as_ref()
            && at.elapsed() < LIVE_TTL
        {
            return status.clone();
        }
        let (logged_in, auth_mode, email) = self.standing(account);
        let note = if self.cli.is_none() {
            Some(NOT_INSTALLED.to_owned())
        } else if !logged_in {
            Some(
                "还没有登录：点「登录」会在浏览器里打开 Google 授权页；自动登录不成功时，也可以在终端运行 gemini 登录一次"
                    .to_owned(),
            )
        } else {
            None
        };
        let status = AccountStatus {
            logged_in,
            auth_mode,
            email: email.as_deref().map(mask_email),
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
        *self.live.lock().unwrap() = Some((Instant::now(), status.clone()));
        status
    }

    /// The CLI's per-project session files are not read: they hold the conversation, not a
    /// token count this account page can add up.
    async fn sessions(&self, _account: &Account) -> Result<Vec<SessionUsage>, CoreError> {
        Ok(Vec::new())
    }

    async fn models(&self, account: &Account) -> Vec<ModelInfo> {
        let listed = self
            .output(account, &["--list-models"])
            .await
            .map(|text| parse_models(&text))
            .unwrap_or_default();
        let ids = if listed.is_empty() {
            self.models.clone()
        } else {
            listed
        };
        ids.into_iter()
            .map(|id| ModelInfo {
                name: id.clone(),
                id,
                description: None,
                context_window: None,
                credit_multiplier: None,
            })
            .collect()
    }

    /// `<crossagents> gemini-login --command <gemini>`: the ACP handshake that makes the CLI
    /// open its browser page. The hub watches the account until the credential file appears.
    fn login_command(&self, account: &Account) -> LoginCommand {
        let mut command = self.login.clone().unwrap_or_else(|| {
            vec![
                std::env::current_exe()
                    .map(|exe| exe.display().to_string())
                    .unwrap_or_else(|_| "crossagents".into()),
                "gemini-login".into(),
            ]
        });
        command.push("--command".into());
        match &self.cli {
            Some(cli) => command.extend(cli.iter().cloned()),
            None => command.push("gemini".into()),
        }
        let mut env: Vec<(String, String)> = self.env_for(account);
        // The browser's login is the point; a hub-wide key would only hide that it ran.
        env.push(("GEMINI_API_KEY".to_owned(), String::new()));
        LoginCommand {
            command,
            env: env
                .iter()
                .filter(|(_, value)| !value.is_empty())
                .cloned()
                .collect(),
            env_remove: vec!["GEMINI_API_KEY".to_owned()],
            serves: None,
            opens_browser: true,
        }
    }

    fn forget_status(&self, _account: &Account) {
        *self.live.lock().unwrap() = None;
    }

    fn agent_env(&self, account: &Account) -> Vec<(String, String)> {
        self.env_for(account)
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn account(home: &Path) -> Account {
        Account {
            codebuddy_site: None,
            egress: None,
            id: "gemini-main".into(),
            provider: "gemini".into(),
            label: "Gemini".into(),
            home: home.to_path_buf(),
            created_at: Utc::now(),
        }
    }

    fn write(home: &Path, name: &str, text: &str) {
        let dir = home.join(CONFIG_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), text).unwrap();
    }

    #[tokio::test]
    async fn a_credential_file_is_a_signed_in_account() {
        let home = tempfile::tempdir().unwrap();
        let accounts = GeminiAccounts::new(None, None, Vec::new());
        let status = accounts.status(&account(home.path())).await;
        assert!(!status.logged_in);
        assert!(status.note.is_some());

        write(
            home.path(),
            "oauth_creds.json",
            r#"{"access_token":"a","refresh_token":"r"}"#,
        );
        write(
            home.path(),
            "google_accounts.json",
            r#"{"active":"zhang@gmail.com","old":[]}"#,
        );
        accounts.forget_status(&account(home.path()));
        let status = accounts.status(&account(home.path())).await;
        assert!(status.logged_in);
        assert_eq!(status.auth_mode.as_deref(), Some("oauth"));
        assert_eq!(status.email.as_deref(), Some("z***@gmail.com"));
    }

    #[tokio::test]
    async fn an_api_key_in_the_settings_is_signed_in_too() {
        let home = tempfile::tempdir().unwrap();
        write(
            home.path(),
            "settings.json",
            r#"{"security":{"auth":{"selectedType":"gemini-api-key"}}}"#,
        );
        let status = GeminiAccounts::new(None, None, Vec::new())
            .status(&account(home.path()))
            .await;
        assert!(status.logged_in);
        assert_eq!(status.auth_mode.as_deref(), Some("gemini-api-key"));
    }

    #[tokio::test]
    async fn the_login_runs_the_helper_against_this_accounts_home() {
        let home = tempfile::tempdir().unwrap();
        let accounts = GeminiAccounts::new(
            Some(vec!["node".into(), "gemini.js".into()]),
            Some(vec!["crossagents".into(), "gemini-login".into()]),
            Vec::new(),
        );
        let login = accounts.login_command(&account(home.path()));
        assert_eq!(
            login.command,
            [
                "crossagents",
                "gemini-login",
                "--command",
                "node",
                "gemini.js"
            ]
        );
        assert!(login.opens_browser);
        assert_eq!(
            login.env.get("GEMINI_CLI_HOME").map(String::as_str),
            Some(home.path().display().to_string().as_str())
        );
        assert_eq!(login.env_remove, ["GEMINI_API_KEY"]);
    }

    #[tokio::test]
    async fn the_machine_login_is_used_without_an_override() {
        let accounts = GeminiAccounts::new(None, None, Vec::new());
        let default = accounts.default_home().expect("a home");
        assert!(accounts.env_for(&account(&default)).is_empty());
    }
}
