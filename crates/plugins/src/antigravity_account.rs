//! Antigravity CLI accounts (docs/research/antigravity-cli.md). The CLI signs in with a
//! Google account and keeps the credential in the OS keyring — not under `~/.gemini` — so a
//! machine has one Antigravity login: another account's home does not change who is signed
//! in, and `HOME` isolation cannot separate two of them.
//!
//! Signing in is `crossagents agy-login`: `agy acp --login` prints a Google link and reads
//! the pasted code from its stdin, so the helper serves a page for exactly that. The hub
//! hands the page's address to the GUI as the login page and watches the account until the
//! CLI reports a sign-in.
//!
//! What the account page can say: whether the CLI is signed in, and the models it lists.
//! There is no local record of the plan or the quota (`agy` keeps that to itself), and no
//! session files with token counts — per-turn tokens come from the driver's event stream.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use crossagents_core::{
    Account, AccountProvider, AccountStatus, CoreError, LoginCommand, ModelInfo, SessionUsage,
};
use crossagents_drivers::antigravity_native::wire::parse_models;
use crossagents_drivers::process::{hide_console, resolve_program};

/// The CLI is asked at most this often.
const LIVE_TTL: Duration = Duration::from_secs(60);
/// `agy models` that takes longer is treated as failed.
const CLI_TIMEOUT: Duration = Duration::from_secs(20);

const NOT_INSTALLED: &str =
    "the Antigravity CLI is not installed (irm https://antigravity.google/cli/install.ps1 | iex)";

const SIGN_IN_NOTE: &str =
    "还没有登录：点「登录」会打开一个本地页面，里面有 Google 授权链接，把授权码粘进去就完成";

pub struct AntigravityAccounts {
    /// How to run the CLI; `None` when it is not installed.
    cli: Option<Vec<String>>,
    /// How to start the login helper (`<crossagents> agy-login`), when the hub has one.
    login: Option<Vec<String>>,
    /// The description's model list, for when the CLI cannot be asked.
    models: Vec<String>,
    live: Mutex<Option<(Instant, AccountStatus, Vec<ModelInfo>)>>,
}

impl Default for AntigravityAccounts {
    fn default() -> Self {
        Self::new(
            crate::builtin::cli("antigravity").and_then(|cli| cli.command()),
            None,
            Vec::new(),
        )
    }
}

impl AntigravityAccounts {
    pub fn new(cli: Option<Vec<String>>, login: Option<Vec<String>>, models: Vec<String>) -> Self {
        Self {
            cli,
            login,
            models,
            live: Mutex::default(),
        }
    }

    /// `agy models`, as the CLI printed it. A non-zero exit is not an error here: the
    /// command exits 1 when it is not signed in, and that is an answer too.
    async fn run(&self, args: &[&str]) -> Result<(String, String), String> {
        let Some(command) = &self.cli else {
            return Err(NOT_INSTALLED.into());
        };
        let (program, rest) = command
            .split_first()
            .ok_or_else(|| "empty Antigravity command".to_owned())?;
        let mut child = tokio::process::Command::new(resolve_program(program));
        child
            .args(rest)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        hide_console(&mut child);
        let output = tokio::time::timeout(CLI_TIMEOUT, child.output())
            .await
            .map_err(|_| {
                format!(
                    "`agy {}` did not answer in {} s",
                    args.join(" "),
                    CLI_TIMEOUT.as_secs()
                )
            })?
            .map_err(|e| format!("cannot run the Antigravity CLI: {e}"))?;
        Ok((
            String::from_utf8_lossy(&output.stdout).into_owned(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ))
    }

    /// Whether the CLI is signed in, and what it lists. Signed out is the CLI saying so:
    /// `agy models` answers `Please sign in to view available models.`
    async fn probe(&self) -> (bool, Vec<ModelInfo>, Option<String>) {
        let (stdout, stderr) = match self.run(&["models"]).await {
            Ok(answer) => answer,
            Err(err) => return (false, Vec::new(), Some(err)),
        };
        let text = format!("{stdout}\n{stderr}");
        let signed_out = text.to_ascii_lowercase().contains("sign in");
        let models: Vec<ModelInfo> = parse_models(&stdout)
            .into_iter()
            .map(|id| ModelInfo {
                name: id.clone(),
                id,
                description: None,
                context_window: None,
                credit_multiplier: None,
            })
            .collect();
        let note = if signed_out {
            Some(SIGN_IN_NOTE.to_owned())
        } else {
            // Not signed out and nothing listed: the CLI's own line says why (a network
            // failure, a model list it could not fetch).
            text.lines()
                .find(|line| line.trim_start().starts_with("Error:"))
                .map(|line| line.trim().to_owned())
        };
        (!signed_out, models, note)
    }
}

#[async_trait]
impl AccountProvider for AntigravityAccounts {
    fn name(&self) -> &str {
        "antigravity"
    }

