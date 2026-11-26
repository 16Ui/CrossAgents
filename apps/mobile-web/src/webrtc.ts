import { TransportError, type EventHandle, type HubTransport, type HttpReply } from "./transport";

export const WEBRTC_CHANNEL_LABEL = "crossagents-rpc";

const ICE_SERVERS: RTCIceServer[] = [
  { urls: "stun:stun.cloudflare.com:3478" },
  { urls: "stun:stun.nextcloud.com:443" },
  { urls: "stun:stun.hot-chilli.net:3478" },
];

const ICE_GATHER_HARD_TIMEOUT_MS = 2600;
const ICE_GATHER_SETTLE_MS = 650;
const CHANNEL_OPEN_TIMEOUT_MS = 7000;
const REQUEST_TIMEOUT_MS = 35_000;
const KEEPALIVE_INTERVAL_MS = 25_000;
const KEEPALIVE_TIMEOUT_MS = 4500;
const OUTBOUND_CHUNK_CHARS = 12_000;
const OUTBOUND_HIGH_WATER = 512 * 1024;
const OUTBOUND_LOW_WATER = 64 * 1024;

export interface WebRtcInfo {
  rttMs: number;
  localType: string;
  remoteType: string;
  ipFamily: "IPv6" | "IPv4" | "unknown";
  hubCandidates: string[];
  connectedAt: number;
}

interface PendingReq {
  resolve: (reply: HttpReply) => void;
  reject: (err: TransportError) => void;
  timer: number;
  status?: number;
  chunks?: string[];
}

interface ActiveSse {
  onEvent: (rawJson: string) => void;
  onError: (err: TransportError) => void;
  chunks?: string[];
}

interface OfferReply {
  sdp: string;
  candidates?: string[];
  remote_candidates?: string[];
}

type ServerFrame =
  | { t: "pong"; n: number }
  | { t: "res"; id: number; s: number; b: string }
  | { t: "res_s"; id: number; s: number }
  | { t: "res_c"; id: number; d: string }
  | { t: "res_e"; id: number }
  | { t: "err"; id: number; c: string; m: string }
  | { t: "sse_data"; id: number; d: string }
  | { t: "sse_ds"; id: number }
  | { t: "sse_dc"; id: number; d: string }
  | { t: "sse_de"; id: number }
  | { t: "sse_err"; id: number; c: string; s: number; m: string };

function mapErrorCode(code: string): TransportError["code"] {
  if (code === "unauthorized") return "unauthorized";
  if (code === "hub_offline") return "hub_offline";
  if (code === "relay_unreachable") return "relay_unreachable";
  if (code === "http") return "http";
  if (code === "bad_config") return "bad_config";
  return "transport";
}

function waitForIceGathering(pc: RTCPeerConnection): Promise<void> {
  if (pc.iceGatheringState === "complete") return Promise.resolve();

  return new Promise((resolve) => {
    let settled = false;
    let settleTimer: number | null = null;
    let hasV4 = false;
    let hasV6 = false;
    const hardTimer = window.setTimeout(finish, ICE_GATHER_HARD_TIMEOUT_MS);

    function finish() {
      if (settled) return;
      settled = true;
      window.clearTimeout(hardTimer);
      if (settleTimer !== null) window.clearTimeout(settleTimer);
      pc.removeEventListener("icegatheringstatechange", onState);
      pc.removeEventListener("icecandidate", onCand);
      resolve();
    }

    function onState() {
      if (pc.iceGatheringState === "complete") finish();
    }

    function onCand(ev: RTCPeerConnectionIceEvent) {
      if (!ev.candidate) {
        finish();
        return;
      }
      const line = ev.candidate.candidate || "";
      const parts = line.split(/\s+/);
      const addr = parts[4] || "";
      const isNonMdns = Boolean(addr) && !addr.endsWith(".local") && !addr.startsWith("fe80:");
      if (isNonMdns) {
        if (addr.includes(":")) hasV6 = true;
        else if (addr.includes(".")) hasV4 = true;
        if (hasV4 && hasV6) {
          // Got both IPv4 and IPv6 routable candidates — settle right away.
          if (settleTimer !== null) window.clearTimeout(settleTimer);
          settleTimer = window.setTimeout(finish, 180);
        } else if (settleTimer === null) {
          settleTimer = window.setTimeout(finish, ICE_GATHER_SETTLE_MS);
        }
      }
    }

    pc.addEventListener("icegatheringstatechange", onState);
    pc.addEventListener("icecandidate", onCand);
  });
}

