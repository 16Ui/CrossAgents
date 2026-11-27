//! Minimal HTTP/1.1 client + chunked SSE stream decoder over an `iroh` QUIC bidirectional stream.
//! Runs directly against the CrossAgents Hub's existing `Hello::Tunnel` byte pipe without requiring
//! any local loopback TCP port in the browser sandbox.

use std::io::Read as _;

use flate2::read::GzDecoder;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt};
use tokio::sync::oneshot;

const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_LINE_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseError {
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for SseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.status > 0 {
            write!(f, "HTTP {}: {}", self.status, self.message)
        } else {
            f.write_str(&self.message)
        }
    }
}

/// Serializes an HTTP/1.1 request carrying CrossAgents Bearer authentication and client OS header.
pub fn build_http1_request(
    method: &str,
    path: &str,
    token: &str,
    client_os: &str,
    body: Option<&str>,
    accept: Option<&str>,
) -> Vec<u8> {
    let mut out = String::with_capacity(256 + body.map_or(0, str::len));
    out.push_str(method);
    out.push(' ');
    out.push_str(path);
    out.push_str(" HTTP/1.1\r\n");
    out.push_str("Host: 127.0.0.1\r\n");
    out.push_str("Authorization: Bearer ");
    out.push_str(token);
    out.push_str("\r\n");
    if !client_os.is_empty() {
        out.push_str("x-crossagents-client-os: ");
        out.push_str(client_os);
        out.push_str("\r\n");
    }
    if let Some(accept) = accept {
        out.push_str("Accept: ");
        out.push_str(accept);
        out.push_str("\r\n");
        out.push_str("Cache-Control: no-cache\r\n");
    } else {
        out.push_str("Accept-Encoding: gzip\r\n");
    }
    out.push_str("Connection: close\r\n");
    if let Some(body) = body {
        out.push_str("Content-Type: application/json\r\n");
        out.push_str(&format!("Content-Length: {}\r\n", body.len()));
        out.push_str("\r\n");
        out.push_str(body);
    } else {
        out.push_str("\r\n");
    }
    out.into_bytes()
}

struct ParsedHeaders {
    status: u16,
    content_length: Option<usize>,
    chunked: bool,
    gzip: bool,
}

async fn read_http_headers<R: AsyncBufRead + Unpin>(
    reader: &mut R,
) -> Result<ParsedHeaders, String> {
    let mut status_line = String::new();
    let n = read_limited_line(reader, &mut status_line, MAX_LINE_BYTES).await?;
    if n == 0 {
        return Err("连接在返回 HTTP 响应头前关闭".into());
    }
    let status = parse_status_code(&status_line)?;
    let mut content_length = None;
    let mut chunked = false;
    let mut gzip = false;
    let mut total_bytes = n;

    loop {
        let mut line = String::new();
        let read = read_limited_line(reader, &mut line, MAX_LINE_BYTES).await?;
        total_bytes += read;
        if total_bytes > MAX_HEADER_BYTES {
            return Err("HTTP 响应头过大".into());
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if read == 0 || trimmed.is_empty() {
            break;
        }
        if let Some((name, val)) = trimmed.split_once(':') {
            let name = name.trim();
            let val = val.trim();
            if name.eq_ignore_ascii_case("content-length") {
                if let Ok(len) = val.parse::<usize>() {
                    content_length = Some(len);
                }
            } else if name.eq_ignore_ascii_case("transfer-encoding")
                && val.to_ascii_lowercase().contains("chunked")
            {
                chunked = true;
            } else if name.eq_ignore_ascii_case("content-encoding")
                && val.to_ascii_lowercase().contains("gzip")
            {
                gzip = true;
            }
        }
    }

    Ok(ParsedHeaders {
        status,
        content_length,
        chunked,
        gzip,
    })
}

fn parse_status_code(line: &str) -> Result<u16, String> {
    let mut parts = line.split_whitespace();
    let proto = parts.next().unwrap_or("");
    if !proto.starts_with("HTTP/") {
        return Err(format!("无效的 HTTP 响应行：{line:?}"));
    }
    let code_str = parts
        .next()
        .ok_or_else(|| format!("缺少 HTTP 状态码：{line:?}"))?;
    code_str
        .parse::<u16>()
        .map_err(|_| format!("无效的 HTTP 状态码：{code_str:?}"))
}

async fn read_limited_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    out: &mut String,
    max: usize,
) -> Result<usize, String> {
    let mut buf = Vec::new();
    let n = (&mut *reader)
        .take(max as u64)
        .read_until(b'\n', &mut buf)
        .await
        .map_err(|e| format!("读取响应失败：{e}"))?;
    *out = String::from_utf8_lossy(&buf).into_owned();
    Ok(n)
}