    fn default_home(&self) -> Option<PathBuf> {
        let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
        Some(PathBuf::from(home))
    }

    async fn status(&self, _account: &Account) -> AccountStatus {
        if let Some((at, status, _)) = self.live.lock().unwrap().as_ref()
            && at.elapsed() < LIVE_TTL
        {
            return status.clone();
        }
        let (logged_in, models, note) = self.probe().await;
        let status = AccountStatus {
            logged_in,
            auth_mode: logged_in.then(|| "google".to_owned()),
            // The credential is in the keyring and the CLI never prints the address.
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
        *self.live.lock().unwrap() = Some((Instant::now(), status.clone(), models));
        status
    }

    /// Nothing to count locally: `agy` keeps conversations in its own store without token
    /// counts. The driver reports each turn's usage from the event stream instead.
    async fn sessions(&self, _account: &Account) -> Result<Vec<SessionUsage>, CoreError> {
        Ok(Vec::new())
    }

    async fn models(&self, account: &Account) -> Vec<ModelInfo> {
        if self.live.lock().unwrap().is_none() {
            self.status(account).await;
        }
        let cached = self
            .live
            .lock()
            .unwrap()
            .as_ref()
            .map(|(_, _, models)| models.clone())
            .unwrap_or_default();
        if !cached.is_empty() {
            return cached;
        }
        // The description's list stands in when the CLI listed nothing (signed out).
        self.models
            .iter()
            .map(|id| ModelInfo {
                name: id.clone(),
                id: id.clone(),
                description: None,
                context_window: None,
                credit_multiplier: None,
            })
            .collect()
    }

    /// `<crossagents> agy-login --command <agy> --port <n>`: the helper serves the page the
    /// GUI opens, and writes the pasted code into `agy acp --login`. The hub watches the
    /// account until the CLI reports a sign-in.
    fn login_command(&self, _account: &Account) -> LoginCommand {
        let port = free_port().unwrap_or(8323);
        let mut command = self.login.clone().unwrap_or_else(|| {
            vec![
                std::env::current_exe()
                    .map(|exe| exe.display().to_string())
                    .unwrap_or_else(|_| "crossagents".into()),
                "agy-login".into(),
            ]
        });
        // `--command` takes everything after it, so the port goes first.
        command.push("--port".into());
        command.push(port.to_string());
        command.push("--command".into());
        match &self.cli {
            Some(cli) => command.extend(cli.iter().cloned()),
            None => command.push("agy".into()),
        }
        LoginCommand {
            command,
            env: Default::default(),
            env_remove: Vec::new(),
            serves: Some(format!("http://127.0.0.1:{port}/")),
            opens_browser: false,
        }
    }

    fn forget_status(&self, _account: &Account) {
        *self.live.lock().unwrap() = None;
    }

    /// Nothing: the credential is in the OS keyring, not in the account's home, so there is
    /// no variable that would point the CLI at another login.
    fn agent_env(&self, _account: &Account) -> Vec<(String, String)> {
        Vec::new()
    }
}

/// A port that was free a moment ago.
fn free_port() -> Option<u16> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).ok()?;
    Some(listener.local_addr().ok()?.port())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn account(home: &std::path::Path) -> Account {
        Account {
            codebuddy_site: None,
            egress: None,
            id: "antigravity-main".into(),
            provider: "antigravity".into(),
            label: "Antigravity".into(),
            home: home.to_path_buf(),
            created_at: Utc::now(),
        }
    }