async function inspectCandidatePair(
  pc: RTCPeerConnection,
  fallbackRttMs: number,
  hubCandidates: string[],
): Promise<WebRtcInfo> {
  let localType = "unknown";
  let remoteType = "unknown";
  let ipFamily: WebRtcInfo["ipFamily"] = "unknown";
  let rttMs = fallbackRttMs;

  try {
    const stats = await pc.getStats();
    let chosenPair: Record<string, unknown> | undefined;
    const localMap = new Map<string, Record<string, unknown>>();
    const remoteMap = new Map<string, Record<string, unknown>>();

    stats.forEach((report) => {
      const rec = report as unknown as Record<string, unknown>;
      if (rec.type === "local-candidate" && typeof rec.id === "string") {
        localMap.set(rec.id, rec);
      } else if (rec.type === "remote-candidate" && typeof rec.id === "string") {
        remoteMap.set(rec.id, rec);
      } else if (rec.type === "candidate-pair") {
        if (rec.nominated === true || rec.selected === true || rec.state === "succeeded") {
          if (!chosenPair || rec.nominated === true || rec.selected === true) {
            chosenPair = rec;
          }
        }
      }
    });

    if (chosenPair) {
      const local = typeof chosenPair.localCandidateId === "string"
        ? localMap.get(chosenPair.localCandidateId)
        : undefined;
      const remote = typeof chosenPair.remoteCandidateId === "string"
        ? remoteMap.get(chosenPair.remoteCandidateId)
        : undefined;
      if (local && typeof local.candidateType === "string") localType = local.candidateType;
      if (remote && typeof remote.candidateType === "string") remoteType = remote.candidateType;
      const addr = String(remote?.address ?? remote?.ip ?? local?.address ?? local?.ip ?? "");
      if (addr.includes(":")) ipFamily = "IPv6";
      else if (addr.includes(".")) ipFamily = "IPv4";
      if (rttMs <= 0 && typeof chosenPair.currentRoundTripTime === "number") {
        rttMs = Math.max(1, Math.round(chosenPair.currentRoundTripTime * 1000));
      }
    }
  } catch {
    // Some WebKit builds limit getStats fields; keep ping-measured RTT.
  }

  return {
    rttMs,
    localType,
    remoteType,
    ipFamily,
    hubCandidates,
    connectedAt: Date.now(),
  };
}

export class WebRtcTransport implements HubTransport {
  readonly mode = "iroh" as const;
  private pc: RTCPeerConnection;
  private dc: RTCDataChannel;
  private closed = false;
  private nextId = 1;
  private nextPing = 1;
  private pending = new Map<number, PendingReq>();
  private pings = new Map<number, { sentAt: number; resolve: (ms: number) => void; reject: (err: Error) => void; timer: number }>();
  private sseListeners = new Map<number, ActiveSse>();
  private keepaliveTimer: number | null = null;
  private onDrop?: (reason: string) => void;
  private onMetrics?: (info: WebRtcInfo) => void;
  info: WebRtcInfo;

  private constructor(
    pc: RTCPeerConnection,
    dc: RTCDataChannel,
    info: WebRtcInfo,
    onDrop?: (reason: string) => void,
    onMetrics?: (info: WebRtcInfo) => void,
  ) {
    this.pc = pc;
    this.dc = dc;
    this.info = info;
    this.onDrop = onDrop;
    this.onMetrics = onMetrics;

    this.dc.bufferedAmountLowThreshold = OUTBOUND_LOW_WATER;
    this.dc.onmessage = (ev) => {
      if (typeof ev.data === "string") {
        this.handleMessage(ev.data);
      }
    };
    this.dc.onclose = () => this.markDropped("P2P 数据通道已关闭");
    this.dc.onerror = () => this.markDropped("P2P 数据通道出错");
    this.pc.oniceconnectionstatechange = () => {
      const s = this.pc.iceConnectionState;
      if (s === "failed" || s === "closed") {
        this.markDropped(`ICE 连接断开 (${s})`);
      }
    };
    this.pc.onconnectionstatechange = () => {
      const s = this.pc.connectionState;
      if (s === "failed" || s === "closed") {
        this.markDropped(`WebRTC 连接断开 (${s})`);
      }
    };

    this.keepaliveTimer = window.setInterval(() => {
      if (this.closed) return;
      void this.ping(KEEPALIVE_TIMEOUT_MS)
        .then((rttMs) => {
          this.info = { ...this.info, rttMs };
          this.onMetrics?.(this.info);
        })
        .catch(() => {
          this.markDropped("P2P 心跳超时");
        });
    }, KEEPALIVE_INTERVAL_MS);
  }

