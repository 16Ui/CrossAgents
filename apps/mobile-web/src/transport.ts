import type { InvitePreview, PairingRecord } from "./types";
import { WebRtcTransport, type WebRtcInfo } from "./webrtc";
import * as wasmBindings from "./wasm-pkg/crossagents_link_wasm.js";
import wasmAssetUrl from "./wasm-pkg/crossagents_link_wasm_bg.wasm?url";

export type { WebRtcInfo };

export type ActiveLinkKind = "p2p" | "relay" | "direct";

export interface LinkStatus {
  kind: ActiveLinkKind;
  p2pState: "idle" | "connecting" | "active" | "fallback";
  p2pInfo?: WebRtcInfo;
  p2pNote?: string;
}

export interface HttpReply {
  status: number;
  body: string;
}

export interface EventHandle {
  cancel(): void;
}

export class TransportError extends Error {
  code: "relay_unreachable" | "hub_offline" | "unauthorized" | "http" | "transport" | "bad_config";
  status: number;

  constructor(
    code: TransportError["code"],
    message: string,
    status = 0,
  ) {
    super(message);
    this.code = code;
    this.status = status;
  }
}

export interface HubTransport {
  readonly mode: "iroh" | "direct";
  readonly connectedRelay?: string;
  readonly linkStatus?: LinkStatus;
  onLinkStatusChange?(cb: (status: LinkStatus) => void): () => void;
  retryP2p?(): Promise<void>;
  pingP2p?(timeoutMs?: number): Promise<number | null>;
  /** Fast post-background/screen-unlock liveness probe so dead iOS sockets fail in <1-3.5s
   *  instead of stalling requests for 35s. */
  wakeCheck?(): Promise<"alive" | "dead">;
  request(method: "GET" | "POST" | "DELETE", path: string, body?: unknown): Promise<HttpReply>;
  subscribeEvents(
    since: number | undefined,
    onEvent: (rawJson: string) => void,
    onError: (err: TransportError) => void,
  ): Promise<EventHandle>;
  isClosed(): boolean;
  close(): Promise<void>;
}

export interface RelayDiagItem {
  url: string;
  host: string;
  httpOk: boolean;
  httpMs: number;
  httpDetail: string;
  wsOk: boolean;
  wsMs: number;
  wsDetail: string;
}

const DEFAULT_N0_RELAYS = [
  "https://usw1-1.relay.n0.iroh.link/",
  "https://aps1-1.relay.n0.iroh.link/",
  "https://euc1-1.relay.n0.iroh.link/",
];

function sanitizeRelayUrlJs(raw: string): string {
  try {
    const u = new URL(raw.trim());
    if (u.hostname.endsWith(".")) {
      u.hostname = u.hostname.replace(/\.+$/, "");
    }
    if (!u.pathname.endsWith("/")) {
      u.pathname = `${u.pathname}/`;
    }
    return u.toString();
  } catch {
    return raw.trim();
  }
}

