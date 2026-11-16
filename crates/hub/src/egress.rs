//! Discovering and probing available network exits (direct connection, system proxy, local
//! proxy ports and user-saved proxies) so a person can pick an exit per account or per agent
//! in the GUI.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossagents_api_types::{EgressCandidate, SaveEgressProxyRequest};
use crossagents_core::CoreError;
use crossagents_drivers::process::{candidate_echo, normalize_proxy_url, system_proxy_url};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;

/// Default plain-HTTP trace endpoint that answers with `ip=` and `loc=` (two-letter country).
const DEFAULT_TRACE_URL: &str = "http://www.cloudflare.com/cdn-cgi/trace";

/// Common local proxy ports (Clash / Mihomo / Verge / FlClash / v2rayN / Xray / NekoRay /
/// Hiddify / Privoxy / iOA) probed on `127.0.0.1`.
const LOCAL_PORTS: &[u16] = &[
    7890, 7891, 7892, 7893, 7897, 7898, 7899, 10808, 10809, 1080, 1081, 2080, 2081, 7878, 8080,
    8118, 8888, 12639, 20171, 20172,
];

const PORT_SCAN_TIMEOUT: Duration = Duration::from_millis(120);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SavedProxiesFile {
    #[serde(default)]
    proxies: Vec<SavedProxy>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedProxy {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

fn proxies_file(home: &Path) -> PathBuf {
    home.join("egress-proxies.json")
}

pub fn load_saved(home: &Path) -> Vec<SavedProxy> {
    std::fs::read(proxies_file(home))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<SavedProxiesFile>(&bytes).ok())
        .map(|file| file.proxies)
        .unwrap_or_default()
}

pub fn save_proxy(home: &Path, req: SaveEgressProxyRequest) -> Result<Vec<SavedProxy>, CoreError> {
    let url = normalize_proxy_url(&req.url).ok_or_else(|| {
        CoreError::InvalidName(format!(
            "proxy address {:?} (expected e.g. http://127.0.0.1:7890 or socks5://127.0.0.1:1080)",
            req.url.trim()
        ))
    })?;
    if url == "direct" {
        return Err(CoreError::InvalidName(
            "\"direct\" is built in and does not need to be added".into(),
        ));
    }
    let mut proxies = load_saved(home);
    if req.remove == Some(true) {
        proxies.retain(|entry| !entry.url.eq_ignore_ascii_case(&url));
    } else {
        let label = req
            .label
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        match proxies
            .iter_mut()
            .find(|entry| entry.url.eq_ignore_ascii_case(&url))
        {
            Some(entry) => entry.label = label,
            None => proxies.push(SavedProxy { url, label }),
        }
    }
    let bytes = serde_json::to_vec_pretty(&SavedProxiesFile {
        proxies: proxies.clone(),
    })
    .map_err(|e| CoreError::Storage(format!("cannot serialize egress-proxies.json: {e}")))?;
    crate::paths::write_atomic(&proxies_file(home), &bytes)
        .map_err(|e| CoreError::Storage(format!("cannot write egress-proxies.json: {e}")))?;
    Ok(proxies)
}

/// Discovers and probes all candidate network exits in parallel.
pub async fn discover_candidates(
    home: &Path,
    echo_url: Option<&str>,
    configured_urls: &[String],
) -> Vec<EgressCandidate> {
    let mut seeds: Vec<(String, String, String)> = vec![(
        "direct".to_owned(),
        "直连 (Direct)".to_owned(),
        "direct".to_owned(),
    )];
    let mut push_unique = |url: String, label: String, source: &str| {
        if let Some(existing) = seeds
            .iter_mut()
            .find(|(u, _, _)| u.eq_ignore_ascii_case(&url))
        {
            if source == "custom" && label != url {
                existing.1 = label;
            }
            return;
        }
        seeds.push((url, label, source.to_owned()));
    };

    if let Some(sys) = system_proxy_url().and_then(|u| normalize_proxy_url(&u))
        && sys != "direct"
    {
        push_unique(sys.clone(), format!("系统代理 ({sys})"), "system");
    }

    for key in [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
    ] {
        if let Ok(val) = std::env::var(key)
            && let Some(url) = normalize_proxy_url(&val)
            && url != "direct"
        {
            push_unique(url.clone(), format!("环境变量 ({url})"), "env");
        }
    }

    for saved in load_saved(home) {
        if let Some(url) = normalize_proxy_url(&saved.url)
            && url != "direct"
        {
            let label = match saved.label {
                Some(name) if !name.is_empty() => format!("{name} ({url})"),
                _ => url.clone(),
            };
            push_unique(url, label, "custom");
        }
    }

    for extra in configured_urls {
        if let Some(url) = normalize_proxy_url(extra)
            && url != "direct"
        {
            push_unique(url.clone(), url, "custom");
        }
    }

    // Scan common local proxy ports in parallel.
    let open_ports = futures::future::join_all(LOCAL_PORTS.iter().copied().map(|port| async move {
        let ok = tokio::time::timeout(
            PORT_SCAN_TIMEOUT,
            TcpStream::connect(("127.0.0.1", port)),
        )
        .await
        .is_ok_and(|res| res.is_ok());
        ok.then_some(port)
    }))
    .await;

    for port in open_ports.into_iter().flatten() {
        let url = format!("http://127.0.0.1:{port}");
        push_unique(url, format!("本机 :{port}"), "local");
    }

    let echo = echo_url.map(str::to_owned);
    let probed = futures::future::join_all(seeds.into_iter().map(|(value, label, source)| {
        let echo = echo.clone();
        async move {
            let started = Instant::now();
            let proxy_arg = (value != "direct").then_some(value.as_str());
            let (ip, asn, country) = probe_exit(echo.as_deref(), proxy_arg).await;
            let latency_ms = ip.is_some().then(|| started.elapsed().as_millis() as u64);
            let error = ip
                .is_none()
                .then(|| "无法通过该入口探测到出口 IP".to_owned());
            EgressCandidate {
                value,
                label,
                source,
                ip,
                country,
                asn,
                latency_ms,
                error,
            }
        }
    }))
    .await;

    probed
        .into_iter()
        // Auto-scanned local ports that did not answer as a working proxy are not shown, so
        // unrelated local services (dev servers, daemons) never clutter the list.
        .filter(|c| c.source != "local" || c.ip.is_some())
        .collect()
}

/// Probes a single exit (`None` or `Some("direct")` for direct connection, or `Some(proxy_url)`)
/// and returns `(ip, asn, country)`.
pub async fn probe_exit(
    echo_url: Option<&str>,
    proxy_url: Option<&str>,
) -> (Option<String>, Option<String>, Option<String>) {
    if let Some(url) = echo_url
        && let Some(body) = candidate_echo(url, proxy_url).await
        && let Some(parsed) = parse_echo_body(&body)
    {
        return parsed;
    }
    if let Some(body) = candidate_echo(DEFAULT_TRACE_URL, proxy_url).await
        && let Some(parsed) = parse_echo_body(&body)
    {
        return parsed;
    }
    (None, None, None)
}

/// Parses an echo response in JSON (`{"ip","country","asn"}` or `{"query","countryCode","as"}`),
/// Cloudflare trace (`ip=...\nloc=...`), or plain-text IP format.
pub fn parse_echo_body(body: &str) -> Option<(Option<String>, Option<String>, Option<String>)> {
    if body.len() > 16384 {
        return None;
    }
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(body) {
        let raw_ip = json
            .get("ip")
            .or_else(|| json.get("query"))
            .and_then(|v| v.as_str())?;
        let ip = raw_ip.trim().parse::<IpAddr>().ok()?.to_string();
        let asn = json
            .get("asn")
            .or_else(|| json.get("as"))
            .and_then(|v| v.as_str())
            .and_then(extract_asn);
        let country = json
            .get("country")
            .or_else(|| json.get("countryCode"))
            .and_then(|v| v.as_str())
            .and_then(normalize_country);
        return Some((Some(ip), asn, country));
    }
    // Key-value trace format (`ip=1.2.3.4\nloc=US`).
    if body.contains("\nip=") || body.starts_with("ip=") {
        let mut ip = None;
        let mut country = None;
        for line in body.lines() {
            if let Some((k, v)) = line.split_once('=') {
                match k.trim() {
                    "ip" => ip = v.trim().parse::<IpAddr>().ok().map(|a| a.to_string()),
                    "loc" => country = normalize_country(v.trim()),
                    _ => {}
                }
            }
        }
        if let Some(ip) = ip {
            return Some((Some(ip), None, country));
        }
    }
    let ip = body.trim().parse::<IpAddr>().ok()?.to_string();
    Some((Some(ip), None, None))
}

fn extract_asn(raw: &str) -> Option<String> {
    let token = raw.split_whitespace().next()?;
    let digits = token
        .strip_prefix("AS")
        .or_else(|| token.strip_prefix("as"))
        .unwrap_or(token);
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) && digits.parse::<u32>().is_ok())
        .then(|| format!("AS{digits}"))
}

