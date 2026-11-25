// 设置 → 设备 (docs/API-AUTH.md §2.2, §4): the credentials the hub has issued, and revoking
// one. Revoked devices stay in the list, faded and last, with when they were revoked. Reading
// the list is read; revoking is admin, so a credential too weak to revoke greys only the
// button and says which level is missing (the hub's own 403 message). The page and its entry
// stay as they are: the list is still worth seeing.

import { useEffect, useState } from "react";
import type { HubInfo } from "@bindings/HubInfo";
import type { Device } from "@bindings/Device";
import type { DeviceLevel } from "@bindings/DeviceLevel";
import type { RemoteNodeInfo } from "@bindings/RemoteNodeInfo";
import { ApiError, api, errorText } from "../api";
import { ErrorBox } from "../components";
import { onHubEvent, useHubData } from "../hub";
import { normalizeNodeOs, useNodes } from "../nodes";
import { isOnline, isRevoked, isThisWindow, KIND_LABELS, LEVEL_LABELS, OS_UNKNOWN_HINT, sortDevices } from "../lib/devices";
import { OsIcon, osName } from "../osIcon";
import { useConnectionState } from "../lib/useConnection";
import { clearRefusals, useOperation } from "../lib/features";
import { full, when } from "../lib/time";
import { useArmed } from "../parts";

export function DevicesPage() {
  // `device.created` / `device.updated` / `device.revoked` arrive for what another window (or
  // the CLI) did, so the list follows them without a reload.
  const devices = useHubData(
    () => api.listDevices().then((r) => r.devices),
    [],
    (e) => e.type === "device.created" || e.type === "device.updated" || e.type === "device.revoked",
  );
  // A 403 is kept by features.ts per operation (api.ts notes it): revoking or changing level
  // needs admin, while the list above only needs read. So this greys the controls, never the entry.
  const revoke = useOperation("devices.revoke");
  const refused = revoke.refused ? revoke.reason : undefined;
  const [error, setError] = useState<string>();
  const list = sortDevices(devices.data ?? []);
  const nodes = useNodes();
  const remoteNodes = (nodes.data ?? []).filter((n) => !n.local);
  const [hub, setHub] = useState<HubInfo>();
  useEffect(() => {
    api.info().then(setHub, () => {});
  }, []);
  const thisDevice = useConnectionState()?.remote?.deviceId;
  useEffect(() => {
    if (!thisDevice) return;
    return onHubEvent((e) => {
      if (e.type === "device.updated" && (e as { device?: { id?: string } }).device?.id === thisDevice) {
        clearRefusals();
      }
    });
  }, [thisDevice]);
  const now = Date.now();

  async function setDeviceLevel(device: Device, level: DeviceLevel) {
    if (level === device.level) return;
    setError(undefined);
    try {
      await api.setDeviceLevel(device.id, level);
      devices.reload();
    } catch (err) {
      if (!(err instanceof ApiError && err.status === 403)) setError(errorText(err));
    }
  }

  async function revokeDevice(device: Device) {
    setError(undefined);
    try {
      await api.revokeDevice(device.id);
      devices.reload();
    } catch (err) {
      // The 403's own reason is already on screen; anything else is an error to show.
      if (!(err instanceof ApiError && err.status === 403)) setError(errorText(err));
    }
  }

  return (
    <>
      <p className="settings-lead">
        这些是 hub 签发过的凭据。客户端按设备的级别访问 API：出差时如需在另一台自己的主力电脑上管理中心账号、开终端或审批扩展，可在中心把该设备升级为<strong>管理（admin）</strong>。
      </p>
      <div className="setting-card">
        <div className="setting-line device-row hub-row" data-role="hub">
          <div className="device-name">
            <div className="setting-label">
              <OsIcon os={hub?.machine?.os} size={16} />
              {hub?.machine?.hostname || "中心"}
              <span className="chip small ok">中心 · hub 在这里</span>
            </div>
            {!hub?.machine && <div className="setting-hint">{hub ? "这台 hub 还不报告所在机器" : "正在读取…"}</div>}
          </div>
        </div>
        {remoteNodes.length > 0 ? (
          remoteNodes.map((node) => <RemoteNodeRow key={node.id} node={node} />)
        ) : (
          <div className="setting-line">
            <div className="device-name">
              <div className="setting-label">
                远端执行节点
                <span className="chip small">0 在线</span>
              </div>
              <div className="setting-hint">
                在中心运行 <code>crossagents device add &lt;名字&gt; --kind node</code> 拿到凭据，再到 Mac / Linux / Windows 目标机运行{" "}
                <code>crossagents node run --hub &lt;hub地址&gt; --token &lt;token&gt;</code>，本机 agent 即可跨系统读写文件、执行命令。
              </div>
            </div>
          </div>
        )}
      </div>
      <ErrorBox error={devices.error ?? error} />
      {refused && (
        <div className="notice">
          调整设备权限或吊销设备需要「管理」（admin）级别，这个凭据不够，操作已置灰；列表照常能看。在中心电脑的「设备」页把当前设备升级为「管理」后即可操作。hub 说：{refused}
        </div>
      )}
      {devices.data === undefined ? null : !list.length ? (
        <div className="setting-card">
          <div className="setting-line">
            <div>
              <div className="setting-label">还没有设备</div>
              <div className="setting-hint">
                在 hub 所在的机器上运行 <code>crossagents device add &lt;名字&gt;</code>；从别的电脑连过来，在设置 →「连接」页配对，配对时会替你运行这条命令。
              </div>
            </div>
          </div>
        </div>
      ) : (
        <div className="setting-card">
          {list.map((device) => (
            <DeviceRow
              key={device.id}
              device={device}
              refused={refused}
              onLevel={setDeviceLevel}
              onRevoke={revokeDevice}
              now={now}
              thisWindow={isThisWindow(device, thisDevice)}
            />
          ))}
        </div>
      )}
    </>
  );
}