export async function runRelayDiagnostics(preferredRelay?: string | null): Promise<RelayDiagItem[]> {
  const urls: string[] = [];
  if (preferredRelay && preferredRelay.trim()) {
    urls.push(sanitizeRelayUrlJs(preferredRelay));
  }
  for (const fallback of DEFAULT_N0_RELAYS) {
    if (!urls.includes(fallback)) urls.push(fallback);
  }

  async function probeOne(relayUrl: string): Promise<RelayDiagItem> {
    let host = relayUrl;
    try {
      host = new URL(relayUrl).host;
    } catch {
      // ignore
    }

    // 1. HTTPS /ping probe
    let httpOk = false;
    let httpMs = 0;
    let httpDetail = "";
    const httpStart = performance.now();
    try {
      const ctrl = new AbortController();
      const timer = window.setTimeout(() => ctrl.abort(), 6000);
      const res = await fetch(new URL("ping", relayUrl).toString(), { signal: ctrl.signal });
      window.clearTimeout(timer);
      httpMs = Math.round(performance.now() - httpStart);
      httpOk = res.ok;
      httpDetail = `HTTP ${res.status}`;
    } catch (err) {
      httpMs = Math.round(performance.now() - httpStart);
      httpDetail = err instanceof Error ? err.message : String(err);
    }

    // 2. WebSocket wss://.../relay probe with iroh subprotocols
    const wsStart = performance.now();
    const wsRes = await new Promise<{ ok: boolean; detail: string }>((resolve) => {
      let settled = false;
      const done = (ok: boolean, detail: string) => {
        if (settled) return;
        settled = true;
        window.clearTimeout(timer);
        resolve({ ok, detail });
      };
      let ws: WebSocket;
      try {
        const wsUrl = new URL("relay", relayUrl);
        wsUrl.protocol = wsUrl.protocol === "http:" ? "ws:" : "wss:";
        ws = new WebSocket(wsUrl.toString(), ["iroh-relay-v2", "iroh-relay-v1"]);
      } catch (err) {
        done(false, err instanceof Error ? err.message : String(err));
        return;
      }
      const timer = window.setTimeout(() => {
        try {
          ws.close();
        } catch {
          // ignore
        }
        done(false, "WebSocket 6s 超时");
      }, 6000);
      ws.onopen = () => {
        const proto = ws.protocol || "(无子协议)";
        try {
          ws.close(1000, "diag");
        } catch {
          // ignore
        }
        done(true, proto);
      };
      ws.onerror = () => {
        // Wait briefly for onclose to report close code if available.
      };
      ws.onclose = (ev) => {
        done(false, `关闭 code=${ev.code}${ev.reason ? ` (${ev.reason})` : ""}`);
      };
    });
    const wsMs = Math.round(performance.now() - wsStart);

    return {
      url: relayUrl,
      host,
      httpOk,
      httpMs,
      httpDetail,
      wsOk: wsRes.ok,
      wsMs,
      wsDetail: wsRes.detail,
    };
  }

  return Promise.all(urls.map(probeOne));
}

// ---------------------------------------------------------------------------
// Direct page-context loader for `crossagents-link-wasm`:
// - Bundled JS bindings avoid Safari dynamic-import failure caching.
// - `.wasm` binary bypasses Service Worker `FetchEvent.respondWith` completely (preventing iOS
//   Safari's ~30s SW fetch timeout / `res.clone()` tee bug -> "FetchEvent.respondWith received
//   an error: TypeError: Load failed"), and instead downloads directly with progress + 3x retry
//   and persists the completed bytes in CacheStorage (`crossagents-mobile-assets-v1`).

const WASM_CACHE_NAME = "crossagents-mobile-assets-v1";

interface WasmLinkModule {
  default: (moduleOrPath?: unknown) => Promise<unknown>;
  decode_invite(code: string): string;
  redeem_invite(code: string, deviceName: string, os: string): Promise<string>;
  WasmLinkSession: {
    connect(configJson: string): Promise<WasmLinkSessionInstance>;
  };
}

interface WasmLinkSessionInstance {
  connected_relay(): string;
  request(method: string, path: string, body?: string): Promise<string>;
  subscribe_events(
    since: number | undefined,
    onEvent: (json: string) => void,
    onError: (errJson: string) => void,
  ): Promise<{ cancel(): void }>;
  is_closed(): boolean;
  close(): Promise<void>;
}

let wasmPromise: Promise<WasmLinkModule> | null = null;
let wasmReady = false;
const wasmStageListeners = new Set<(detail: string) => void>();

function notifyWasmStage(detail: string) {
  for (const cb of wasmStageListeners) {
    try {
      cb(detail);
    } catch {
      // ignore listener errors
    }
  }
}