async fn read_chunked_body<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let Some(size) = read_chunk_size(reader).await? else {
            break;
        };
        if size == 0 {
            // Drain trailing headers until blank line.
            let mut trailer = String::new();
            loop {
                trailer.clear();
                let n = read_limited_line(reader, &mut trailer, MAX_LINE_BYTES).await?;
                if n == 0 || trailer.trim_end_matches(['\r', '\n']).is_empty() {
                    break;
                }
            }
            break;
        }
        let start = out.len();
        out.resize(start + size, 0);
        reader
            .read_exact(&mut out[start..])
            .await
            .map_err(|e| format!("读取 chunked 数据块失败：{e}"))?;
        consume_crlf(reader).await?;
    }
    Ok(out)
}

async fn read_chunk_size<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Option<usize>, String> {
    let mut line = String::new();
    let n = read_limited_line(reader, &mut line, MAX_LINE_BYTES).await?;
    if n == 0 {
        return Ok(None);
    }
    let hex_part = line
        .trim_end_matches(['\r', '\n'])
        .split(';')
        .next()
        .unwrap_or("")
        .trim();
    if hex_part.is_empty() {
        return Ok(None);
    }
    let size = usize::from_str_radix(hex_part, 16)
        .map_err(|_| format!("无效的 chunked 大小：{hex_part:?}"))?;
    Ok(Some(size))
}

async fn consume_crlf<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<(), String> {
    let mut crlf = [0u8; 2];
    reader
        .read_exact(&mut crlf)
        .await
        .map_err(|e| format!("读取 chunked 换行符失败：{e}"))?;
    Ok(())
}

/// Reads a complete HTTP/1.1 response (supports `Content-Length`, `chunked`, and EOF framing)
/// directly from an existing buffered reader so pipelined bytes right after `Hello::Tunnel` reply
/// are consumed without double-buffering or byte loss.
pub async fn read_http1_response<R: AsyncBufRead + Unpin>(
    mut reader: R,
) -> Result<HttpResponse, String> {
    let headers = read_http_headers(&mut reader).await?;

    let bytes = if headers.status == 204 || headers.status == 304 {
        Vec::new()
    } else if headers.chunked {
        read_chunked_body(&mut reader).await?
    } else if let Some(len) = headers.content_length {
        let mut buf = vec![0u8; len];
        reader
            .read_exact(&mut buf)
            .await
            .map_err(|e| format!("读取响应体失败：{e}"))?;
        buf
    } else {
        let mut buf = Vec::new();
        reader
            .read_to_end(&mut buf)
            .await
            .map_err(|e| format!("读取响应体失败：{e}"))?;
        buf
    };

    let bytes = if headers.gzip && !bytes.is_empty() {
        let mut decoder = GzDecoder::new(&bytes[..]);
        let mut out = Vec::new();
        decoder
            .read_to_end(&mut out)
            .map_err(|e| format!("解压 gzip 响应体失败：{e}"))?;
        out
    } else {
        bytes
    };

    let body = String::from_utf8(bytes).map_err(|e| format!("响应体不是有效 UTF-8：{e}"))?;
    Ok(HttpResponse {
        status: headers.status,
        body,
    })
}