fn normalize_country(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    (trimmed.len() == 2
        && trimmed.bytes().all(|b| b.is_ascii_alphabetic())
        && !trimmed.eq_ignore_ascii_case("XX"))
    .then(|| trimmed.to_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json_cloudflare_trace_and_plain_ip() {
        assert_eq!(
            parse_echo_body(r#"{"ip":"142.250.1.1","country":"us","asn":"AS15169"}"#),
            Some((
                Some("142.250.1.1".into()),
                Some("AS15169".into()),
                Some("US".into())
            ))
        );
        assert_eq!(
            parse_echo_body("fl=11\nip=43.132.141.23\nloc=HK\ncolo=HKG\n"),
            Some((Some("43.132.141.23".into()), None, Some("HK".into())))
        );
        assert_eq!(
            parse_echo_body("203.0.113.5\n"),
            Some((Some("203.0.113.5".into()), None, None))
        );
        assert_eq!(parse_echo_body("not an ip"), None);
    }

    #[test]
    fn saves_and_removes_custom_proxies() {
        let dir = tempfile::tempdir().unwrap();
        let list = save_proxy(
            dir.path(),
            SaveEgressProxyRequest {
                url: "127.0.0.1:7890".into(),
                label: Some("美区节点".into()),
                remove: None,
            },
        )
        .unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].url, "http://127.0.0.1:7890");
        assert_eq!(list[0].label.as_deref(), Some("美区节点"));

        let list = save_proxy(
            dir.path(),
            SaveEgressProxyRequest {
                url: "http://127.0.0.1:7890".into(),
                label: None,
                remove: Some(true),
            },
        )
        .unwrap();
        assert!(list.is_empty());
    }
}