    /// A stand-in for `agy models`.
    fn fake_cli(answer: &str) -> (Vec<String>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-agy.mjs");
        std::fs::write(&script, format!("process.stdout.write({answer:?});")).unwrap();
        (vec!["node".into(), script.display().to_string()], dir)
    }

    #[tokio::test]
    async fn a_signed_out_cli_says_so_and_offers_the_login() {
        let (cli, _dir) = fake_cli("Error: Please sign in to view available models.");
        let home = tempfile::tempdir().unwrap();
        let accounts = AntigravityAccounts::new(Some(cli), None, Vec::new());
        let status = accounts.status(&account(home.path())).await;
        assert!(!status.logged_in);
        assert!(status.auth_mode.is_none());
        assert_eq!(status.note.as_deref(), Some(SIGN_IN_NOTE));
        assert!(accounts.models(&account(home.path())).await.is_empty());
    }

    #[tokio::test]
    async fn a_listing_is_a_signed_in_account_with_models() {
        let (cli, _dir) = fake_cli(
            "Gemini 3.8 Flash (High)   gemini-3.8-flash-high\n\
             Gemini 3.1 Pro (High)     gemini-3.1-pro-high\n\
             auto\n",
        );
        let home = tempfile::tempdir().unwrap();
        let accounts = AntigravityAccounts::new(Some(cli), None, Vec::new());
        let status = accounts.status(&account(home.path())).await;
        assert!(status.logged_in);
        assert_eq!(status.auth_mode.as_deref(), Some("google"));
        assert!(status.note.is_none());
        let models: Vec<_> = accounts
            .models(&account(home.path()))
            .await
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(
            models,
            ["gemini-3.8-flash-high", "gemini-3.1-pro-high", "auto"]
        );
    }

    #[tokio::test]
    async fn a_cli_that_is_not_installed_is_not_a_signed_in_account() {
        let home = tempfile::tempdir().unwrap();
        let accounts = AntigravityAccounts::new(None, None, vec!["gemini-3.8-flash".into()]);
        let status = accounts.status(&account(home.path())).await;
        assert!(!status.logged_in);
        assert_eq!(status.note.as_deref(), Some(NOT_INSTALLED));
        // The description's list stands in when the CLI cannot be asked.
        let models: Vec<_> = accounts
            .models(&account(home.path()))
            .await
            .into_iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(models, ["gemini-3.8-flash"]);
    }

    #[tokio::test]
    async fn the_login_runs_the_helper_against_the_cli_and_a_local_page() {
        let home = tempfile::tempdir().unwrap();
        let accounts = AntigravityAccounts::new(
            Some(vec!["agy".into()]),
            Some(vec!["crossagents".into(), "agy-login".into()]),
            Vec::new(),
        );
        let login = accounts.login_command(&account(home.path()));
        assert_eq!(login.command[..2], ["crossagents", "agy-login"]);
        assert_eq!(login.command[2], "--port");
        let port: u16 = login.command[3].parse().expect("a port");
        assert_eq!(login.command[4..], ["--command", "agy"]);
        assert_eq!(
            login.serves.as_deref(),
            Some(format!("http://127.0.0.1:{port}/").as_str())
        );
        assert!(!login.opens_browser);
        assert!(login.env.is_empty() && login.env_remove.is_empty());
    }

    #[test]
    fn the_default_home_is_the_machines() {
        let accounts = AntigravityAccounts::default();
        let home = accounts.default_home().expect("a home");
        assert!(home.is_absolute());
    }
}
