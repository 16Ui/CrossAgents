import type { InvitePreview, PairingRecord } from "./types";

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
// Lazy loader for the compiled Rust WASM `crossagents-link-wasm` module.

interface WasmLinkModule {
  default: () => Promise<unknown>;
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

async function loadWasm(): Promise<WasmLinkModule> {
  if (!wasmPromise) {
    wasmPromise = (async () => {
      const mod = (await import("./wasm-pkg/crossagents_link_wasm.js")) as unknown as WasmLinkModule;
      await mod.default();
      return mod;
    })().catch((err) => {
      wasmPromise = null;
      throw err;
    });
  }
  return wasmPromise;
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

  static async connect(record: PairingRecord): Promise<IrohWasmTransport> {
    if (!record.endpoint) {
      throw new TransportError("bad_config", "缺少中心公钥配置");
    }
    const mod = await loadWasm();
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

export async function connectTransport(record: PairingRecord): Promise<HubTransport> {
  if (record.mode === "direct") {
    return new DirectHttpTransport(record);
  }
  return IrohWasmTransport.connect(record);
}
