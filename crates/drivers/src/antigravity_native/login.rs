//! Signing in to Antigravity CLI: its own login mode, plus a page for the pasted code.
//!
//! `agy acp --login` is the CLI's non-interactive sign-in (docs/research/antigravity-cli.md).
//! It prints the Google sign-in URL on stdout, then reads the authorization code — or the
//! whole URL of the page it lands on — from stdin and exits. It waits there for as long as
//! stdin stays open, with no timeout of its own (measured: still waiting after 200 s), so a
//! code that is pasted a few minutes later still works.
//!
//! A login command the hub starts gets a closed stdin, which is why the code has nowhere to
//! go. This module is the missing half: `crossagents agy-login --command <agy> --port N`
//! starts the CLI, serves a one-field page on that port and writes whatever the person
//! pastes into the CLI's stdin. The hub hands the page's address to the GUI as the login
//! page, the same way CodeBuddy's own web UI is handed over.

use std::collections::VecDeque;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossagents_core::CoreError;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::process::{hide_console, resolve_program};

/// How long the CLI gets to print its sign-in link.
const URL_WAIT: Duration = Duration::from_secs(30);
/// How long the CLI gets to finish once a code has been pasted.
const CODE_WAIT: Duration = Duration::from_secs(120);
/// The most of a request that is read; a pasted code is a few kilobytes at most.
const REQUEST_LIMIT: usize = 64 * 1024;

/// Serves the login page on `port` until the CLI reports a finished sign-in.
///
/// `command` launches the CLI (program first); `acp --login` is appended to it.
pub async fn login(command: &[String], port: u16) -> Result<(), CoreError> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await.map_err(|e| {
        CoreError::Agent(format!(
            "cannot serve the Antigravity login page on port {port}: {e}"
        ))
    })?;
    let mut sign_in = SignIn::start(command).await?;
    loop {
        // The CLI can end on its own (a network error, a failed exchange): a fresh one
        // prints a fresh link, and the person only has to reload the page.
        if matches!(sign_in.child.try_wait(), Ok(Some(_))) {
            sign_in = SignIn::start(command).await?;
        }
        let Ok((mut socket, _)) = listener.accept().await else {
            continue;
        };
        let Some(request) = read_request(&mut socket).await else {
            continue;
        };
        match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/") => respond(&mut socket, "200 OK", &page(&sign_in.url, None)).await,
            ("POST", "/code") => {
                let code = form_value(&request.body, "code");
                let code = code.trim();
                if code.is_empty() {
                    let error = "没有粘贴授权码。";
                    respond(&mut socket, "200 OK", &page(&sign_in.url, Some(error))).await;
                    continue;
                }
                let mut line = String::with_capacity(code.len() + 1);
                line.push_str(code);
                line.push('\n');
                if let Err(err) = sign_in.stdin.write_all(line.as_bytes()).await {
                    sign_in = SignIn::start(command).await?;
                    let error = format!("无法把授权码交给 CLI：{err}");
                    respond(&mut socket, "200 OK", &page(&sign_in.url, Some(&error))).await;
                    continue;
                }
                let _ = sign_in.stdin.flush().await;
                match wait_exit(&mut sign_in.child, CODE_WAIT).await {
                    Exit::Done(true) => {
                        respond(&mut socket, "200 OK", DONE).await;
                        return Ok(());
                    }
                    Exit::Done(false) => {
                        // The CLI says why ("token exchange failed: …"); it is gone now, so
                        // a new one is started and the page offers a new link to try again.
                        let error = sign_in.error_text();
                        sign_in = SignIn::start(command).await?;
                        respond(&mut socket, "200 OK", &page(&sign_in.url, Some(&error))).await;
                    }
                    Exit::Running => {
                        respond(&mut socket, "200 OK", WAITING).await;
                    }
                }
            }
            _ => respond(&mut socket, "404 Not Found", NOT_FOUND).await,
        }
    }
}

/// One `agy acp --login` process, the link it printed, and the lines it has said since.
struct SignIn {
    child: tokio::process::Child,
    stdin: tokio::process::ChildStdin,
    url: String,
    /// The last lines of both streams, for the message when a code is refused.
    tail: Arc<Mutex<VecDeque<String>>>,
}

