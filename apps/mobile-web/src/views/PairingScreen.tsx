import { useEffect, useState } from "react";
import { Activity, Clipboard, Globe, QrCode, Share, ShieldCheck } from "lucide-react";
import { BrandMark } from "../brands";
import { isIOS, isStandalonePwa } from "../push";
import { decodeInviteLocal, redeemInviteWasm, runRelayDiagnostics, type RelayDiagItem } from "../transport";
import type { InvitePreview, PairingRecord } from "../types";
import { QrScannerModal } from "../components/QrScannerModal";

function relayHostLabel(url?: string | null): string {
  if (!url) return "";
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

export function PairingScreen({ onPaired }: { onPaired: (record: PairingRecord) => void }) {
  const [mode, setMode] = useState<"iroh" | "direct">("iroh");
  const [code, setCode] = useState("");
  const [deviceName, setDeviceName] = useState("我的 iPhone");
  const [preview, setPreview] = useState<InvitePreview | null>(null);
  const [scanning, setScanning] = useState(false);

  const [directUrl, setDirectUrl] = useState("");
  const [directToken, setDirectToken] = useState("");

  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const [diagBusy, setDiagBusy] = useState(false);
  const [diagItems, setDiagItems] = useState<RelayDiagItem[] | null>(null);

  const needsHomeScreen = isIOS() && !isStandalonePwa();

  useEffect(() => {
    if (!code.trim()) {
      setPreview(null);
      return;
    }
    let cancelled = false;
    void decodeInviteLocal(code).then((res) => {
      if (!cancelled) setPreview(res);
    });
    return () => {
      cancelled = true;
    };
  }, [code]);

  const expired = Boolean(preview?.ok && preview.expires && preview.expires * 1000 <= Date.now());
  const canPairIroh = !busy && Boolean(preview?.ok) && !expired;

  async function pairByCode(rawCode?: string) {
    const targetCode = (rawCode ?? code).trim();
    if (!targetCode || busy) return;
    setBusy(true);
    setError(undefined);
    try {
      const record = await redeemInviteWasm(targetCode, deviceName);
      onPaired(record);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  function pairDirect() {
    const url = directUrl.trim().replace(/\/+$/, "");
    const token = directToken.trim();
    if (!url.startsWith("https://") && !url.startsWith("http://")) {
      setError("请输入完整的 https:// 中心地址");
      return;
    }
    if (!token) {
      setError("请输入设备 Token（cad_...）");
      return;
    }
    let host = url;
    try {
      host = new URL(url).host;
    } catch {
      // ignore
    }
    onPaired({
      mode: "direct",
      baseUrl: url,
      host,
      deviceId: "direct-client",
      deviceName: deviceName.trim() || "我的 iPhone",
      token,
      pairedAt: new Date().toISOString(),
    });
  }

  return (
    <div className="app-shell">
      <div className="page-scroll" style={{ paddingTop: "calc(var(--safe-top) + 26px)" }}>
        <div
          style={{
            display: "flex",
            flexDirection: "column",
            alignItems: "center",
            textAlign: "center",
            marginBottom: 22,
          }}
        >
          <img
            src="./icon-192.png"
            alt="CrossAgents"
            style={{
              width: 64,
              height: 64,
              borderRadius: 16,
              border: "1px solid var(--border)",
            }}
          />
          <div style={{ display: "inline-flex", alignItems: "center", gap: 7, marginTop: 12 }}>
            <BrandMark size={20} />
            <h1 style={{ margin: 0, fontSize: 23, fontWeight: 700, letterSpacing: "-0.02em" }}>CrossAgents</h1>
          </div>
          <p style={{ margin: "5px 0 0", fontSize: 13, color: "var(--muted)" }}>
            端到端加密远程控制你的多机 Agent 团队 · 零 VPN
          </p>
        </div>

        {needsHomeScreen && (
          <div
            className="card"
            style={{ borderColor: "color-mix(in srgb, var(--accent) 40%, var(--border))", background: "var(--accent-bg)" }}
          >
            <div style={{ display: "flex", gap: 10, alignItems: "flex-start" }}>
              <Share size={17} style={{ color: "var(--accent-text)", flex: "none", marginTop: 2 }} />
              <div style={{ fontSize: 12.5, lineHeight: 1.5 }}>
                <strong style={{ color: "var(--text)" }}>建议先「添加到主屏幕」再配对</strong>
                <div style={{ color: "var(--text-2)", marginTop: 2 }}>
                  点 Safari 底部「分享」→「添加到主屏幕」，从主屏图标打开可获全屏体验并开启免费 iOS 锁屏推送。
                </div>
              </div>
            </div>
          </div>
        )}

        <div className="segmented" style={{ marginBottom: 12 }}>
          <button type="button" className={mode === "iroh" ? "active" : ""} onClick={() => setMode("iroh")}>
            扫码 / 配对码（推荐）
          </button>
          <button type="button" className={mode === "direct" ? "active" : ""} onClick={() => setMode("direct")}>
            HTTPS 直连（备选）
          </button>
        </div>

        {mode === "iroh" ? (
          <>
            <div className="card" style={{ display: "flex", flexDirection: "column", gap: 10 }}>
              <button type="button" className="primary-btn" disabled={busy} onClick={() => setScanning(true)}>
                <QrCode size={18} />
                扫描电脑端配对二维码
              </button>
              <div style={{ fontSize: 12, color: "var(--muted)", textAlign: "center", lineHeight: 1.5 }}>
                中心电脑打开 CrossAgents → 设置 →「连接」→「生成配对码」
              </div>
            </div>

            <div className="card" style={{ display: "flex", flexDirection: "column", gap: 11 }}>
              <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
                <span style={{ fontSize: 13.5, fontWeight: 600 }}>或粘贴一次性配对码</span>
                <button
                  type="button"
                  className="secondary-btn"
                  style={{ padding: "4px 10px", fontSize: 12 }}
                  disabled={busy}
                  onClick={async () => {
                    try {
                      const text = await navigator.clipboard.readText();
                      if (text) {
                        setCode(text.trim());
                        setError(undefined);
                      }
                    } catch {
                      setError("浏览器未授予剪贴板读取权限，请长按输入框手动粘贴。");
                    }
                  }}
                >
                  <Clipboard size={12} />
                  粘贴
                </button>
              </div>

              <textarea
                className="field-textarea mono"
                rows={3}
                placeholder="ca1-…"
                value={code}
                onChange={(e) => {
                  setCode(e.target.value);
                  setError(undefined);
                }}
                disabled={busy}
                style={{ fontSize: 12 }}
              />

              {preview && (
                <div
                  className={`chip ${preview.ok && !expired ? "ok" : "danger"}`}
                  style={{ borderRadius: 9, padding: "7px 10px", whiteSpace: "normal", lineHeight: 1.45 }}
                >
                  <ShieldCheck size={14} style={{ flex: "none" }} />
                  <span>
                    {preview.ok
                      ? expired
                        ? `中心 ${preview.host} 的配对码已过期，请重新生成`
                        : `目标中心：${preview.host} · ${
                            preview.relay
                              ? `中继 ${relayHostLabel(preview.relay)}`
                              : preview.has_relay
                                ? "端到端加密中继可用"
                                : "自动选择加密中继"
                          }`
                      : preview.error}
                  </span>
                </div>
              )}

              <div>
                <div style={{ fontSize: 12, color: "var(--muted)", marginBottom: 5 }}>这台手机的设备名</div>
                <input
                  className="field-input"
                  value={deviceName}
                  onChange={(e) => setDeviceName(e.target.value)}
                  maxLength={48}
                  disabled={busy}
                />
              </div>

              {error && <div className="error-box">{error}</div>}

              <button type="button" className="primary-btn" disabled={!canPairIroh} onClick={() => void pairByCode()}>
                {busy ? "正在建立加密配对…" : "配对并连接"}
              </button>

              <button
                type="button"
                className="ghost-btn"
                style={{ width: "100%" }}
                disabled={diagBusy}
                onClick={async () => {
                  setDiagBusy(true);
                  try {
                    setDiagItems(await runRelayDiagnostics(preview?.relay));
                  } finally {
                    setDiagBusy(false);
                  }
                }}
              >
                <Activity size={13} />
                {diagBusy ? "正在测试中继连通性…" : "测试中继连通性"}
              </button>

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
          </>
        ) : (
          <div className="card" style={{ display: "flex", flexDirection: "column", gap: 11 }}>
            <div style={{ fontSize: 12.5, color: "var(--muted)", lineHeight: 1.5 }}>
              如果你配置了 Tailscale Funnel 或自建反代 HTTPS 域名，可以在此直接填写地址与设备 Token。
            </div>
            <div>
              <div style={{ fontSize: 12, color: "var(--muted)", marginBottom: 5 }}>中心 HTTPS 地址</div>
              <input
                className="field-input mono"
                placeholder="https://desktop.tailnet.ts.net"
                value={directUrl}
                onChange={(e) => setDirectUrl(e.target.value)}
                style={{ fontSize: 13.5 }}
              />
            </div>
            <div>
              <div style={{ fontSize: 12, color: "var(--muted)", marginBottom: 5 }}>设备 Token</div>
              <input
                className="field-input mono"
                type="password"
                placeholder="cad_..."
                value={directToken}
                onChange={(e) => setDirectToken(e.target.value)}
                style={{ fontSize: 13.5 }}
              />
            </div>
            {error && <div className="error-box">{error}</div>}
            <button type="button" className="primary-btn" onClick={pairDirect}>
              <Globe size={16} />
              连接中心
            </button>
          </div>
        )}
      </div>

      {scanning && (
        <QrScannerModal
          onClose={() => setScanning(false)}
          onScanned={(scanned) => {
            setScanning(false);
            setCode(scanned);
            setError(undefined);
            void pairByCode(scanned);
          }}
        />
      )}
    </div>
  );
}