async function fetchWasmBytesDirect(): Promise<Uint8Array> {
  const absUrl = new URL(wasmAssetUrl, window.location.href).toString();

  // 1. Check CacheStorage directly first (instant ~45ms cold-start once cached).
  if ("caches" in window) {
    try {
      const cache = await caches.open(WASM_CACHE_NAME);
      const cached = await cache.match(absUrl);
      if (cached && cached.ok) {
        const buf = await cached.arrayBuffer();
        if (buf.byteLength > 100_000) {
          return new Uint8Array(buf);
        }
      }
    } catch {
      // ignore cache read errors
    }
  }

  // 2. Download directly in page context with live progress & automatic retries.
  let lastErr: unknown;
  const maxAttempts = 3;
  for (let attempt = 1; attempt <= maxAttempts; attempt++) {
    try {
      notifyWasmStage(
        attempt === 1
          ? "正在下载加密内核（首次更新后将本地缓存）…"
          : `正在重试下载加密内核（第 ${attempt}/${maxAttempts} 次）…`,
      );
      const res = await fetch(absUrl, {
        cache: attempt === 1 ? "default" : "no-store",
      });
      if (!res.ok) {
        throw new Error(`HTTP ${res.status} ${res.statusText}`);
      }

      let bytes: Uint8Array;
      if (res.body && typeof res.body.getReader === "function") {
        const reader = res.body.getReader();
        const chunks: Uint8Array[] = [];
        let received = 0;
        const contentEncoding = res.headers.get("content-encoding");
        const clHeader = Number(res.headers.get("content-length") || "0");
        const exactTotal = !contentEncoding && clHeader > 0 ? clHeader : 0;
        let lastUi = 0;
        while (true) {
          const { done, value } = await reader.read();
          if (done) break;
          if (value && value.byteLength > 0) {
            chunks.push(value);
            received += value.byteLength;
            const now = performance.now();
            if (now - lastUi >= 250) {
              lastUi = now;
              if (exactTotal > 0 && received <= exactTotal) {
                const pct = Math.min(99, Math.round((received / exactTotal) * 100));
                notifyWasmStage(`正在下载加密内核 ${pct}%…`);
              } else {
                notifyWasmStage(`正在下载加密内核（已收 ${(received / 1024 / 1024).toFixed(1)} MB）…`);
              }
            }
          }
        }
        bytes = new Uint8Array(received);
        let offset = 0;
        for (const chunk of chunks) {
          bytes.set(chunk, offset);
          offset += chunk.byteLength;
        }
      } else {
        bytes = new Uint8Array(await res.arrayBuffer());
      }

      if (bytes.byteLength < 100_000) {
        throw new Error(`加密内核文件不完整（仅 ${bytes.byteLength} 字节）`);
      }

      // 3. Persist completed bytes in CacheStorage and prune superseded `.wasm` files.
      if ("caches" in window) {
        try {
          const cache = await caches.open(WASM_CACHE_NAME);
          const keys = await cache.keys();
          await Promise.all(
            keys
              .filter((k) => k.url.endsWith(".wasm") && k.url !== absUrl)
              .map((k) => cache.delete(k)),
          );
          await cache.put(
            absUrl,
            new Response(bytes.slice().buffer, {
              status: 200,
              headers: { "Content-Type": "application/wasm" },
            }),
          );
        } catch {
          // ignore storage quota errors; in-memory instantiation still works
        }
      }

      return bytes;
    } catch (err) {
      lastErr = err;
      if (attempt < maxAttempts) {
        await new Promise((r) => window.setTimeout(r, 800 * attempt));
      }
    }
  }
  throw lastErr;
}

async function loadWasm(onStage?: (detail: string) => void): Promise<WasmLinkModule> {
  if (wasmReady) {
    return wasmBindings as unknown as WasmLinkModule;
  }
  if (onStage) wasmStageListeners.add(onStage);
  try {
    if (!wasmPromise) {
      wasmPromise = (async () => {
        const mod = wasmBindings as unknown as WasmLinkModule;
        const bytes = await fetchWasmBytesDirect();
        notifyWasmStage("正在初始化加密内核…");
        await mod.default({ module_or_path: bytes });
        wasmReady = true;
        return mod;
      })().catch((err) => {
        wasmPromise = null;
        const msg = err instanceof Error ? err.message : String(err);
        throw new TransportError(
          "relay_unreachable",
          `加密内核下载失败（GitHub Pages 网络中断或过慢，可稍等重试或临时开代理缓存一次）：${msg}`,
        );
      });
    }
    return await wasmPromise;
  } finally {
    if (onStage) wasmStageListeners.delete(onStage);
  }
}

