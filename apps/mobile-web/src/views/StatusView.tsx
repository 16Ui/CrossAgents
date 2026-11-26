import { useEffect, useState } from "react";
import { Activity, Bell, BellOff, RefreshCw, Send, ShieldAlert, Unlink, Zap } from "lucide-react";
import {
  OsIcon,
  planLabel,
  quotaLevel,
  resetText,
  UserAvatar,
  VendorMark,
  vendorName,
  windowLabel,
} from "../brands";
import {
  disableWebPush,
  enableWebPush,
  getCurrentPushEndpoint,
  getPushSupport,
  sendTestPush,
} from "../push";
import { nodeDisplayName } from "../store";
import { FONTS, MODES, PAPERS, setTheme, useTheme } from "../theme";
import { runRelayDiagnostics, type HubTransport, type LinkStatus, type RelayDiagItem } from "../transport";
import type {
  ConnectionState,
  HubInfo,
  PairingRecord,
  QuotaResponse,
  RateWindow,
  RemoteNodeInfo,
  UserProfile,
} from "../types";

function relayHostLabel(url?: string | null): string {
  if (!url) return "";
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

export function StatusView({
  pairing,
  connection,
  linkStatus,
  hubInfo,
  profile,
  nodes,
  quota,
  transport,
  onReconnect,
  onRefresh,
  onForget,
}: {
  pairing: PairingRecord;
  connection: ConnectionState;
  linkStatus?: LinkStatus;
  hubInfo?: HubInfo;
  profile?: UserProfile;
  nodes: RemoteNodeInfo[];
  quota?: QuotaResponse;
  transport: HubTransport | null;
  onReconnect: () => void;
  onRefresh: () => void;
  onForget: () => void;
}) {
  const theme = useTheme();
  const [pushEndpoint, setPushEndpoint] = useState<string | null>(null);
  const [pushBusy, setPushBusy] = useState(false);
  const [pushNote, setPushNote] = useState<string>();
  const [confirmForget, setConfirmForget] = useState(false);
  const [meterMode, setMeterMode] = useState<"remain" | "used">("remain");
  const [diagBusy, setDiagBusy] = useState(false);
  const [diagItems, setDiagItems] = useState<RelayDiagItem[] | null>(null);
  const [p2pBusy, setP2pBusy] = useState(false);

  const pushSupport = getPushSupport();

  useEffect(() => {
    void getCurrentPushEndpoint().then(setPushEndpoint);
  }, []);

  async function togglePush() {
    if (!transport || pushBusy) return;
    setPushBusy(true);
    setPushNote(undefined);
    try {
      if (pushEndpoint) {
        await disableWebPush(transport);
        setPushEndpoint(null);
        setPushNote("已关闭本机锁屏推送提醒。");
      } else {
        const ep = await enableWebPush(transport);
        setPushEndpoint(ep);
        setPushNote("锁屏推送已开启！有待办审批时会自动推送提醒。");
      }
    } catch (err) {
      setPushNote(err instanceof Error ? err.message : String(err));
    } finally {
      setPushBusy(false);
    }
  }

  async function testPush() {
    if (!transport || pushBusy) return;
    setPushBusy(true);
    setPushNote(undefined);
    try {
      const res = await sendTestPush(transport, pushEndpoint);
      setPushNote(
        res.sent > 0
          ? "测试推送已发出，请留意锁屏/顶部横幅通知。"
          : "推送未能送达，请确认中心网络可访问 Apple 推送服务。",
      );
    } catch (err) {
      setPushNote(err instanceof Error ? err.message : String(err));
    } finally {
      setPushBusy(false);
    }
  }

  const connChip =
    connection.kind === "connected"
      ? connection.mode === "direct"
        ? { label: "HTTPS 直连", cls: "ok" }
        : linkStatus?.kind === "p2p"
          ? {
              label:
                linkStatus.p2pInfo && linkStatus.p2pInfo.rttMs > 0
                  ? `P2P 直连 · ${linkStatus.p2pInfo.rttMs}ms`
                  : "P2P 直连",
              cls: "ok",
            }
          : linkStatus?.p2pState === "connecting"
            ? { label: "iroh · 尝试直连…", cls: "ok" }
            : { label: "iroh 中继", cls: "ok" }
      : connection.kind === "connecting"
        ? { label: "连接中…", cls: "accent" }
        : connection.kind === "revoked"
          ? { label: "已吊销", cls: "danger" }
          : { label: connection.kind === "relay_unreachable" ? "中继不可达" : "离线", cls: "warn" };

  const p2pDetailText =
    linkStatus?.kind === "p2p" && linkStatus.p2pInfo
      ? [
          linkStatus.p2pInfo.ipFamily !== "unknown" ? linkStatus.p2pInfo.ipFamily : null,
          linkStatus.p2pInfo.localType !== "unknown" || linkStatus.p2pInfo.remoteType !== "unknown"
            ? `${linkStatus.p2pInfo.localType} ↔ ${linkStatus.p2pInfo.remoteType}`
            : null,
          linkStatus.p2pInfo.rttMs > 0 ? `${linkStatus.p2pInfo.rttMs}ms` : null,
        ]
          .filter(Boolean)
          .join(" · ")
      : linkStatus?.p2pNote;

  return (
    <>
      <div className="nav-bar">
        <div className="nav-title-wrap">
          <h1 className="nav-title">概览</h1>
          <div className="nav-subtitle">推送、外观、节点与订阅额度</div>
        </div>
        <button type="button" className="icon-btn" onClick={onRefresh} title="刷新">
          <RefreshCw size={16} />
        </button>
      </div>

      <div className="page-scroll">
        {/* Personal Profile Card (aligned 1:1 with Desktop Sidebar `.foot-user`) */}
        <div className="card profile-banner-card">
          <div className="profile-avatar-wrap">
            <UserAvatar image={profile?.avatar} size={46} />
            <span
              className={`profile-status-dot ${connChip.cls} ${connection.kind === "connecting" ? "pulse" : ""}`}
              aria-hidden="true"
            />
          </div>
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ fontSize: 16, fontWeight: 650, letterSpacing: "-0.015em", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
              {profile?.name?.trim() || pairing.deviceName || "我"}
            </div>
            <div style={{ fontSize: 12, color: "var(--muted)", marginTop: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
              {hubInfo?.machine?.hostname || pairing.host}
              {hubInfo?.version ? ` · v${hubInfo.version}` : ""}
              {pairing.deviceName ? ` · ${pairing.deviceName}` : ""}
            </div>
          </div>
          <span className={`chip ${connChip.cls}`}>
            <span className={`dot ${connChip.cls}`} />
            {connChip.label}
          </span>
        </div>

        {/* Lock-Screen Web Push */}
        <div className="section-label">锁屏审批推送</div>
        <div className="card" style={{ display: "flex", flexDirection: "column", gap: 10 }}>
          <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8 }}>
            <div>
              <div style={{ fontSize: 14.5, fontWeight: 600 }}>免费 iOS 锁屏提醒</div>
              <div style={{ fontSize: 12, color: "var(--muted)", marginTop: 2 }}>
                锁屏仅提示「有一项待处理」，不泄露命令或仓库隐私
              </div>
            </div>
            <span className={`chip ${pushEndpoint ? "ok" : ""}`}>{pushEndpoint ? "已开启" : "未开启"}</span>
          </div>

          {!pushSupport.supported ? (
            <div className="chip accent" style={{ borderRadius: 8, padding: "8px 10px", whiteSpace: "normal", lineHeight: 1.45 }}>
              {pushSupport.reason}
            </div>
          ) : (
            <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
              <button
                type="button"
                className={pushEndpoint ? "secondary-btn" : "primary-btn"}
                style={pushEndpoint ? undefined : { flex: 1 }}
                disabled={pushBusy || !transport || connection.kind !== "connected"}
                onClick={() => void togglePush()}
              >
                {pushEndpoint ? <BellOff size={14} /> : <Bell size={15} />}
                {pushBusy ? "处理中…" : pushEndpoint ? "关闭推送" : "开启免费锁屏推送"}
              </button>
              {pushEndpoint && (
                <button
                  type="button"
                  className="secondary-btn"
                  disabled={pushBusy || !transport || connection.kind !== "connected"}
                  onClick={() => void testPush()}
                >
                  <Send size={13} />
                  发一条测试通知
                </button>
              )}
            </div>
          )}

          {pushNote && (
            <div style={{ fontSize: 12.5, color: "var(--text-2)", lineHeight: 1.5 }}>{pushNote}</div>
          )}
        </div>

        {/* Appearance (1:1 aligned with Desktop Settings → 外观) */}
        <div className="section-label">外观与主题</div>
        <div className="card" style={{ display: "flex", flexDirection: "column", gap: 12 }}>
          <div className="theme-grid">
            {PAPERS.map((p) => {
              const active = theme.paper === p.key;
              return (
                <button
                  key={p.key}
                  type="button"
                  className={`theme-swatch-btn ${active ? "active" : ""}`}
                  onClick={() => setTheme({ paper: p.key })}
                >
                  <span className="theme-swatch-dot" style={{ background: p.bg }}>
                    <span className="theme-swatch-accent" style={{ background: p.accent }} />
                  </span>
                  <div style={{ minWidth: 0 }}>
                    <div style={{ fontSize: 13, fontWeight: 600, color: "var(--text)" }}>{p.label}</div>
                    <div style={{ fontSize: 11, color: "var(--muted)", whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>
                      {p.hint}
                    </div>
                  </div>
                </button>
              );
            })}
          </div>

          <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
            <div style={{ fontSize: 11.5, color: "var(--muted)" }}>明暗模式</div>
            <div className="segmented">
              {MODES.map((m) => (
                <button
                  key={m.key}
                  type="button"
                  className={theme.mode === m.key ? "active" : ""}
                  onClick={() => setTheme({ mode: m.key })}
                >
                  {m.label}
                </button>
              ))}
            </div>
          </div>

          <div style={{ display: "flex", flexDirection: "column", gap: 6 }}>
            <div style={{ fontSize: 11.5, color: "var(--muted)" }}>对话正文字体</div>
            <div className="segmented">
              {FONTS.map((f) => (
                <button
                  key={f.key}
                  type="button"
                  className={theme.font === f.key ? "active" : ""}
                  onClick={() => setTheme({ font: f.key })}
                >
                  {f.label}
                </button>
              ))}
            </div>
          </div>
        </div>

        {/* Connection */}
        <div className="section-label">当前中心连接</div>
        <div className="card" style={{ display: "flex", flexDirection: "column", gap: 11 }}>
          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
            <div className="account-vendor-frame">
              <OsIcon os={hubInfo?.machine?.os ?? "windows"} size={17} />
            </div>
            <div style={{ flex: 1, minWidth: 0 }}>
              <div style={{ fontSize: 14.5, fontWeight: 600 }}>
                {hubInfo?.machine?.hostname || pairing.host}
              </div>
              <div style={{ fontSize: 11.5, color: "var(--muted)" }}>
                {pairing.deviceName}
                {pairing.level ? ` · ${pairing.level}` : ""}
                {hubInfo?.version ? ` · v${hubInfo.version}` : ""}
              </div>
              {pairing.relay && (
                <div className="mono" style={{ fontSize: 11, color: "var(--muted)", marginTop: 2 }}>
                  信令/中继：{relayHostLabel(pairing.relay)}
                </div>
              )}
              {pairing.mode === "iroh" && p2pDetailText && (
                <div
                  className="mono"
                  style={{
                    fontSize: 11,
                    color: linkStatus?.kind === "p2p" ? "var(--ok-text)" : "var(--muted)",
                    marginTop: 2,
                  }}
                >
                  {linkStatus?.kind === "p2p" ? `P2P 直连：${p2pDetailText}` : p2pDetailText}
                </div>
              )}
            </div>
            <span className={`chip ${connChip.cls}`}>
              <span className={`dot ${connChip.cls}`} />
              {connChip.label}
            </span>
          </div>

          <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
            <button type="button" className="secondary-btn" style={{ flex: 1 }} onClick={onReconnect}>
              <RefreshCw size={14} />
              重连
            </button>
            {pairing.mode === "iroh" && transport?.retryP2p && connection.kind === "connected" && (
              <button
                type="button"
                className="secondary-btn"
                disabled={p2pBusy || linkStatus?.p2pState === "connecting"}
                onClick={async () => {
                  setP2pBusy(true);
                  try {
                    await transport.retryP2p?.();
                  } finally {
                    setP2pBusy(false);
                  }
                }}
              >
                <Zap size={14} />
                {p2pBusy || linkStatus?.p2pState === "connecting"
                  ? "直连中"
                  : linkStatus?.kind === "p2p"
                    ? "测 RTT"
                    : "尝试直连"}
              </button>
            )}
            <button
              type="button"
              className="secondary-btn"
              disabled={diagBusy}
              onClick={async () => {
                setDiagBusy(true);
                try {
                  setDiagItems(await runRelayDiagnostics(pairing.relay));
                } finally {
                  setDiagBusy(false);
                }
              }}
            >
              <Activity size={14} />
              {diagBusy ? "测速中" : "测中继"}
            </button>
            <button
              type="button"
              className="danger-btn"
              onClick={() => {
                if (confirmForget) onForget();
                else setConfirmForget(true);
              }}
            >
              <Unlink size={14} />
              {confirmForget ? "确认忘记" : "忘记"}
            </button>
          </div>

          {diagItems && (
            <div
              style={{
                borderRadius: 9,
                padding: "9px 10px",
                background: "var(--surface-sunk)",
                border: "1px solid var(--border)",
                fontSize: 11.5,
                lineHeight: 1.55,
                display: "flex",
                flexDirection: "column",
                gap: 6,
              }}
            >
              {diagItems.map((item) => (
                <div key={item.url} className="mono" style={{ wordBreak: "break-all" }}>
                  <div style={{ fontWeight: 600 }}>{item.host}</div>
                  <div style={{ color: item.wsOk ? "var(--ok-text)" : "var(--danger-text)" }}>
                    HTTPS {item.httpOk ? `${item.httpMs}ms` : item.httpDetail} · WSS{" "}
                    {item.wsOk ? `${item.wsMs}ms (${item.wsDetail})` : item.wsDetail}
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>

        {/* Nodes */}
        <div className="section-label">
          在线机器与节点 ({nodes.filter((n) => n.online).length}/{nodes.length})
        </div>
        {nodes.length === 0 ? (
          <div className="card" style={{ fontSize: 13, color: "var(--muted)" }}>
            暂无节点数据
          </div>
        ) : (
          nodes.map((node) => {
            const installed = node.plugins.filter((p) => p.installed);
            return (
              <div key={node.id} className="card" style={{ display: "flex", flexDirection: "column", gap: 9 }}>
                <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
                  <div className="account-vendor-frame">
                    <OsIcon os={node.os} size={17} />
                  </div>
                  <div style={{ flex: 1, minWidth: 0 }}>
                    <div style={{ fontSize: 14, fontWeight: 600 }}>
                      {nodeDisplayName(node.id, nodes, hubInfo, pairing.host)}
                    </div>
                    <div className="mono" style={{ fontSize: 11.5, color: "var(--muted)" }}>
                      {node.os}/{node.arch}
                      {node.version ? ` · v${node.version}` : ""}
                    </div>
                  </div>
                  <span className={`chip ${node.online ? "ok" : ""}`}>
                    <span className={`dot ${node.online ? "ok" : "off"}`} />
                    {node.online ? "在线" : "离线"}
                  </span>
                </div>
                {installed.length > 0 && (
                  <div className="pill-wrap">
                    {installed.map((p) => (
                      <span key={p.name} className="chip">
                        <VendorMark plugin={p.name} size={12} />
                        <span>{vendorName(p.name)}</span>
                        {p.version && <span className="muted mono">{p.version}</span>}
                      </span>
                    ))}
                  </div>
                )}
              </div>
            );
          })
        )}

        {/* Quota (aligned 1:1 with Desktop `Quota.tsx`) */}
        <div className="section-label" style={{ justifyContent: "space-between" }}>
          <span>订阅与额度 {quota?.accounts.length ? `(${quota.accounts.length})` : ""}</span>
          {quota && quota.accounts.length > 0 && (
            <div className="segmented" style={{ width: "auto" }}>
              <button
                type="button"
                style={{ padding: "2px 9px", fontSize: 11.5 }}
                className={meterMode === "remain" ? "active" : ""}
                onClick={() => setMeterMode("remain")}
              >
                剩余
              </button>
              <button
                type="button"
                style={{ padding: "2px 9px", fontSize: 11.5 }}
                className={meterMode === "used" ? "active" : ""}
                onClick={() => setMeterMode("used")}
              >
                已用
              </button>
            </div>
          )}
        </div>
        {!quota || quota.accounts.length === 0 ? (
          <div className="card" style={{ fontSize: 13, color: "var(--muted)" }}>
            暂无订阅配额数据
          </div>
        ) : (
          quota.accounts.map((item) => {
            const windows: { group?: string | null; w: RateWindow; reached: boolean }[] =
              item.status.limits.length > 0
                ? item.status.limits.flatMap((g) =>
                    g.windows.map((w) => ({ group: g.name || g.model, w, reached: g.reached })),
                  )
                : item.status.rate_limits.map((w) => ({ group: null, w, reached: item.status.limit_reached }));
            const plan = planLabel(item.status.plan);

            return (
              <div key={item.account.id} className="card" style={{ display: "flex", flexDirection: "column", gap: 11 }}>
                <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
                  <div className="account-vendor-frame">
                    <VendorMark plugin={item.account.provider} size={18} />
                  </div>
                  <div style={{ flex: 1, minWidth: 0 }}>
                    <div style={{ fontSize: 14, fontWeight: 600, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                      {item.account.label || item.account.id}
                    </div>
                    <div style={{ fontSize: 11.5, color: "var(--muted)" }}>
                      {vendorName(item.account.provider)}
                      {plan ? ` · ${plan}` : ""}
                      {item.status.email ? ` · ${item.status.email}` : ""}
                    </div>
                  </div>
                  {item.status.limit_reached ? (
                    <span className="chip danger">已达限额</span>
                  ) : !item.status.logged_in ? (
                    <span className="chip warn">未登录</span>
                  ) : null}
                </div>

                {item.at_risk && (
                  <div className="error-box" style={{ display: "flex", alignItems: "center", gap: 6 }}>
                    <ShieldAlert size={14} style={{ flex: "none" }} />
                    <span>{item.at_risk}</span>
                  </div>
                )}

                {windows.length === 0 ? (
                  <div style={{ fontSize: 12.5, color: "var(--muted)" }}>
                    {item.status.credits?.has_credits
                      ? item.status.credits.unlimited
                        ? "无限额度"
                        : `余额：${item.status.credits.balance ?? "—"}`
                      : item.status.note || "暂无额度窗口数据"}
                  </div>
                ) : (
                  <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
                    {windows.map((entry, idx) => {
                      const used = Math.max(0, Math.min(100, Math.round(entry.w.used_percent)));
                      const val = meterMode === "used" ? used : 100 - used;
                      const tone = quotaLevel(used, entry.reached && used >= 100);
                      const baseLabel = windowLabel(entry.w).replace(/限额$/, "") || windowLabel(entry.w);
                      const title = entry.group && entry.group !== baseLabel ? `${entry.group} · ${baseLabel}` : baseLabel;
                      const reset = resetText(entry.w.resets_at);
                      const pool =
                        entry.w.total != null && entry.w.remaining != null
                          ? `${Math.round(meterMode === "used" ? entry.w.total - entry.w.remaining : entry.w.remaining)} / ${Math.round(entry.w.total)}`
                          : `${val}%`;
                      return (
                        <div key={idx} style={{ display: "flex", flexDirection: "column", gap: 4 }}>
                          <div style={{ display: "flex", justifyContent: "space-between", fontSize: 12.5 }}>
                            <span style={{ color: "var(--text-2)" }}>{title}</span>
                            <span
                              style={{
                                fontWeight: 600,
                                fontVariantNumeric: "tabular-nums",
                                color:
                                  tone === "danger"
                                    ? "var(--danger-text)"
                                    : tone === "warn"
                                      ? "var(--warn-text)"
                                      : "var(--text-2)",
                              }}
                            >
                              {meterMode === "used" ? "已用" : "剩余"} {pool}
                            </span>
                          </div>
                          <div className={`quota-bar ${tone}`}>
                            <span style={{ width: `${val}%` }} />
                          </div>
                          {reset && <div style={{ fontSize: 11, color: "var(--muted)" }}>{reset}</div>}
                        </div>
                      );
                    })}
                  </div>
                )}
              </div>
            );
          })
        )}
      </div>
    </>
  );
}