impl SignIn {
    async fn start(command: &[String]) -> Result<Self, CoreError> {
        let (program, args) = command
            .split_first()
            .ok_or_else(|| CoreError::Agent("empty Antigravity command".into()))?;
        let mut cmd = Command::new(resolve_program(program));
        cmd.args(args)
            .arg("acp")
            .arg("--login")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        hide_console(&mut cmd);
        let mut child = cmd
            .spawn()
            .map_err(|e| CoreError::Agent(format!("cannot start the Antigravity CLI: {e}")))?;
        let stdin = child.stdin.take().expect("piped stdin");
        let tail = Arc::new(Mutex::new(VecDeque::new()));
        let (lines_tx, mut lines_rx) = mpsc::unbounded_channel::<String>();
        let streams: Vec<Box<dyn AsyncRead + Unpin + Send>> = [
            child
                .stdout
                .take()
                .map(|s| Box::new(s) as Box<dyn AsyncRead + Unpin + Send>),
            child
                .stderr
                .take()
                .map(|s| Box::new(s) as Box<dyn AsyncRead + Unpin + Send>),
        ]
        .into_iter()
        .flatten()
        .collect();
        for stream in streams {
            let tx = lines_tx.clone();
            let tail = tail.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stream).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    {
                        let mut tail = tail.lock().expect("login tail");
                        tail.push_back(line.clone());
                        while tail.len() > 40 {
                            tail.pop_front();
                        }
                    }
                    if tx.send(line).is_err() {
                        break;
                    }
                }
            });
        }
        let url = wait_for_url(&mut lines_rx, &mut child).await?;
        // Nothing reads the lines after the link has been found, but the readers must not
        // stop on a full channel: draining keeps the tail up to date.
        tokio::spawn(async move { while lines_rx.recv().await.is_some() {} });
        Ok(Self {
            child,
            stdin,
            url,
            tail,
        })
    }

    /// The CLI's own words about a refused code: its last line that is not the link or a
    /// blank.
    fn error_text(&self) -> String {
        let tail = self.tail.lock().expect("login tail");
        tail.iter()
            .rev()
            .find(|line| !line.trim().is_empty() && !line.contains("https://"))
            .map(|line| line.trim().to_owned())
            .unwrap_or_else(|| "the Antigravity CLI did not say why".into())
    }
}

async fn wait_for_url(
    lines: &mut mpsc::UnboundedReceiver<String>,
    child: &mut tokio::process::Child,
) -> Result<String, CoreError> {
    let deadline = tokio::time::Instant::now() + URL_WAIT;
    loop {
        match tokio::time::timeout_at(deadline, lines.recv()).await {
            Ok(Some(line)) => {
                if let Some(url) = find_url(&line) {
                    return Ok(url);
                }
            }
            Ok(None) => {
                let _ = child.kill().await;
                return Err(CoreError::Agent(
                    "the Antigravity CLI stopped before printing its sign-in link".into(),
                ));
            }
            Err(_) => {
                let _ = child.kill().await;
                return Err(CoreError::Agent(format!(
                    "the Antigravity CLI did not print a sign-in link within {} s",
                    URL_WAIT.as_secs()
                )));
            }
        }
    }
}

enum Exit {
    Done(bool),
    Running,
}

async fn wait_exit(child: &mut tokio::process::Child, within: Duration) -> Exit {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Exit::Done(status.success()),
            Ok(None) => {}
            Err(_) => return Exit::Done(false),
        }
        if tokio::time::Instant::now() >= deadline {
            return Exit::Running;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

/// The first `https://` link in a line of CLI output.
fn find_url(line: &str) -> Option<String> {
    let start = line.find("https://")?;
    let url: String = line[start..]
        .chars()
        .take_while(|c| !c.is_whitespace() && !matches!(c, '"' | '\'' | '<' | '>' | ')'))
        .collect();
    Some(url.trim_end_matches(['.', ',', ';']).to_owned())
}

struct Request {
    method: String,
    path: String,
    body: Vec<u8>,
}

/// One HTTP/1.1 request: the request line, `Content-Length` bytes of body, nothing else.
async fn read_request(socket: &mut TcpStream) -> Option<Request> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        if buf.len() > REQUEST_LIMIT {
            return None;
        }
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.lines();
    let mut request = lines.next()?.split_whitespace();
    let method = request.next()?.to_owned();
    let path = request.next()?.split('?').next()?.to_owned();
    let length = lines
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0)
        .min(REQUEST_LIMIT);
    let mut body = buf[header_end..].to_vec();
    while body.len() < length {
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    body.truncate(length);
    Some(Request { method, path, body })
}