function parseWasmError(err: unknown): TransportError {
  const raw = err instanceof Error ? err.message : String(err);
  try {
    const parsed = JSON.parse(raw) as { code?: string; message?: string; status?: number };
    const code = parsed.code;
    const message = parsed.message || raw;
    if (code === "unauthorized") return new TransportError("unauthorized", message, 401);
    if (code === "relay_unreachable") return new TransportError("relay_unreachable", message);
    if (code === "hub_offline") return new TransportError("hub_offline", message);
    if (code === "bad_config") return new TransportError("bad_config", message);
    return new TransportError("transport", message, parsed.status ?? 0);
  } catch {
    return new TransportError("transport", raw);
  }
}

export function detectClientOs(): string {
  const ua = navigator.userAgent || "";
  if (/iPhone|iPad|iPod/i.test(ua)) return "ios";
  if (/Android/i.test(ua)) return "android";
  if (/Mac/i.test(ua)) return "macos";
  if (/Windows/i.test(ua)) return "windows";
  return "ios";
}

export async function decodeInviteLocal(code: string): Promise<InvitePreview> {
  const trimmed = code.trim();
  if (!trimmed) return { ok: false, error: "请输入或扫描配对码" };
  try {
    const mod = await loadWasm();
    return JSON.parse(mod.decode_invite(trimmed)) as InvitePreview;
  } catch {
    // Pure JS fallback preview in case WASM is still loading.
    const cleaned = trimmed.replace(/\s+/g, "");
    if (!cleaned.startsWith("ca1-")) {
      return { ok: false, error: "不是有效的 CrossAgents 配对码（应以 ca1- 开头）" };
    }
    try {
      const b64 = cleaned.slice(4).replace(/-/g, "+").replace(/_/g, "/");
      const pad = b64.length % 4 === 0 ? "" : "=".repeat(4 - (b64.length % 4));
      const json = JSON.parse(atob(b64 + pad)) as {
        e?: string;
        r?: string;
        c?: boolean;
        a?: string[];
        h?: string;
        x?: number;
      };
      if (!json.e || !json.x) return { ok: false, error: "配对码结构不完整" };
      return {
        ok: true,
        host: json.h || "CrossAgents 中心",
        expires: json.x,
        has_relay: Boolean(json.r),
        relay: json.r ? sanitizeRelayUrlJs(json.r) : null,
        custom_relay: Boolean(json.c),
        lan_addrs: json.a?.length ?? 0,
      };
    } catch {
      return { ok: false, error: "配对码内容损坏" };
    }
  }
}

export async function redeemInviteWasm(code: string, deviceName: string): Promise<PairingRecord> {
  const mod = await loadWasm();
  const os = detectClientOs();
  const raw = await mod.redeem_invite(code.trim(), deviceName.trim() || "iPhone", os);
  const parsed = JSON.parse(raw) as {
    ok: boolean;
    error?: string;
    paired?: {
      device: { id: string; name: string; level?: string };
      token: string;
      host: string;
      endpoint: string;
      relay?: string | null;
      custom_relay?: boolean;
      addrs?: string[];
    };
  };
  if (!parsed.ok || !parsed.paired) {
    throw new TransportError("transport", parsed.error || "配对失败，请重试");
  }
  const p = parsed.paired;
  return {
    mode: "iroh",
    endpoint: p.endpoint,
    relay: p.relay ?? null,
    customRelay: Boolean(p.custom_relay),
    addrs: p.addrs ?? [],
    host: p.host || "CrossAgents 中心",
    deviceId: p.device.id,
    deviceName: p.device.name,
    token: p.token,
    level: p.device.level,
    pairedAt: new Date().toISOString(),
  };
}

// ---------------------------------------------------------------------------
// Primary transport: Browser WASM iroh (E2EE over relay, zero VPN on both ends)

export class IrohWasmTransport implements HubTransport {
  readonly mode = "iroh" as const;
  readonly connectedRelay: string;
  private session: WasmLinkSessionInstance;

  private constructor(session: WasmLinkSessionInstance) {
    this.session = session;
    this.connectedRelay = session.connected_relay();
  }

