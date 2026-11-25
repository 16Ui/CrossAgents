import { useEffect, useState } from "react";
import { ArrowUpLeft, ChevronRight, Folder, X } from "lucide-react";
import { availableEfforts, extractErrorMessage, nodeDisplayName } from "../store";
import type { HubTransport } from "../transport";
import type { Agent, HubInfo, NodeDirsResponse, PluginInfo, RemoteNodeInfo } from "../types";

export function NewChatModal({
  transport,
  nodes,
  hubInfo,
  fallbackHost,
  onClose,
  onCreated,
}: {
  transport: HubTransport;
  nodes: RemoteNodeInfo[];
  hubInfo?: HubInfo;
  fallbackHost?: string;
  onClose: () => void;
  onCreated: (agent: Agent) => void;
}) {
  const onlineNodes = nodes.filter((n) => n.online);
  const defaultNode = onlineNodes.find((n) => n.local)?.id ?? onlineNodes[0]?.id ?? "local";

  const [nodeId, setNodeId] = useState(defaultNode);
  const [cwd, setCwd] = useState("");
  const [plugins, setPlugins] = useState<PluginInfo[]>([]);
  const [pluginName, setPluginName] = useState("");
  const [model, setModel] = useState("");
  const [effort, setEffort] = useState("");
  const [firstMessage, setFirstMessage] = useState("");

  const [loadingPlugins, setLoadingPlugins] = useState(false);
  const [creating, setCreating] = useState(false);
  const [browsingDirs, setBrowsingDirs] = useState(false);
  const [dirData, setDirData] = useState<NodeDirsResponse | null>(null);
  const [dirPathInput, setDirPathInput] = useState("");
  const [dirLoading, setDirLoading] = useState(false);
  const [error, setError] = useState<string>();

  useEffect(() => {
    let cancelled = false;
    setLoadingPlugins(true);
    setError(undefined);
    const query = nodeId && nodeId !== "local" ? `?node=${encodeURIComponent(nodeId)}` : "";
    transport
      .request("GET", `/v1/plugins${query}`)
      .then((res) => {
        if (cancelled) return;
        if (res.status >= 400) throw new Error(extractErrorMessage(res.status, res.body));
        const parsed = JSON.parse(res.body) as { plugins: PluginInfo[] };
        const installed = parsed.plugins.filter((p) => p.cli?.installed !== false);
        const list = installed.length ? installed : parsed.plugins;
        setPlugins(list);
        const first = list[0];
        setPluginName(first?.name ?? "");
        const models = first ? (first.models.length ? first.models : first.all_models) : [];
        setModel(models[0] ?? "");
        setEffort("");
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (!cancelled) setLoadingPlugins(false);
      });
    return () => {
      cancelled = true;
    };
  }, [transport, nodeId]);

  const currentPlugin = plugins.find((p) => p.name === pluginName);
  const models = currentPlugin ? (currentPlugin.models.length ? currentPlugin.models : currentPlugin.all_models) : [];
  const efforts = availableEfforts(currentPlugin, model);

  async function loadDir(path: string) {
    setDirLoading(true);
    setError(undefined);
    try {
      const q = path ? `?path=${encodeURIComponent(path)}` : "";
      const res = await transport.request("GET", `/v1/nodes/${encodeURIComponent(nodeId)}/dirs${q}`);
      if (res.status >= 400) throw new Error(extractErrorMessage(res.status, res.body));
      const parsed = JSON.parse(res.body) as NodeDirsResponse;
      setDirData(parsed);
      setDirPathInput(parsed.path);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setDirLoading(false);
    }
  }

  async function create() {
    if (!pluginName || creating) return;
    setCreating(true);
    setError(undefined);
    try {
      const trimmedMsg = firstMessage.trim();
      const body = {
        plugin: pluginName,
        driver: null,
        model: model || null,
        cwd: cwd.trim(),
        topic: trimmedMsg ? trimmedMsg.slice(0, 48) : undefined,
        config: effort ? { effort } : {},
        profile: {
          display_name: null,
          title: null,
          avatar: null,
          avatar_image: null,
          color: null,
          persona: null,
        },
        ...(nodeId && nodeId !== "local" ? { node: nodeId } : {}),
      };
      const res = await transport.request("POST", "/v1/agents", body);
      if (res.status >= 400) throw new Error(extractErrorMessage(res.status, res.body));
      const created = JSON.parse(res.body) as Agent;
      if (trimmedMsg) {
        await transport.request("POST", `/v1/agents/${encodeURIComponent(created.id)}/messages`, {
          text: trimmedMsg,
          client_message_id: crypto.randomUUID(),
        });
      }
      onCreated(created);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setCreating(false);
    }
  }

  return (
    <div className="sheet-backdrop" onClick={onClose}>
      <div className="sheet-panel" onClick={(e) => e.stopPropagation()}>
        <div className="sheet-head">
          <strong>{browsingDirs ? `浏览目录 · ${nodeDisplayName(nodeId, nodes, hubInfo, fallbackHost)}` : "新建对话"}</strong>
          <button type="button" className="icon-btn" onClick={() => (browsingDirs ? setBrowsingDirs(false) : onClose())}>
            <X size={18} />
          </button>
        </div>

        <div className="sheet-body">
          {error && (
            <div className="chip danger" style={{ borderRadius: 10, padding: "8px 10px", marginBottom: 12, display: "block" }}>
              {error}
            </div>
          )}

          {browsingDirs ? (
            <div style={{ display: "flex", flexDirection: "column", gap: 10 }}>
              <div style={{ display: "flex", gap: 6 }}>
                <input
                  className="field-input mono"
                  style={{ fontSize: 12.5, padding: "8px 10px" }}
                  value={dirPathInput}
                  onChange={(e) => setDirPathInput(e.target.value)}
                  placeholder="输入绝对路径跳转…"
                />
                <button type="button" className="secondary-btn" onClick={() => void loadDir(dirPathInput.trim())}>
                  跳转
                </button>
              </div>

              <div className="agent-list">
                {dirData?.parent && (
                  <button type="button" className="agent-row" onClick={() => void loadDir(dirData.parent!)}>
                    <ArrowUpLeft size={16} />
                    <span>返回上级目录</span>
                  </button>
                )}
                {dirLoading && !dirData ? (
                  <div style={{ padding: 16, fontSize: 13, color: "var(--text-2)" }}>读取目录中…</div>
                ) : dirData && dirData.dirs.length === 0 ? (
                  <div style={{ padding: 16, fontSize: 13, color: "var(--text-2)" }}>该目录下没有子文件夹</div>
                ) : (
                  dirData?.dirs.map((d) => (
                    <button key={d.path} type="button" className="agent-row" onClick={() => void loadDir(d.path)}>
                      <Folder size={16} style={{ color: "var(--accent)", flex: "none" }} />
                      <span style={{ flex: 1, overflow: "hidden", textOverflow: "ellipsis" }}>{d.name}</span>
                      <ChevronRight size={15} style={{ color: "var(--text-3)" }} />
                    </button>
                  ))
                )}
              </div>

              <div style={{ display: "flex", gap: 8 }}>
                <button
                  type="button"
                  className="secondary-btn"
                  style={{ flex: 1 }}
                  onClick={() => {
                    setCwd("");
                    setBrowsingDirs(false);
                  }}
                >
                  用默认对话目录
                </button>
                <button
                  type="button"
                  className="primary-btn"
                  style={{ flex: 1 }}
                  onClick={() => {
                    setCwd(dirData?.path || dirPathInput.trim());
                    setBrowsingDirs(false);
                  }}
                >
                  选定此目录
                </button>
              </div>
            </div>
          ) : (
            <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
              <div>
                <div style={{ fontSize: 12, color: "var(--text-2)", marginBottom: 5 }}>目标机器</div>
                <select
                  className="field-select"
                  value={nodeId}
                  onChange={(e) => {
                    setNodeId(e.target.value);
                    setCwd("");
                  }}
                >
                  {onlineNodes.length === 0 ? (
                    <option value="local">{nodeDisplayName("local", nodes, hubInfo, fallbackHost)}</option>
                  ) : (
                    onlineNodes.map((n) => (
                      <option key={n.id} value={n.id}>
                        {nodeDisplayName(n.id, nodes, hubInfo, fallbackHost)}
                      </option>
                    ))
                  )}
                </select>
              </div>

              <div>
                <div style={{ fontSize: 12, color: "var(--text-2)", marginBottom: 5 }}>工作目录</div>
                <button
                  type="button"
                  className="field-input"
                  style={{ display: "flex", alignItems: "center", justifyContent: "space-between", textAlign: "left" }}
                  onClick={() => {
                    setBrowsingDirs(true);
                    void loadDir(cwd);
                  }}
                >
                  <span className="mono" style={{ fontSize: 13, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                    {cwd || "默认对话目录（点击浏览远程文件夹）"}
                  </span>
                  <ChevronRight size={16} style={{ color: "var(--text-3)", flex: "none" }} />
                </button>
              </div>

              <div>
                <div style={{ fontSize: 12, color: "var(--text-2)", marginBottom: 5 }}>CLI / 引擎</div>
                {loadingPlugins ? (
                  <div style={{ fontSize: 13, color: "var(--text-2)" }}>正在读取目标机器的可用 CLI…</div>
                ) : (
                  <select
                    className="field-select"
                    value={pluginName}
                    onChange={(e) => {
                      const next = e.target.value;
                      setPluginName(next);
                      const p = plugins.find((x) => x.name === next);
                      const mList = p ? (p.models.length ? p.models : p.all_models) : [];
                      setModel(mList[0] ?? "");
                      setEffort("");
                    }}
                  >
                    {plugins.map((p) => (
                      <option key={p.name} value={p.name}>
                        {p.name}
                      </option>
                    ))}
                  </select>
                )}
              </div>

              {models.length > 0 && (
                <div>
                  <div style={{ fontSize: 12, color: "var(--text-2)", marginBottom: 5 }}>模型</div>
                  <select className="field-select" value={model} onChange={(e) => setModel(e.target.value)}>
                    <option value="">默认模型</option>
                    {models.map((m) => (
                      <option key={m} value={m}>
                        {m}
                      </option>
                    ))}
                  </select>
                </div>
              )}

              {efforts.length > 0 && (
                <div>
                  <div style={{ fontSize: 12, color: "var(--text-2)", marginBottom: 5 }}>推理力度</div>
                  <select className="field-select" value={effort} onChange={(e) => setEffort(e.target.value)}>
                    <option value="">默认</option>
                    {efforts.map((ef) => (
                      <option key={ef} value={ef}>
                        {ef}
                      </option>
                    ))}
                  </select>
                </div>
              )}

              <div>
                <div style={{ fontSize: 12, color: "var(--text-2)", marginBottom: 5 }}>开场消息（可选）</div>
                <textarea
                  className="field-textarea"
                  rows={3}
                  placeholder="可直接输入第一条指令，创建后自动发送…"
                  value={firstMessage}
                  onChange={(e) => setFirstMessage(e.target.value)}
                />
              </div>

              <button type="button" className="primary-btn" disabled={creating || !pluginName} onClick={() => void create()}>
                {creating ? "创建中…" : "创建并进入对话"}
              </button>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