function RemoteNodeRow({ node }: { node: RemoteNodeInfo }) {
  const os = normalizeNodeOs(node.os);
  const installedPlugins = (node.plugins ?? []).filter((p) => p.installed);
  return (
    <div className={`setting-line device-row ${node.online ? "" : "revoked"}`}>
      <div className="device-name">
        <div className="setting-label">
          <OsIcon os={os} size={15} />
          {node.name}
          <span className={`chip small ${node.online ? "ok" : ""}`}>{node.online ? "在线节点" : "离线节点"}</span>
          {node.os && node.os !== "unknown" && (
            <span className="chip small">{node.arch ? `${node.os}/${node.arch}` : node.os}</span>
          )}
          {node.shell && <span className="chip small">{node.shell}</span>}
          {installedPlugins.map((p) => (
            <span key={p.name} className="chip small accent" title={p.models.join(", ") || p.name}>
              {p.name}
              {p.models.length > 0 ? ` · ${p.models.length}` : ""}
            </span>
          ))}
        </div>
        <div className="device-times">
          <span className="setting-hint">主机 {node.hostname}</span>
          {node.default_cwd && <span className="setting-hint">目录 {node.default_cwd}</span>}
          {node.version && <span className="setting-hint">v{node.version}</span>}
          <span className="setting-hint" title={`${node.online ? "连接于" : "最后在线"} ${full(node.connected_at)}`}>
            {node.online ? "连接" : "最后在线"} {when(node.connected_at)}
          </span>
        </div>
      </div>
    </div>
  );
}

/** One device: what it is, what it may do, when it was made and last seen; revoked ones faint. */
function DeviceRow({
  device,
  refused,
  onLevel,
  onRevoke,
  now,
  thisWindow,
}: {
  device: Device;
  refused?: string;
  onLevel: (device: Device, level: DeviceLevel) => Promise<void>;
  onRevoke: (device: Device) => void;
  now: number;
  thisWindow: boolean;
}) {
  const remove = useArmed();
  const [savingLevel, setSavingLevel] = useState(false);
  const revoked = isRevoked(device);
  const changeLevel = async (next: DeviceLevel) => {
    setSavingLevel(true);
    try {
      await onLevel(device, next);
    } finally {
      setSavingLevel(false);
    }
  };
  return (
    <div className={`setting-line device-row ${revoked ? "revoked" : ""}`}>
      <div className="device-name">
        <div className="setting-label">
          <span title={device.os ? osName(device.os) : OS_UNKNOWN_HINT}>
            <OsIcon os={device.os} size={15} />
          </span>
          {device.name}
          {thisWindow && <span className="chip small accent">这个窗口</span>}
          <span className="chip small">{KIND_LABELS[device.kind]}</span>
          <span className={`chip small ${device.level === "admin" ? "accent" : ""}`}>{LEVEL_LABELS[device.level]}</span>
          {revoked && <span className="chip small danger">已吊销</span>}
        </div>
        {!device.os && <div className="setting-hint">{OS_UNKNOWN_HINT}</div>}
        <div className="device-times">
          {!revoked && (
            <span className="setting-hint device-presence" data-online={isOnline(device, now)}>
              <span className={`dot ${isOnline(device, now) ? "ok" : ""}`} />
              {isOnline(device, now) ? "在线" : device.last_seen_at ? `离线 · ${when(device.last_seen_at)}` : "离线 · 从未连接"}
            </span>
          )}
          <span className="setting-hint" title={`创建于 ${full(device.created_at)}`}>
            创建 {when(device.created_at)}
          </span>
          <span className="setting-hint" title={device.last_seen_at ? `最后在线 ${full(device.last_seen_at)}` : "还没有连接过"}>
            最后在线 {device.last_seen_at ? when(device.last_seen_at) : "从未连接"}
          </span>
          {revoked && device.revoked_at && (
            <span className="setting-hint" title={`吊销于 ${full(device.revoked_at)}`}>
              吊销于 {when(device.revoked_at)}
            </span>
          )}
        </div>
      </div>
      {!revoked && (
        <div className="row device-actions">
          {device.kind === "client" && (
            <select
              className="device-level-select"
              aria-label={`调整 ${device.name} 权限级别`}
              value={device.level}
              disabled={!!refused || savingLevel}
              title={refused ?? "切换权限级别（管理级别可远程管账号、开终端、审批扩展）"}
              onChange={(e) => void changeLevel(e.target.value as DeviceLevel)}
            >
              <option value="read">只读（read）</option>
              <option value="write">读写（write）</option>
              <option value="admin">管理（admin）</option>
            </select>
          )}
          <button
            type="button"
            data-action="revoke"
            className={`ghost small danger ${remove.armed ? "armed" : ""}`}
            disabled={!!refused}
            title={refused ?? "这台设备的凭据立即失效；记录留着"}
            onClick={() => remove.click(() => onRevoke(device))}
          >
            {remove.armed ? "再点一次吊销" : "吊销"}
          </button>
        </div>
      )}
    </div>
  );
}