  static async connect(
    record: PairingRecord,
    onStage?: (detail: string) => void,
  ): Promise<IrohWasmTransport> {
    if (!record.endpoint) {
      throw new TransportError("bad_config", "缺少中心公钥配置");
    }
    const mod = await loadWasm(onStage);
    onStage?.("正在经加密中继连接中心…");
    const cfg = JSON.stringify({
      endpoint: record.endpoint,
      relay: record.relay ?? null,
      custom_relay: Boolean(record.customRelay),
      token: record.token,
      os: detectClientOs(),
    });
    try {
      const session = await mod.WasmLinkSession.connect(cfg);
      return new IrohWasmTransport(session);
    } catch (err) {
      throw parseWasmError(err);
    }
  }

  async request(method: "GET" | "POST" | "DELETE", path: string, body?: unknown): Promise<HttpReply> {
    try {
      const jsonBody = body === undefined ? undefined : JSON.stringify(body);
      const raw = await this.session.request(method, path, jsonBody);
      return JSON.parse(raw) as HttpReply;
    } catch (err) {
      throw parseWasmError(err);
    }
  }

  async subscribeEvents(
    since: number | undefined,
    onEvent: (rawJson: string) => void,
    onError: (err: TransportError) => void,
  ): Promise<EventHandle> {
    try {
      const sub = await this.session.subscribe_events(
        since,
        onEvent,
        (errJson) => onError(parseWasmError(errJson)),
      );
      return {
        cancel: () => {
          try {
            sub.cancel();
          } catch {
            // ignore double-cancel
          }
        },
      };
    } catch (err) {
      throw parseWasmError(err);
    }
  }

  isClosed(): boolean {
    try {
      return this.session.is_closed();
    } catch {
      return true;
    }
  }

  async close(): Promise<void> {
    try {
      await this.session.close();
    } catch {
      // ignore close errors
    }
  }
}

// ---------------------------------------------------------------------------
// Fallback transport: Direct HTTPS (Tailscale Funnel / Cloudflare Tunnel)

export class DirectHttpTransport implements HubTransport {
  readonly mode = "direct" as const;
  private baseUrl: string;
  private token: string;
  private closed = false;

  constructor(record: PairingRecord) {
    if (!record.baseUrl) {
      throw new TransportError("bad_config", "缺少直连 HTTPS 地址");
    }
    this.baseUrl = record.baseUrl.replace(/\/+$/, "");
    this.token = record.token;
  }