  static isSupported(): boolean {
    return typeof RTCPeerConnection !== "undefined";
  }

  static async connect(
    signaling: HubTransport,
    opts?: {
      onDrop?: (reason: string) => void;
      onMetrics?: (info: WebRtcInfo) => void;
    },
  ): Promise<WebRtcTransport> {
    if (!WebRtcTransport.isSupported()) {
      throw new TransportError("transport", "当前浏览器不支持 WebRTC");
    }

    const pc = new RTCPeerConnection({ iceServers: ICE_SERVERS });
    const dc = pc.createDataChannel(WEBRTC_CHANNEL_LABEL, { ordered: true });

    try {
      const offer = await pc.createOffer();
      await pc.setLocalDescription(offer);
      await waitForIceGathering(pc);

      const localSdp = pc.localDescription?.sdp;
      if (!localSdp) {
        throw new TransportError("transport", "无法生成本地 WebRTC SDP");
      }

      const reply = await signaling.request("POST", "/v1/link/webrtc/offer", { sdp: localSdp });
      if (reply.status === 404) {
        throw new TransportError("http", "中心版本暂不支持 P2P 直连（升级中心后自动启用）", 404);
      }
      if (reply.status === 401) {
        throw new TransportError("unauthorized", "设备授权已失效或被中心吊销", 401);
      }
      if (reply.status >= 400) {
        let msg = `信令交换失败 (HTTP ${reply.status})`;
        try {
          const parsed = JSON.parse(reply.body) as { error?: { message?: string } };
          if (parsed.error?.message) msg = parsed.error.message;
        } catch {
          // ignore
        }
        throw new TransportError("http", msg, reply.status);
      }

      const answer = JSON.parse(reply.body) as OfferReply;
      if (!answer.sdp) {
        throw new TransportError("transport", "中心未返回有效的 WebRTC Answer");
      }
      await pc.setRemoteDescription({ type: "answer", sdp: answer.sdp });

      await new Promise<void>((resolve, reject) => {
        if (dc.readyState === "open") {
          resolve();
          return;
        }
        let settled = false;
        const remoteTag = answer.remote_candidates?.length
          ? `（已识别候选：${answer.remote_candidates.join(", ")}）`
          : "";
        const timer = window.setTimeout(() => {
          fail(new TransportError("transport", `P2P 打洞超时，已保持中继${remoteTag}`));
        }, CHANNEL_OPEN_TIMEOUT_MS);

        function cleanup() {
          window.clearTimeout(timer);
          dc.onopen = null;
          dc.onerror = null;
          dc.onclose = null;
          pc.oniceconnectionstatechange = null;
          pc.onconnectionstatechange = null;
        }

        function ok() {
          if (settled) return;
          settled = true;
          cleanup();
          resolve();
        }

        function fail(err: TransportError) {
          if (settled) return;
          settled = true;
          cleanup();
          reject(err);
        }

        dc.onopen = ok;
        dc.onerror = () => fail(new TransportError("transport", "P2P 通道握手失败"));
        dc.onclose = () => fail(new TransportError("transport", "P2P 通道未就绪即关闭"));
        pc.oniceconnectionstatechange = () => {
          if (pc.iceConnectionState === "failed" || pc.iceConnectionState === "closed") {
            fail(new TransportError("transport", "P2P ICE 打洞失败，已保持中继"));
          }
        };
        pc.onconnectionstatechange = () => {
          if (pc.connectionState === "failed" || pc.connectionState === "closed") {
            fail(new TransportError("transport", "P2P 连接建立失败，已保持中继"));
          }
        };
      });

      const initialInfo: WebRtcInfo = {
        rttMs: 0,
        localType: "unknown",
        remoteType: "unknown",
        ipFamily: "unknown",
        hubCandidates: answer.candidates ?? [],
        connectedAt: Date.now(),
      };
      const transport = new WebRtcTransport(pc, dc, initialInfo, opts?.onDrop, opts?.onMetrics);
      const rttMs = await transport.ping(3000).catch(() => 0);
      transport.info = await inspectCandidatePair(pc, rttMs, answer.candidates ?? []);
      return transport;
    } catch (err) {
      try {
        dc.close();
      } catch {
        // ignore
      }
      try {
        pc.close();
      } catch {
        // ignore
      }
      throw err instanceof TransportError
        ? err
        : new TransportError("transport", err instanceof Error ? err.message : String(err));
    }
  }

