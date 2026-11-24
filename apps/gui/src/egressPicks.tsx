import { useCallback, useEffect, useState } from "react";
import type { Agent } from "@bindings/Agent";
import type { EgressCandidate } from "@bindings/EgressCandidate";
import { RefreshCw } from "lucide-react";
import { api, errorText } from "./api";

/** `config.proxy` on an agent: `"direct"`, a proxy URL, or `null` (follow account/system). */
export function proxyOf(agent: Pick<Agent, "config">): string | null {
  const value = (agent.config as Record<string, unknown> | null | undefined)?.proxy;
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

/** Compact label for chips (`"http://127.0.0.1:7890"` -> `"127.0.0.1:7890"`). */
export function shortProxyLabel(proxy: string | null | undefined): string {
  if (!proxy) return "默认";
  if (proxy.toLowerCase() === "auto") return "自动最低延迟";
  if (proxy.toLowerCase() === "direct") return "直连";
  return proxy.replace(/^https?:\/\//i, "");
}

export function formatCandidateOption(c: EgressCandidate): string {
  if (c.ip) {
    const region = c.country ? `[${c.country}] ` : "";
    const rtt = c.latency_ms != null ? ` (${c.latency_ms}ms)` : "";
    return `${region}${c.ip} · ${c.label}${rtt}`;
  }
  return `${c.label}（无法探测出口 IP）`;
}

let cachedCandidates: EgressCandidate[] | undefined;
let inflight: Promise<EgressCandidate[]> | undefined;
let lastProbedAt = 0;
const listeners = new Set<(list: EgressCandidate[]) => void>();

function notifyAll(list: EgressCandidate[]) {
  cachedCandidates = list;
  lastProbedAt = Date.now();
  for (const fn of listeners) fn(list);
}

export function useEgressCandidates() {
  const [candidates, setCandidates] = useState<EgressCandidate[]>(cachedCandidates ?? []);
  const [loading, setLoading] = useState(!cachedCandidates);
  const [error, setError] = useState<string>();

  const reload = useCallback(async (force = true) => {
    if (!force && cachedCandidates) {
      setCandidates(cachedCandidates);
      setLoading(false);
      return cachedCandidates;
    }
    setLoading(true);
    setError(undefined);
    try {
      inflight ??= api
        .egressCandidates()
        .then((r) => r.candidates)
        .finally(() => {
          inflight = undefined;
        });
      const list = await inflight;
      notifyAll(list);
      return list;
    } catch (err) {
      setError(errorText(err));
      return cachedCandidates ?? [];
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    listeners.add(setCandidates);
    if (!cachedCandidates) void reload(false);
    return () => {
      listeners.delete(setCandidates);
    };
  }, [reload]);

  return { candidates, loading, error, reload: () => reload(true), update: notifyAll };
}

export function EgressPicker({
  value,
  onChange,
  defaultLabel = "跟随默认（系统 / 环境代理）",
  disabled = false,
  title,
  compact = false,
}: {
  value: string | null;
  onChange: (next: string | null) => void;
  defaultLabel?: string;
  disabled?: boolean;
  title?: string;
  compact?: boolean;
}) {
  const { candidates, loading, error, reload, update } = useEgressCandidates();
  const [adding, setAdding] = useState(false);
  const [customUrl, setCustomUrl] = useState("");
  const [customLabel, setCustomLabel] = useState("");
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string>();

  const current = value?.trim() || "";
  const isAuto = current.toLowerCase() === "auto";
  const hasCurrent =
    !current ||
    isAuto ||
    candidates.some((c) => c.value.toLowerCase() === current.toLowerCase());
  const selectedCustom = candidates.find(
    (c) => c.value.toLowerCase() === current.toLowerCase() && c.source === "custom",
  );
  const bestProxy =
    [...candidates]
      .filter((c) => c.ip && c.latency_ms != null && c.value !== "direct")
      .sort((a, b) => (a.latency_ms ?? 999999) - (b.latency_ms ?? 999999))[0] ??
    [...candidates]
      .filter((c) => c.ip && c.latency_ms != null)
      .sort((a, b) => (a.latency_ms ?? 999999) - (b.latency_ms ?? 999999))[0];

  useEffect(() => {
    if (!isAuto) return;
    const highLatency = !bestProxy || bestProxy.latency_ms == null || bestProxy.latency_ms > 280;
    const intervalMs = highLatency ? 15_000 : 60_000;
    const timer = setInterval(() => {
      if (Date.now() - lastProbedAt >= intervalMs - 1_000) {
        void reload();
      }
    }, Math.min(intervalMs, 15_000));
    return () => clearInterval(timer);
  }, [isAuto, bestProxy?.latency_ms, reload]);

  async function addProxy() {
    const url = customUrl.trim();
    if (!url) return;
    setSaving(true);
    setSaveError(undefined);
    try {
      const res = await api.saveEgressProxy({
        url,
        label: customLabel.trim() || undefined,
      });
      update(res.candidates);
      const normalized = url.includes("://") ? url : `http://${url}`;
      const match = res.candidates.find((c) => c.value.toLowerCase() === normalized.toLowerCase());
      onChange(match ? match.value : normalized);
      setCustomUrl("");
      setCustomLabel("");
      setAdding(false);
    } catch (err) {
      setSaveError(errorText(err));
    } finally {
      setSaving(false);
    }
  }

  async function removeSelectedCustom() {
    if (!selectedCustom) return;
    setSaving(true);
    setSaveError(undefined);
    try {
      const res = await api.saveEgressProxy({
        url: selectedCustom.value,
        remove: true,
      });
      update(res.candidates);
      onChange(null);
    } catch (err) {
      setSaveError(errorText(err));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className={`egress-picker ${compact ? "compact" : ""}`} style={{ display: "flex", flexDirection: "column", gap: 6 }}>
      <div className="row" style={{ gap: 6, flexWrap: "wrap", alignItems: "center" }}>
        <select
          value={isAuto ? "auto" : current}
          disabled={disabled || saving}
          title={title}
          style={compact ? { fontSize: 12, padding: "2px 8px", height: 26, maxWidth: 360 } : { flex: 1, minWidth: 220 }}
          onChange={(e) => {
            onChange(e.target.value ? e.target.value : null);
          }}
        >
          <option value="">{defaultLabel}</option>
          <option value="auto">
            ⚡ 自动选择最低延迟{bestProxy ? `（${bestProxy.label} · ${bestProxy.latency_ms}ms）` : ""}
          </option>
          {!hasCurrent && <option value={current}>{current}（当前已配）</option>}
          {candidates.map((c) => (
            <option key={c.value} value={c.value}>
              {formatCandidateOption(c)}
            </option>
          ))}
        </select>
        <button
          type="button"
          className="ghost small"
          disabled={loading || disabled}
          onClick={() => void reload()}
          title="重新扫描本机代理端口并探测各出口的真实 IP 与国家/地区"
        >
          <span className="row" style={{ gap: 4 }}>
            <RefreshCw size={12} className={loading ? "spin" : undefined} />
            <span>{loading ? "探测中…" : "刷新出口"}</span>
          </span>
        </button>
        <button
          type="button"
          className="ghost small"
          disabled={disabled || saving}
          onClick={() => setAdding((v) => !v)}
          title="手动添加 HTTP 或 SOCKS5 代理端口并探测其出口 IP"
        >
          {adding ? "收起" : "+ 添加代理"}
        </button>
        {selectedCustom && (
          <button
            type="button"
            className="ghost small danger"
            disabled={disabled || saving}
            onClick={() => void removeSelectedCustom()}
            title="从候选代理列表中移除此自定义代理"
          >
            移除该代理
          </button>
        )}
      </div>
      {adding && (
        <div className="row" style={{ gap: 6, flexWrap: "wrap", alignItems: "center" }}>
          <input
            value={customUrl}
            onChange={(e) => setCustomUrl(e.target.value)}
            placeholder="代理地址，如 http://127.0.0.1:7890 或 socks5://127.0.0.1:1080"
            style={{ flex: 2, minWidth: 210, fontSize: compact ? 12 : undefined }}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                void addProxy();
              }
            }}
          />
          <input
            value={customLabel}
            onChange={(e) => setCustomLabel(e.target.value)}
            placeholder="备注（可选，如 美区节点）"
            style={{ flex: 1, minWidth: 120, fontSize: compact ? 12 : undefined }}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                void addProxy();
              }
            }}
          />
          <button
            type="button"
            className="ghost small"
            disabled={saving || !customUrl.trim()}
            onClick={() => void addProxy()}
          >
            {saving ? "探测中…" : "保存并探测"}
          </button>
        </div>
      )}
      {(error || saveError) && (
        <span className="muted small" style={{ color: "var(--danger-text)" }}>
          {saveError ?? error}
        </span>
      )}
    </div>
  );
}