/// `code=<value>` out of a form body, URL-decoded.
fn form_value(body: &[u8], key: &str) -> String {
    let text = String::from_utf8_lossy(body);
    for pair in text.split('&') {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        if name == key {
            return percent_decode(value);
        }
    }
    String::new()
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => match u8::from_str_radix(&value[i + 1..i + 3], 16) {
                Ok(byte) => {
                    out.push(byte);
                    i += 2;
                }
                Err(_) => out.push(b'%'),
            },
            byte => out.push(byte),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

async fn respond(socket: &mut TcpStream, status: &str, html: &str) {
    let body = html.as_bytes();
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = socket.write_all(head.as_bytes()).await;
    let _ = socket.write_all(body).await;
    let _ = socket.flush().await;
}

const STYLE: &str = "body{font:15px/1.6 -apple-system,'Segoe UI',sans-serif;margin:0;padding:32px;\
color:#1c1c1e;background:#fafafa}h1{font-size:19px;margin:0 0 16px}ol{padding-left:20px}\
li{margin:6px 0}a{color:#0a66c2}form{margin:20px 0 8px;display:flex;gap:8px;flex-wrap:wrap}\
input{flex:1 1 320px;min-width:240px;padding:9px 11px;font:inherit;border:1px solid #c8c8cc;\
border-radius:7px;background:#fff}button{padding:9px 16px;font:inherit;border:0;border-radius:7px;\
background:#1c1c1e;color:#fff;cursor:pointer}.error{margin:16px 0 0;padding:10px 12px;\
border-radius:7px;background:#fdecea;color:#8a1c13;white-space:pre-wrap}\
.hint{color:#6b6b70;font-size:13px}.done{font-size:17px}";

/// The form: the CLI's link, a box for the code, and the last refusal when there is one.
fn page(url: &str, error: Option<&str>) -> String {
    let error = match error {
        Some(error) => format!(r#"<p class="error">{}</p>"#, escape(error)),
        None => String::new(),
    };
    format!(
        r#"<!doctype html>
<html lang="zh"><head><meta charset="utf-8"><title>登录 Antigravity</title>
<style>{STYLE}</style></head><body>
<h1>登录 Antigravity</h1>
<ol>
<li>打开 <a href="{url}" target="_blank" rel="noreferrer">Google 登录页</a>，用要用的账号登录。</li>
<li>登录完页面会给出一段授权码（复制整页地址也行），粘贴到下面。</li>
</ol>
{error}
<form method="post" action="/code">
<input name="code" autofocus autocomplete="off" spellcheck="false"
 placeholder="授权码，或登录后落地页的完整地址">
<button type="submit">完成登录</button>
</form>
<p class="hint">完成后这里会显示已登录；CrossAgents 也会在一分钟内变成已登录。</p>
</body></html>"#,
        url = escape(url)
    )
}

const DONE: &str = "<!doctype html><html lang=\"zh\"><head><meta charset=\"utf-8\">\
<title>已登录</title><style>body{font:15px/1.6 -apple-system,'Segoe UI',sans-serif;\
margin:0;padding:32px;color:#1c1c1e;background:#fafafa}.done{font-size:17px}</style></head>\
<body><p class=\"done\">Antigravity 已登录，可以关掉这个页面了。</p></body></html>";

const WAITING: &str = "<!doctype html><html lang=\"zh\"><head><meta charset=\"utf-8\">\
<title>正在登录</title><style>body{font:15px/1.6 -apple-system,'Segoe UI',sans-serif;\
margin:0;padding:32px;color:#1c1c1e;background:#fafafa}</style></head>\
<body><p>CLI 还在处理，稍等一会儿再刷新这个页面。</p></body></html>";

const NOT_FOUND: &str = "<!doctype html><p>not found</p>";

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for `agy acp --login`: prints a link, then takes a code on stdin. `good`
    /// signs in; anything else is refused the way the CLI refuses it, and a new process
    /// prints a new link.
    const FAKE: &str = r#"
import { createInterface } from "node:readline";
const send = (s) => process.stdout.write(s + "\n");
send("Open this link in a browser to log in with Google:");
send("");
send("  https://accounts.google.com/o/oauth2/auth?state=test&code_challenge=x");
send("");
send("Then paste the authorization code, or the full URL of the page you land on, and press Enter:");
createInterface({ input: process.stdin }).on("line", (line) => {
  if (line.includes("good")) {
    send("Logged in as zhang@gmail.com");
    process.exit(0);
  }
  send('couldn\'t log in: token exchange failed: oauth2: "invalid_grant" "Malformed auth code."');
  process.exit(1);
});
"#;

    fn fake_cli() -> (Vec<String>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-agy.mjs");
        std::fs::write(&script, FAKE).unwrap();
        (vec!["node".into(), script.display().to_string()], dir)
    }

    fn free_port() -> u16 {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.local_addr().unwrap().port()
    }

    /// One request to the page, answered as text.
    async fn call(port: u16, method: &str, path: &str, body: &str) -> String {
        let mut socket = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        let request = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: \
             application/x-www-form-urlencoded\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(request.as_bytes()).await.unwrap();
        let mut answer = String::new();
        socket.read_to_string(&mut answer).await.unwrap();
        answer
    }

    #[tokio::test]
    async fn the_page_shows_the_link_and_a_good_code_finishes() {
        let (command, _dir) = fake_cli();
        let port = free_port();
        let server = tokio::spawn(async move { login(&command, port).await });
        // The link is there once the CLI has printed it; the listener answers meanwhile.
        let mut page = String::new();
        for _ in 0..50 {
            page = call(port, "GET", "/", "").await;
            if page.contains("https://accounts.google.com") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(page.contains("https://accounts.google.com"), "{page}");
        assert!(page.contains("完成登录"), "{page}");

        let answer = call(port, "POST", "/code", "code=good").await;
        assert!(answer.contains("200 OK"), "{answer}");
        assert!(answer.contains("已登录"), "{answer}");
        server.await.unwrap().expect("signed in");
    }

    #[tokio::test]
    async fn a_refused_code_says_why_and_offers_a_new_link() {
        let (command, _dir) = fake_cli();
        let port = free_port();
        let server = tokio::spawn(async move { login(&command, port).await });
        for _ in 0..50 {
            if call(port, "GET", "/", "")
                .await
                .contains("accounts.google.com")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let answer = call(port, "POST", "/code", "code=nope").await;
        assert!(answer.contains("token exchange failed"), "{answer}");
        // The CLI is gone, so the page carries the link of the process started after it.
        assert!(answer.contains("accounts.google.com"), "{answer}");

        let answer = call(port, "POST", "/code", "code=good").await;
        assert!(answer.contains("已登录"), "{answer}");
        server.await.unwrap().expect("signed in");
    }

    #[tokio::test]
    async fn a_pasted_callback_url_is_handed_over_as_it_is() {
        let (command, _dir) = fake_cli();
        let port = free_port();
        let server = tokio::spawn(async move { login(&command, port).await });
        for _ in 0..50 {
            if call(port, "GET", "/", "")
                .await
                .contains("accounts.google.com")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        // The CLI accepts the whole URL; the helper passes it through untouched.
        let body =
            "code=https%3A%2F%2Fantigravity.google%2Foauth-callback%3Fcode%3Dgood%26state%3Dtest";
        let answer = call(port, "POST", "/code", body).await;
        assert!(answer.contains("已登录"), "{answer}");
        server.await.unwrap().expect("signed in");
    }

    #[test]
    fn a_link_is_taken_out_of_the_clis_line() {
        assert_eq!(
            find_url("  https://accounts.google.com/o/oauth2/auth?x=1).").as_deref(),
            Some("https://accounts.google.com/o/oauth2/auth?x=1")
        );
        assert_eq!(find_url("Then paste the authorization code"), None);
    }

    #[test]
    fn a_form_value_is_decoded() {
        assert_eq!(form_value(b"code=4%2F0A+a%2Bb", "code"), "4/0A a+b");
        assert_eq!(form_value(b"other=x", "code"), "");
    }
}