/// Streams HTTP/1.1 SSE events (chunked or plain), invoking `on_data` for each `data:` line until
/// cancelled or stream ends. Takes `AsyncBufRead` directly so pipelined bytes after `Hello::Tunnel`
/// reply are preserved cleanly.
pub async fn stream_http1_sse<R, F>(
    mut reader: R,
    mut cancel_rx: oneshot::Receiver<()>,
    mut on_data: F,
) -> Result<(), SseError>
where
    R: AsyncBufRead + Unpin,
    F: FnMut(String),
{
    let headers = read_http_headers(&mut reader).await.map_err(|message| SseError {
        status: 0,
        message,
    })?;

    if !(200..300).contains(&headers.status) {
        let body = if headers.chunked {
            read_chunked_body(&mut reader).await.unwrap_or_default()
        } else if let Some(len) = headers.content_length {
            let mut b = vec![0u8; len.min(64 * 1024)];
            let _ = reader.read_exact(&mut b).await;
            b
        } else {
            let mut b = Vec::new();
            let _ = (&mut reader).take(64 * 1024).read_to_end(&mut b).await;
            b
        };
        let message = String::from_utf8_lossy(&body).trim().to_owned();
        return Err(SseError {
            status: headers.status,
            message: if message.is_empty() {
                format!("事件流连接被拒绝（HTTP {}）", headers.status)
            } else {
                message
            },
        });
    }

    let mut pending = Vec::<u8>::new();
    let flush_lines = |buf: &mut Vec<u8>, cb: &mut F| {
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let mut line_bytes: Vec<u8> = buf.drain(..=pos).collect();
            if line_bytes.last() == Some(&b'\n') {
                line_bytes.pop();
            }
            if line_bytes.last() == Some(&b'\r') {
                line_bytes.pop();
            }
            let line = String::from_utf8_lossy(&line_bytes);
            if let Some(rest) = line.strip_prefix("data:") {
                let payload = rest.trim_start();
                if !payload.is_empty() {
                    cb(payload.to_owned());
                }
            }
        }
    };

    if headers.chunked {
        loop {
            let next_size = tokio::select! {
                _ = &mut cancel_rx => return Ok(()),
                res = read_chunk_size(&mut reader) => res.map_err(|message| SseError { status: 0, message })?,
            };
            let Some(size) = next_size else {
                return Err(SseError {
                    status: 0,
                    message: "事件流已断开".into(),
                });
            };
            if size == 0 {
                return Err(SseError {
                    status: 0,
                    message: "事件流已结束".into(),
                });
            }
            let mut remaining = size;
            let mut scratch = [0u8; 4096];
            while remaining > 0 {
                let want = remaining.min(scratch.len());
                let read = tokio::select! {
                    _ = &mut cancel_rx => return Ok(()),
                    res = reader.read(&mut scratch[..want]) => res.map_err(|e| SseError { status: 0, message: format!("事件流读取失败：{e}") })?,
                };
                if read == 0 {
                    return Err(SseError {
                        status: 0,
                        message: "事件流意外中断".into(),
                    });
                }
                remaining -= read;
                pending.extend_from_slice(&scratch[..read]);
                flush_lines(&mut pending, &mut on_data);
            }
            tokio::select! {
                _ = &mut cancel_rx => return Ok(()),
                res = consume_crlf(&mut reader) => res.map_err(|message| SseError { status: 0, message })?,
            }
        }
    } else {
        let mut scratch = [0u8; 4096];
        loop {
            let read = tokio::select! {
                _ = &mut cancel_rx => return Ok(()),
                res = reader.read(&mut scratch) => res.map_err(|e| SseError { status: 0, message: format!("事件流读取失败：{e}") })?,
            };
            if read == 0 {
                return Err(SseError {
                    status: 0,
                    message: "事件流已断开".into(),
                });
            }
            pending.extend_from_slice(&scratch[..read]);
            flush_lines(&mut pending, &mut on_data);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_http1_requests_with_headers() {
        let req = build_http1_request("GET", "/v1/agents", "cad_abc", "ios", None, None);
        let text = String::from_utf8(req).unwrap();
        assert!(text.starts_with("GET /v1/agents HTTP/1.1\r\n"));
        assert!(text.contains("Authorization: Bearer cad_abc\r\n"));
        assert!(text.contains("x-crossagents-client-os: ios\r\n"));
        assert!(text.ends_with("\r\n\r\n"));

        let post = build_http1_request(
            "POST",
            "/v1/questions/1/answer",
            "cad_abc",
            "ios",
            Some("{\"answer\":\"yes\"}"),
            None,
        );
        let post_text = String::from_utf8(post).unwrap();
        assert!(post_text.contains("Content-Type: application/json\r\n"));
        assert!(post_text.contains("Content-Length: 16\r\n"));
        assert!(post_text.ends_with("\r\n\r\n{\"answer\":\"yes\"}"));
    }

    #[tokio::test]
    async fn reads_content_length_and_chunked_responses() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\n\r\n{\"ok\":true}";
        let res = read_http1_response(&raw[..]).await.unwrap();
        assert_eq!(res.status, 200);
        assert_eq!(res.body, "{\"ok\":true}");

        let chunked = b"HTTP/1.1 201 Created\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6;ext=1\r\n world\r\n0\r\n\r\n";
        let res = read_http1_response(&chunked[..]).await.unwrap();
        assert_eq!(res.status, 201);
        assert_eq!(res.body, "hello world");

        let no_content = b"HTTP/1.1 204 No Content\r\n\r\n";
        let res = read_http1_response(&no_content[..]).await.unwrap();
        assert_eq!(res.status, 204);
        assert_eq!(res.body, "");
    }

    #[tokio::test]
    async fn streams_chunked_sse_events_and_surfaces_401() {
        let frame1 = "id: 1\nevent: human.asked\ndata: {\"seq\":1,\"type\":\"human.asked\"}\n\n";
        let frame2 = ": keep-alive\n\ndata: {\"seq\":2,\"type\":\"agent.output\"}\n\n";
        let sse = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{}\r\n{:x}\r\n{}\r\n0\r\n\r\n",
            frame1.len(),
            frame1,
            frame2.len(),
            frame2,
        );
        let (_tx, rx) = oneshot::channel();
        let mut got = Vec::new();
        let eof_err = stream_http1_sse(sse.as_bytes(), rx, |line| got.push(line))
            .await
            .unwrap_err();
        assert_eq!(eof_err.status, 0);
        assert_eq!(
            got,
            vec![
                "{\"seq\":1,\"type\":\"human.asked\"}",
                "{\"seq\":2,\"type\":\"agent.output\"}",
            ]
        );

        let unauth = b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 14\r\n\r\ndevice revoked";
        let (_tx2, rx2) = oneshot::channel();
        let err = stream_http1_sse(&unauth[..], rx2, |_| {}).await.unwrap_err();
        assert_eq!(err.status, 401);
        assert_eq!(err.message, "device revoked");
    }

    #[tokio::test]
    async fn reads_pipelined_response_after_tunnel_reply_line() {
        // Simulates a single QUIC packet delivering both the `{"ok":true}\n` tunnel reply and
        // the immediately following HTTP/1.1 response into one shared `BufReader`.
        let wire = b"{\"ok\":true}\nHTTP/1.1 200 OK\r\nContent-Length: 16\r\n\r\n{\"agents\":[\"a\"]}";
        let mut reader = tokio::io::BufReader::with_capacity(4096, &wire[..]);
        let mut line = Vec::new();
        reader.read_until(b'\n', &mut line).await.unwrap();
        assert_eq!(line, b"{\"ok\":true}\n");
        let res = read_http1_response(&mut reader).await.unwrap();
        assert_eq!(res.status, 200);
        assert_eq!(res.body, "{\"agents\":[\"a\"]}");
    }

    #[tokio::test]
    async fn decompresses_gzip_http1_responses() {
        use flate2::{Compression, write::GzEncoder};
        use std::io::Write as _;

        let payload = "{\"turns\":[1,2,3],\"text\":\"hello gzip world\"}";
        let mut enc = GzEncoder::new(Vec::new(), Compression::default());
        enc.write_all(payload.as_bytes()).unwrap();
        let gz = enc.finish().unwrap();

        let mut wire = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\n\r\n",
            gz.len()
        )
        .into_bytes();
        wire.extend_from_slice(&gz);

        let res = read_http1_response(&wire[..]).await.unwrap();
        assert_eq!(res.status, 200);
        assert_eq!(res.body, payload);
    }
}
