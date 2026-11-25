import { useEffect, useState } from "react";
import { Bell, BellOff, Laptop, Monitor, RefreshCw, Send, Server, ShieldAlert, Unlink } from "lucide-react";
import {
  disableWebPush,
  enableWebPush,
  getCurrentPushEndpoint,
  getPushSupport,
  sendTestPush,
} from "../push";
import { nodeDisplayName, resetCountdown } from "../store";
import type { HubTransport } from "../transport";
import type { ConnectionState, HubInfo, PairingRecord, QuotaResponse, RateWindow, RemoteNodeInfo } from "../types";

export function StatusView({
  pairing,
  connection,
  hubInfo,
  nodes,
  quota,
  transport,
  onReconnect,
  onRefresh,
  onForget,
}: {
  pairing: PairingRecord;
  connection: ConnectionState;
  hubInfo?: HubInfo;
  nodes: RemoteNodeInfo[];
  quota?: QuotaResponse;
  transport: HubTransport | null;
  onReconnect: () => void;
  onRefresh: () => void;
  onForget: () => void;
}) {
  const [pushEndpoint, setPushEndpoint] = useState<string | null>(null);
  const [pushBusy, setPushBusy] = useState(false);
  const [pushNote, setPushNote] = useState<string>();
  const [confirmForget, setConfirmForget] = useState(false);

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
      setPushNote(res.sent > 0 ? "测试推送已发出，请留意锁屏/顶部横幅通知。" : "推送未能送达，请确认中心网络可访问推送服务。");
    } catch (err) {
      setPushNote(err instanceof Error ? err.message : String(err));
    } finally {
      setPushBusy(false);
    }
  }

  const connChip =
    connection.kind === "connected"
      ? { label: connection.mode === "iroh" ? "iroh E2EE 已连接" : "HTTPS 直连", cls: "ok" }
      : connection.kind === "connecting"
        ? { label: "连接中…", cls: "accent" }
        : connection.kind === "revoked"
          ? { label: "已吊销", cls: "danger" }
          : { label: connection.kind === "relay_unreachable" ? "中继不可达" : "中心离线", cls: "warn" };

  return (
    <>
      <div className="nav-bar">
        <div className="nav-title-wrap">
          <h1 className="nav-title">概览</h1>
        </div>
        <button type="button" className="icon-btn" onClick={onRefresh} title="刷新">
          <RefreshCw size={17} />
        </button>
      </div>

      <div className="page-scroll">
        {/* Free iOS Lock-Screen Push Notification Card */}
        <div className="section-label">锁屏审批推送（免费 Web Push）</div>
        <div className="card" style={{ display: "flex", flexDirection: "column", gap: 10 }}>
          <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8 }}>
            <div>
              <div style={{ fontSize: 15, fontWeight: 700 }}>待办锁屏提醒</div>
              <div style={{ fontSize: 12.5, color: "var(--text-2)", marginTop: 2 }}>
                锁屏仅提示「有一项待处理」，不泄露命令或仓库隐私
              </div>
            </div>
            <span className={`chip ${pushEndpoint ? "ok" : ""}`}>{pushEndpoint ? "已开启" : "未开启"}</span>
          </div>

          {!pushSupport.supported ? (
            <div className="chip accent" style={{ borderRadius: 10, padding: "8px 10px", whiteSpace: "normal", lineHeight: 1.45 }}>
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
                {pushEndpoint ? <BellOff size={15} /> : <Bell size={16} />}
                {pushBusy ? "处理中…" : pushEndpoint ? "关闭推送" : "开启免费锁屏推送"}
              </button>
              {pushEndpoint && (
                <button
                  type="button"
                  className="secondary-btn"
                  disabled={pushBusy || !transport || connection.kind !== "connected"}
                  onClick={() => void testPush()}
                >
                  <Send size={14} />
                  发一条测试通知
                </button>
              )}
            </div>
          )}

          {pushNote && (
            <div style={{ fontSize: 12.5, color: "var(--text-2)", lineHeight: 1.45 }}>{pushNote}</div>
          )}
        </div>

        {/* Connection Card */}
        <div className="section-label">当前中心连接</div>
        <div className="card" style={{ display: "flex", flexDirection: "column", gap: 12 }}>
          <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
            <Monitor size={22} style={{ color: "var(--accent)", flex: "none" }} />
            <div style={{ flex: 1, minWidth: 0 }}>
              <div style={{ fontSize: 15.5, fontWeight: 700 }}>
                {hubInfo?.machine?.hostname || pairing.host}
              </div>
              <div style={{ fontSize: 12, color: "var(--text-2)" }}>
                设备：{pairing.deviceName}
                {pairing.level ? ` · ${pairing.level}` : ""}
                {hubInfo?.version ? ` · Hub v${hubInfo.version}` : ""}
              </div>
            </div>
            <span className={`chip ${connChip.cls}`}>{connChip.label}</span>
          </div>

          <div style={{ display: "flex", gap: 8 }}>
            <button type="button" className="secondary-btn" style={{ flex: 1 }} onClick={onReconnect}>
              <RefreshCw size={14} />
              重新连接
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
              {confirmForget ? "再点一次忘记" : "忘记中心"}
            </button>
          </div>
        </div>

        {/* Nodes */}
        <div className="section-label">
          在线机器与节点 ({nodes.filter((n) => n.online).length}/{nodes.length})
        </div>
        {nodes.length === 0 ? (
          <div className="card" style={{ fontSize: 13, color: "var(--text-2)" }}>
            暂无节点数据
          </div>
        ) : (
          nodes.map((node) => {
            const installed = node.plugins.filter((p) => p.installed);
            return (
              <div key={node.id} className="card" style={{ display: "flex", flexDirection: "column", gap: 8 }}>
                <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
                  {node.os === "macos" ? (
                    <Laptop size={19} />
                  ) : node.os === "linux" ? (
                    <Server size={19} />
                  ) : (
                    <Monitor size={19} />
                  )}
                  <div style={{ flex: 1, minWidth: 0 }}>
                    <div style={{ fontSize: 14.5, fontWeight: 700 }}>
                      {nodeDisplayName(node.id, nodes, hubInfo, pairing.host)}
                    </div>
                    <div className="mono" style={{ fontSize: 11.5, color: "var(--text-2)" }}>
                      {node.os}/{node.arch}
                      {node.version ? ` · v${node.version}` : ""}
                    </div>
                  </div>
                  <span className={`chip ${node.online ? "ok" : ""}`}>{node.online ? "在线" : "离线"}</span>
                </div>
                {installed.length > 0 && (
                  <div className="pill-wrap">
                    {installed.map((p) => (
                      <span key={p.name} className="chip mono">
                        {p.name}
                        {p.version ? ` ${p.version}` : ""}
                      </span>
                    ))}
                  </div>
                )}
              </div>
            );
          })
        )}

        {/* Subscription Quota */}
        <div className="section-label">订阅配额与用量</div>
        {!quota || quota.accounts.length === 0 ? (
          <div className="card" style={{ fontSize: 13, color: "var(--text-2)" }}>
            暂无订阅配额数据
          </div>
        ) : (
          quota.accounts.map((item) => {
            const windows: { group?: string | null; w: RateWindow }[] =
              item.status.limits.length > 0
                ? item.status.limits.flatMap((g) => g.windows.map((w) => ({ group: g.name || g.model, w })))
                : item.status.rate_limits.map((w) => ({ group: null, w }));

            return (
              <div key={item.account.id} className="card" style={{ display: "flex", flexDirection: "column", gap: 10 }}>
                <div style={{ display: "flex", alignItems: "center", gap: 6, flexWrap: "wrap" }}>
                  <strong style={{ fontSize: 14.5 }}>{item.account.label || item.account.id}</strong>
                  <span className="chip mono">{item.account.provider}</span>
                  {item.status.plan && <span className="chip accent">{item.status.plan}</span>}
                  <span style={{ flex: 1 }} />
                  {item.status.limit_reached && <span className="chip danger">已达上限</span>}
                  {!item.status.logged_in && <span className="chip warn">未登录</span>}
                </div>

                {item.status.email && (
                  <div style={{ fontSize: 12, color: "var(--text-2)", marginTop: -4 }}>{item.status.email}</div>
                )}

                {item.at_risk && (
                  <div className="chip danger" style={{ borderRadius: 8, padding: "6px 9px", whiteSpace: "normal" }}>
                    <ShieldAlert size={14} /> {item.at_risk}
                  </div>
                )}

                {windows.length === 0 ? (
                  <div style={{ fontSize: 12.5, color: "var(--text-2)" }}>
                    {item.status.credits?.has_credits
                      ? item.status.credits.unlimited
                        ? "无限额度"
                        : `余额：${item.status.credits.balance ?? "-"}`
                      : item.status.note || "暂无窗口配额数据"}
                  </div>
                ) : (
                  windows.map((entry, idx) => {
                    const used = Math.max(0, Math.min(100, entry.w.used_percent));
                    const remain = Math.max(0, Math.round(100 - used));
                    const color = used >= 90 ? "var(--danger)" : used >= 70 ? "var(--warn)" : "var(--ok)";
                    const reset = resetCountdown(entry.w.resets_at);
                    const title = [entry.group, entry.w.name].filter(Boolean).join(" · ");
                    return (
                      <div key={idx} style={{ display: "flex", flexDirection: "column", gap: 4 }}>
                        <div style={{ display: "flex", justifyContent: "space-between", fontSize: 12 }}>
                          <span style={{ color: "var(--text-2)" }}>{title}</span>
                          <span style={{ fontWeight: 700, color }}>
                            {entry.w.remaining !== undefined && entry.w.total
                              ? `余 ${Math.round(entry.w.remaining)} / ${Math.round(entry.w.total)}`
                              : `剩余 ${remain}%`}
                          </span>
                        </div>
                        <div className="progress-track">
                          <div className="progress-fill" style={{ width: `${used}%`, background: color }} />
                        </div>
                        {reset && <div style={{ fontSize: 11, color: "var(--text-3)" }}>{reset}</div>}
                      </div>
                    );
                  })
                )}
              </div>
            );
          })
        )}
      </div>
    </>
  );
}