  private markDropped(reason: string) {
    if (this.closed) return;
    this.closed = true;
    if (this.keepaliveTimer !== null) {
      window.clearInterval(this.keepaliveTimer);
      this.keepaliveTimer = null;
    }
    const err = new TransportError("transport", reason);
    for (const [, p] of this.pending) {
      window.clearTimeout(p.timer);
      p.reject(err);
    }
    this.pending.clear();
    for (const [, p] of this.pings) {
      window.clearTimeout(p.timer);
      p.reject(err);
    }
    this.pings.clear();
    const sses = Array.from(this.sseListeners.values());
    this.sseListeners.clear();
    for (const s of sses) {
      s.onError(err);
    }
    try {
      this.dc.close();
    } catch {
      // ignore
    }
    try {
      this.pc.close();
    } catch {
      // ignore
    }
    this.onDrop?.(reason);
  }

  private handleMessage(raw: string) {
    let frame: ServerFrame;
    try {
      frame = JSON.parse(raw) as ServerFrame;
    } catch {
      return;
    }

    switch (frame.t) {
      case "pong": {
        const entry = this.pings.get(frame.n);
        if (entry) {
          this.pings.delete(frame.n);
          window.clearTimeout(entry.timer);
          const ms = Math.max(1, Math.round(performance.now() - entry.sentAt));
          entry.resolve(ms);
        }
        break;
      }
      case "res": {
        const entry = this.pending.get(frame.id);
        if (!entry) return;
        this.pending.delete(frame.id);
        window.clearTimeout(entry.timer);
        if (frame.s === 401) {
          entry.reject(new TransportError("unauthorized", "设备授权已失效或被中心吊销", 401));
        } else {
          entry.resolve({ status: frame.s, body: frame.b });
        }
        break;
      }
      case "res_s": {
        const entry = this.pending.get(frame.id);
        if (entry) {
          entry.status = frame.s;
          entry.chunks = [];
        }
        break;
      }
      case "res_c": {
        const entry = this.pending.get(frame.id);
        if (entry?.chunks) {
          entry.chunks.push(frame.d);
        }
        break;
      }
      case "res_e": {
        const entry = this.pending.get(frame.id);
        if (!entry) return;
        this.pending.delete(frame.id);
        window.clearTimeout(entry.timer);
        const status = entry.status ?? 200;
        const body = entry.chunks ? entry.chunks.join("") : "";
        if (status === 401) {
          entry.reject(new TransportError("unauthorized", "设备授权已失效或被中心吊销", 401));
        } else {
          entry.resolve({ status, body });
        }
        break;
      }
      case "err": {
        const entry = this.pending.get(frame.id);
        if (!entry) return;
        this.pending.delete(frame.id);
        window.clearTimeout(entry.timer);
        entry.reject(new TransportError(mapErrorCode(frame.c), frame.m));
        break;
      }
      case "sse_data": {
        this.sseListeners.get(frame.id)?.onEvent(frame.d);
        break;
      }
      case "sse_ds": {
        const sub = this.sseListeners.get(frame.id);
        if (sub) sub.chunks = [];
        break;
      }
      case "sse_dc": {
        const sub = this.sseListeners.get(frame.id);
        if (sub?.chunks) sub.chunks.push(frame.d);
        break;
      }
      case "sse_de": {
        const sub = this.sseListeners.get(frame.id);
        if (sub?.chunks) {
          const payload = sub.chunks.join("");
          sub.chunks = undefined;
          sub.onEvent(payload);
        }
        break;
      }
      case "sse_err": {
        const sub = this.sseListeners.get(frame.id);
        if (!sub) return;
        this.sseListeners.delete(frame.id);
        sub.onError(new TransportError(mapErrorCode(frame.c), frame.m, frame.s));
        break;
      }
    }
  }