  async request(method: "GET" | "POST" | "DELETE", path: string, body?: unknown): Promise<HttpReply> {
    let res: Response;
    try {
      res = await fetch(`${this.baseUrl}${path}`, {
        method,
        headers: {
          Authorization: `Bearer ${this.token}`,
          "x-crossagents-client-os": detectClientOs(),
          ...(body === undefined ? {} : { "Content-Type": "application/json" }),
        },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
    } catch (err) {
      throw new TransportError("hub_offline", `无法访问中心地址：${err instanceof Error ? err.message : String(err)}`);
    }
    const text = await res.text();
    if (res.status === 401) {
      throw new TransportError("unauthorized", "设备授权已失效或被中心吊销", 401);
    }
    return { status: res.status, body: text };
  }

  async subscribeEvents(
    since: number | undefined,
    onEvent: (rawJson: string) => void,
    onError: (err: TransportError) => void,
  ): Promise<EventHandle> {
    const controller = new AbortController();
    const query = since && since > 0 ? `since=${since}` : "live=true";

    (async () => {
      try {
        const res = await fetch(`${this.baseUrl}/v1/events?${query}`, {
          headers: {
            Authorization: `Bearer ${this.token}`,
            "x-crossagents-client-os": detectClientOs(),
            Accept: "text/event-stream",
          },
          signal: controller.signal,
        });
        if (res.status === 401) {
          onError(new TransportError("unauthorized", "设备授权已失效或被中心吊销", 401));
          return;
        }
        if (!res.ok || !res.body) {
          onError(new TransportError("http", `事件流 HTTP ${res.status}`, res.status));
          return;
        }
        const reader = res.body.getReader();
        const decoder = new TextDecoder();
        let buffer = "";
        while (true) {
          const { done, value } = await reader.read();
          if (done) break;
          buffer += decoder.decode(value, { stream: true });
          let nl = buffer.indexOf("\n");
          while (nl !== -1) {
            const line = buffer.slice(0, nl).replace(/\r$/, "");
            buffer = buffer.slice(nl + 1);
            if (line.startsWith("data:")) {
              const payload = line.slice(5).trimStart();
              if (payload) onEvent(payload);
            }
            nl = buffer.indexOf("\n");
          }
        }
        if (!controller.signal.aborted) {
          onError(new TransportError("transport", "事件流已断开"));
        }
      } catch (err) {
        if (!controller.signal.aborted) {
          onError(new TransportError("hub_offline", err instanceof Error ? err.message : String(err)));
        }
      }
    })();

    return {
      cancel: () => controller.abort(),
    };
  }

  isClosed(): boolean {
    return this.closed;
  }

  async close(): Promise<void> {
    this.closed = true;
  }
}

// ---------------------------------------------------------------------------
// Hybrid transport: instant iroh E2EE + transparent WebRTC P2P upgrade & fallback

interface ManagedSse {
  lastSeq: number | undefined;
  onEvent: (rawJson: string) => void;
  onError: (err: TransportError) => void;
  handle: EventHandle | null;
  usingP2p: boolean;
  cancelled: boolean;
}

export class HybridTransport implements HubTransport {
  readonly mode = "iroh" as const;
  readonly connectedRelay: string;
  private primary: IrohWasmTransport;
  private webrtc: WebRtcTransport | null = null;
  private closed = false;
  private upgrading = false;
  private status: LinkStatus;
  private listeners = new Set<(status: LinkStatus) => void>();
  private activeSse: ManagedSse | null = null;

  constructor(primary: IrohWasmTransport) {
    this.primary = primary;
    this.connectedRelay = primary.connectedRelay;
    this.status = WebRtcTransport.isSupported()
      ? { kind: "relay", p2pState: "connecting", p2pNote: "正在尝试 P2P 直连…" }
      : { kind: "relay", p2pState: "fallback", p2pNote: "当前浏览器不支持 WebRTC" };
    if (WebRtcTransport.isSupported()) {
      void this.upgradeToP2p();
    }
  }

  get linkStatus(): LinkStatus {
    return this.status;
  }

  onLinkStatusChange(cb: (status: LinkStatus) => void): () => void {
    this.listeners.add(cb);
    return () => {
      this.listeners.delete(cb);
    };
  }

  private setLinkStatus(next: LinkStatus) {
    this.status = next;
    for (const cb of this.listeners) {
      try {
        cb(next);
      } catch {
        // ignore listener errors
      }
    }
  }

  async retryP2p(): Promise<void> {
    if (this.closed || this.primary.isClosed()) return;
    if (this.webrtc && !this.webrtc.isClosed()) {
      await this.pingP2p();
      return;
    }
    await this.upgradeToP2p();
  }

  async pingP2p(timeoutMs = 3000): Promise<number | null> {
    const rtc = this.webrtc;
    if (!rtc || rtc.isClosed()) return null;
    try {
      const rttMs = await rtc.ping(timeoutMs);
      rtc.info = { ...rtc.info, rttMs };
      this.setLinkStatus({
        kind: "p2p",
        p2pState: "active",
        p2pInfo: rtc.info,
      });
      return rttMs;
    } catch {
      this.handleP2pDropped("P2P 心跳超时，已切回中继");
      return null;
    }
  }

  async wakeCheck(): Promise<"alive" | "dead"> {
    if (this.closed) return "dead";
    // 1. If P2P looked open before iOS froze JS, probe it with an aggressive 900ms ping first so
    //    we never stall `bootstrap()` for 35s on a zombie DataChannel.
    if (this.webrtc && !this.webrtc.isClosed()) {
      const rtt = await this.pingP2p(900);
      if (rtt !== null) return "alive";
    }
    // 2. P2P was dead or inactive: check whether the underlying iroh QUIC/WS session survived.
    if (this.primary.isClosed()) return "dead";
    try {
      const reply = await Promise.race([
        this.primary.request("GET", "/v1/info"),
        new Promise<never>((_, reject) =>
          window.setTimeout(() => reject(new Error("wake probe timeout")), 3500),
        ),
      ]);
      if (reply.status < 500) {
        // iroh survived! Re-establish P2P in background right away.
        void this.retryP2p();
        return "alive";
      }
      return "dead";
    } catch {
      return "dead";
    }
  }

  private async upgradeToP2p(): Promise<void> {
    if (this.closed || this.upgrading || !WebRtcTransport.isSupported()) return;
    if (this.primary.isClosed()) return;
    this.upgrading = true;
    this.setLinkStatus({
      kind: "relay",
      p2pState: "connecting",
      p2pNote: "正在经加密中继交换直连候选…",
    });

    try {
      const rtc = await WebRtcTransport.connect(this.primary, {
        onDrop: (reason) => this.handleP2pDropped(reason),
        onMetrics: (info) => {
          if (this.webrtc === rtc && !this.closed) {
            this.setLinkStatus({
              kind: "p2p",
              p2pState: "active",
              p2pInfo: info,
            });
          }
        },
      });
      if (this.closed) {
        await rtc.close();
        return;
      }
      this.webrtc = rtc;
      this.setLinkStatus({
        kind: "p2p",
        p2pState: "active",
        p2pInfo: rtc.info,
      });
      // Seamlessly migrate active live SSE onto the low-latency P2P DataChannel.
      this.migrateSseToP2p(rtc);
    } catch (err) {
      if (this.closed) return;
      const msg = err instanceof Error ? err.message : String(err);
      this.setLinkStatus({
        kind: "relay",
        p2pState: "fallback",
        p2pNote: msg,
      });
    } finally {
      this.upgrading = false;
    }
  }

  private handleP2pDropped(reason: string) {
    if (this.closed) return;
    if (!this.webrtc) return;
    this.webrtc = null;
    this.setLinkStatus({
      kind: "relay",
      p2pState: "fallback",
      p2pNote: `${reason}，已自动回退到 iroh 中继`,
    });
    // If SSE was riding P2P, immediately fall back to iroh without losing events.
    if (this.activeSse && !this.activeSse.cancelled && this.activeSse.usingP2p) {
      this.reopenSseOnPrimary(this.activeSse);
    }
  }

  private trackEventSeq(sub: ManagedSse, rawJson: string) {
    try {
      const parsed = JSON.parse(rawJson) as { seq?: number };
      if (typeof parsed.seq === "number" && parsed.seq > (sub.lastSeq ?? 0)) {
        sub.lastSeq = parsed.seq;
      }
    } catch {
      // ignore
    }
    sub.onEvent(rawJson);
  }

  private migrateSseToP2p(rtc: WebRtcTransport) {
    const sub = this.activeSse;
    if (!sub || sub.cancelled || sub.usingP2p) return;
    const prevHandle = sub.handle;
    void rtc
      .subscribeEvents(
        sub.lastSeq,
        (raw) => {
          if (!sub.cancelled) this.trackEventSeq(sub, raw);
        },
        (err) => {
          if (sub.cancelled || this.activeSse !== sub) return;
          if (err.code === "unauthorized") {
            sub.onError(err);
            return;
          }
          // P2P SSE dropped: fall back to primary iroh SSE cleanly.
          if (this.webrtc === rtc) {
            this.handleP2pDropped(err.message);
          } else if (sub.usingP2p) {
            this.reopenSseOnPrimary(sub);
          }
        },
      )
      .then((newHandle) => {
        if (sub.cancelled || this.activeSse !== sub) {
          newHandle.cancel();
          return;
        }
        sub.handle = newHandle;
        sub.usingP2p = true;
        prevHandle?.cancel();
      })
      .catch(() => {
        // Keep existing iroh SSE intact if P2P subscribe failed.
      });
  }

  private reopenSseOnPrimary(sub: ManagedSse) {
    if (sub.cancelled || this.activeSse !== sub) return;
    sub.usingP2p = false;
    sub.handle = null;
    if (this.primary.isClosed()) {
      sub.onError(new TransportError("hub_offline", "实时连接已断开"));
      return;
    }
    void this.primary
      .subscribeEvents(
        sub.lastSeq,
        (raw) => {
          if (!sub.cancelled) this.trackEventSeq(sub, raw);
        },
        (err) => {
          if (!sub.cancelled && this.activeSse === sub && !sub.usingP2p) {
            sub.onError(err);
          }
        },
      )
      .then((handle) => {
        if (sub.cancelled || this.activeSse !== sub) {
          handle.cancel();
          return;
        }
        sub.handle = handle;
      })
      .catch((err) => {
        if (!sub.cancelled && this.activeSse === sub) {
          sub.onError(err instanceof TransportError ? err : new TransportError("transport", String(err)));
        }
      });
  }

  async request(method: "GET" | "POST" | "DELETE", path: string, body?: unknown): Promise<HttpReply> {
    const rtc = this.webrtc;
    if (rtc && !rtc.isClosed()) {
      try {
        return await rtc.request(method, path, body);
      } catch (err) {
        const te = err instanceof TransportError ? err : new TransportError("transport", String(err));
        if (te.code === "unauthorized") throw te;
        this.handleP2pDropped(te.message);
        if (method === "GET" && !this.primary.isClosed()) {
          return this.primary.request(method, path, body);
        }
        throw te;
      }
    }
    return this.primary.request(method, path, body);
  }

  async subscribeEvents(
    since: number | undefined,
    onEvent: (rawJson: string) => void,
    onError: (err: TransportError) => void,
  ): Promise<EventHandle> {
    if (this.activeSse) {
      this.activeSse.cancelled = true;
      this.activeSse.handle?.cancel();
    }
    const sub: ManagedSse = {
      lastSeq: since,
      onEvent,
      onError,
      handle: null,
      usingP2p: false,
      cancelled: false,
    };
    this.activeSse = sub;

    const rtc = this.webrtc;
    if (rtc && !rtc.isClosed()) {
      try {
        sub.handle = await rtc.subscribeEvents(
          since,
          (raw) => {
            if (!sub.cancelled) this.trackEventSeq(sub, raw);
          },
          (err) => {
            if (sub.cancelled || this.activeSse !== sub) return;
            if (err.code === "unauthorized") {
              sub.onError(err);
              return;
            }
            if (this.webrtc === rtc) {
              this.handleP2pDropped(err.message);
            } else if (sub.usingP2p) {
              this.reopenSseOnPrimary(sub);
            }
          },
        );
        sub.usingP2p = true;
      } catch {
        // Fall through to primary iroh transport below.
      }
    }

    if (!sub.handle) {
      sub.handle = await this.primary.subscribeEvents(
        since,
        (raw) => {
          if (!sub.cancelled) this.trackEventSeq(sub, raw);
        },
        (err) => {
          if (!sub.cancelled && this.activeSse === sub && !sub.usingP2p) {
            sub.onError(err);
          }
        },
      );
      sub.usingP2p = false;
    }

    return {
      cancel: () => {
        sub.cancelled = true;
        sub.handle?.cancel();
        sub.handle = null;
        if (this.activeSse === sub) this.activeSse = null;
      },
    };
  }

  isClosed(): boolean {
    if (this.closed) return true;
    const p2pAlive = Boolean(this.webrtc && !this.webrtc.isClosed());
    return !p2pAlive && this.primary.isClosed();
  }

  async close(): Promise<void> {
    this.closed = true;
    if (this.activeSse) {
      this.activeSse.cancelled = true;
      this.activeSse.handle?.cancel();
      this.activeSse = null;
    }
    const rtc = this.webrtc;
    this.webrtc = null;
    await Promise.all([rtc?.close(), this.primary.close()]);
  }
}

export async function connectTransport(
  record: PairingRecord,
  onStage?: (detail: string) => void,
): Promise<HubTransport> {
  if (record.mode === "direct") {
    return new DirectHttpTransport(record);
  }
  const iroh = await IrohWasmTransport.connect(record, onStage);
  return new HybridTransport(iroh);
}
