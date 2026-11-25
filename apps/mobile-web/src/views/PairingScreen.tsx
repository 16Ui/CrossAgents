import { useEffect, useState } from "react";
import { Activity, Clipboard, Globe, QrCode, Share, ShieldCheck } from "lucide-react";
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

  // Direct HTTPS fallback (e.g. Tailscale Funnel / custom domain)
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
      <div className="page-scroll" style={{ paddingTop: "calc(var(--safe-top) + 22px)" }}>
        <div style={{ textAlign: "center", marginBottom: 22 }}>
          <img
            src="./icon-192.png"
            alt="CrossAgents"
            style={{
              width: 68,
              height: 68,
              borderRadius: 18,
              boxShadow: "0 8px 24px rgba(79, 70, 229, 0.28)",
            }}
          />
          <h1 style={{ margin: "12px 0 4px", fontSize: 26, fontWeight: 800 }}>CrossAgents</h1>
          <p style={{ margin: 0, fontSize: 13.5, color: "var(--text-2)" }}>
            浏览器 WASM iroh 端到端加密直连 · 两端零 VPN · 永不过期
          </p>
        </div>

        {needsHomeScreen && (
          <div className="card" style={{ borderColor: "var(--accent)", background: "var(--accent-soft)" }}>
            <div style={{ display: "flex", gap: 10, alignItems: "flex-start" }}>
              <Share size={18} style={{ color: "var(--accent)", flex: "none", marginTop: 2 }} />
              <div style={{ fontSize: 13, lineHeight: 1.5 }}>
                <strong>建议先「添加到主屏幕」再扫码配对</strong>
                <div style={{ color: "var(--text-2)", marginTop: 2 }}>
                  点击 Safari 底部「分享」→「添加到主屏幕」，然后从桌面图标打开，可获得全屏 App 体验并解锁<strong>免费 iOS 锁屏审批推送</strong>。
                </div>
              </div>
            </div>
          </div>
        )}

        <div className="segmented">
          <button type="button" className={mode === "iroh" ? "active" : ""} onClick={() => setMode("iroh")}>
            扫码 / 配对码（推荐）
          </button>
          <button type="button" className={mode === "direct" ? "active" : ""} onClick={() => setMode("direct")}>
            HTTPS 直连（备选）
          </button>
        </div>

        {mode === "iroh" ? (
          <>
            <div className="card" style={{ display: "flex", flexDirection: "column", gap: 12 }}>
              <button type="button" className="primary-btn" disabled={busy} onClick={() => setScanning(true)}>
                <QrCode size={19} />
                扫描电脑端配对二维码
              </button>
              <div style={{ fontSize: 12, color: "var(--text-2)", textAlign: "center", lineHeight: 1.5 }}>
                在中心电脑打开 CrossAgents → 设置 →「连接」→「让别的电脑连进来」→「生成配对码」
              </div>
            </div>

            <div className="card" style={{ display: "flex", flexDirection: "column", gap: 12 }}>
              <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
                <span style={{ fontSize: 13.5, fontWeight: 600 }}>或粘贴一次性配对码</span>
                <button
                  type="button"
                  className="secondary-btn"
                  style={{ padding: "5px 10px", fontSize: 12 }}
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
                  <Clipboard size={13} />
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
                style={{ fontSize: 12.5 }}
              />

              {preview && (
                <div
                  className={`chip ${preview.ok && !expired ? "ok" : "danger"}`}
                  style={{ borderRadius: 10, padding: "8px 10px", display: "flex", alignItems: "center", gap: 6 }}
                >
                  <ShieldCheck size={15} />
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
                <div style={{ fontSize: 12, color: "var(--text-2)", marginBottom: 5 }}>这台手机的设备名</div>
                <input
                  className="field-input"
                  value={deviceName}
                  onChange={(e) => setDeviceName(e.target.value)}
                  maxLength={48}
                  disabled={busy}
                />
              </div>

              {error && (
                <div
                  className="chip danger"
                  style={{ borderRadius: 10, padding: "9px 11px", whiteSpace: "normal", lineHeight: 1.5, wordBreak: "break-word" }}
                >
                  {error}
                </div>
              )}

              <button
                type="button"
                className="primary-btn"
                disabled={!canPairIroh}
                onClick={() => void pairByCode()}
              >
                {busy ? "正在通过 iroh 建立加密配对…" : "配对并连接"}
              </button>

              <button
                type="button"
                className="secondary-btn"
                style={{ justifyContent: "center", fontSize: 12.5, padding: "8px 12px" }}
                disabled={diagBusy}
                onClick={async () => {
                  setDiagBusy(true);
                  try {
                    const res = await runRelayDiagnostics(preview?.relay);
                    setDiagItems(res);
                  } finally {
                    setDiagBusy(false);
                  }
                }}
              >
                <Activity size={14} />
                {diagBusy ? "正在测试中继连通性…" : "测试中继连通性"}
              </button>

              {diagItems && (
                <div
                  style={{
                    borderRadius: 10,
                    padding: "9px 11px",
                    background: "var(--surface-2)",
                    fontSize: 11.5,
                    lineHeight: 1.55,
                    display: "flex",
                    flexDirection: "column",
                    gap: 6,
                  }}
                >
                  <div style={{ fontWeight: 600, color: "var(--text-2)" }}>浏览器中继连通性诊断</div>
                  {diagItems.map((item) => (
                    <div key={item.url} className="mono" style={{ wordBreak: "break-all" }}>
                      <div>
                        <strong>{item.host}</strong>
                      </div>
                      <div style={{ color: item.httpOk ? "var(--ok)" : "var(--danger)" }}>
                        HTTPS /ping: {item.httpOk ? "OK" : "失败"} ({item.httpMs}ms) {item.httpDetail}
                      </div>
                      <div style={{ color: item.wsOk ? "var(--ok)" : "var(--danger)" }}>
                        WSS /relay: {item.wsOk ? "OK" : "失败"} ({item.wsMs}ms) {item.wsDetail}
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </div>
          </>
        ) : (
          <div className="card" style={{ display: "flex", flexDirection: "column", gap: 12 }}>
            <div style={{ fontSize: 13, color: "var(--text-2)", lineHeight: 1.5 }}>
              如果你配置了 Tailscale Funnel 或自建反代 HTTPS 域名，可以在此直接填写地址与设备 Token。
            </div>
            <div>
              <div style={{ fontSize: 12, color: "var(--text-2)", marginBottom: 5 }}>中心 HTTPS 地址</div>
              <input
                className="field-input mono"
                placeholder="https://desktop.tailnet.ts.net"
                value={directUrl}
                onChange={(e) => setDirectUrl(e.target.value)}
                style={{ fontSize: 13.5 }}
              />
            </div>
            <div>
              <div style={{ fontSize: 12, color: "var(--text-2)", marginBottom: 5 }}>设备 Token</div>
              <input
                className="field-input mono"
                type="password"
                placeholder="cad_..."
                value={directToken}
                onChange={(e) => setDirectToken(e.target.value)}
                style={{ fontSize: 13.5 }}
              />
            </div>
            {error && (
              <div className="chip danger" style={{ borderRadius: 10, padding: "9px 11px", whiteSpace: "normal" }}>
                {error}
              </div>
            )}
            <button type="button" className="primary-btn" onClick={pairDirect}>
              <Globe size={17} />
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