  private async sendWithBackpressure(payload: string): Promise<void> {
    if (this.closed || this.dc.readyState !== "open") {
      throw new TransportError("transport", "P2P 数据通道未打开");
    }
    if (this.dc.bufferedAmount > OUTBOUND_HIGH_WATER) {
      await new Promise<void>((resolve, reject) => {
        const onLow = () => {
          cleanup();
          resolve();
        };
        const onClose = () => {
          cleanup();
          reject(new TransportError("transport", "P2P 通道在发送期间关闭"));
        };
        const cleanup = () => {
          this.dc.removeEventListener("bufferedamountlow", onLow);
          this.dc.removeEventListener("close", onClose);
        };
        this.dc.addEventListener("bufferedamountlow", onLow);
        this.dc.addEventListener("close", onClose);
      });
    }
    try {
      this.dc.send(payload);
    } catch (err) {
      this.markDropped(err instanceof Error ? err.message : String(err));
      throw new TransportError("transport", "P2P 发送失败");
    }
  }

  async ping(timeoutMs = 3000): Promise<number> {
    if (this.closed || this.dc.readyState !== "open") {
      throw new TransportError("transport", "P2P 通道未连接");
    }
    const n = this.nextPing++;
    const sentAt = performance.now();
    return new Promise<number>((resolve, reject) => {
      const timer = window.setTimeout(() => {
        this.pings.delete(n);
        reject(new TransportError("transport", "P2P Ping 超时"));
      }, timeoutMs);
      this.pings.set(n, { sentAt, resolve, reject, timer });
      try {
        this.dc.send(JSON.stringify({ t: "ping", n }));
      } catch (err) {
        this.pings.delete(n);
        window.clearTimeout(timer);
        reject(err instanceof Error ? err : new Error(String(err)));
      }
    });
  }

  async request(method: "GET" | "POST" | "DELETE", path: string, body?: unknown): Promise<HttpReply> {
    if (this.closed || this.dc.readyState !== "open") {
      throw new TransportError("transport", "P2P 通道未打开");
    }
    const id = this.nextId++;
    const bodyStr = body === undefined ? undefined : JSON.stringify(body);

    const promise = new Promise<HttpReply>((resolve, reject) => {
      const timer = window.setTimeout(() => {
        this.pending.delete(id);
        reject(new TransportError("transport", `P2P 请求超时 (${method} ${path})`));
      }, REQUEST_TIMEOUT_MS);
      this.pending.set(id, { resolve, reject, timer });
    });

    try {
      if (bodyStr === undefined || bodyStr.length <= OUTBOUND_CHUNK_CHARS) {
        await this.sendWithBackpressure(
          JSON.stringify(
            bodyStr === undefined
              ? { t: "req", id, m: method, p: path }
              : { t: "req", id, m: method, p: path, b: bodyStr },
          ),
        );
      } else {
        await this.sendWithBackpressure(JSON.stringify({ t: "req_s", id, m: method, p: path }));
        for (let i = 0; i < bodyStr.length; i += OUTBOUND_CHUNK_CHARS) {
          await this.sendWithBackpressure(
            JSON.stringify({ t: "req_c", id, d: bodyStr.slice(i, i + OUTBOUND_CHUNK_CHARS) }),
          );
        }
        await this.sendWithBackpressure(JSON.stringify({ t: "req_e", id }));
      }
    } catch (err) {
      const entry = this.pending.get(id);
      if (entry) {
        this.pending.delete(id);
        window.clearTimeout(entry.timer);
      }
      throw err instanceof TransportError
        ? err
        : new TransportError("transport", err instanceof Error ? err.message : String(err));
    }

    return promise;
  }

  async subscribeEvents(
    since: number | undefined,
    onEvent: (rawJson: string) => void,
    onError: (err: TransportError) => void,
  ): Promise<EventHandle> {
    if (this.closed || this.dc.readyState !== "open") {
      throw new TransportError("transport", "P2P 通道未打开");
    }
    const id = this.nextId++;
    this.sseListeners.set(id, { onEvent, onError });
    await this.sendWithBackpressure(JSON.stringify({ t: "sse_open", id, since: since ?? 0 }));
    return {
      cancel: () => {
        if (!this.sseListeners.delete(id)) return;
        if (!this.closed && this.dc.readyState === "open") {
          try {
            this.dc.send(JSON.stringify({ t: "sse_close", id }));
          } catch {
            // ignore
          }
        }
      },
    };
  }

  isClosed(): boolean {
    return this.closed || this.dc.readyState !== "open";
  }

  async close(): Promise<void> {
    this.markDropped("P2P 已主动关闭");
  }
}
