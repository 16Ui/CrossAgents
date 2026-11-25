import { useMemo, useState } from "react";
import { Bell, Folder, Laptop, Monitor, Plus, RefreshCw, Search, Server } from "lucide-react";
import { nodeDisplayName, relativeTime, shortWorkspace } from "../store";
import type { Agent, HubInfo, HumanItem, RemoteNodeInfo } from "../types";

const PALETTE = ["#4f46e5", "#2563eb", "#0d9488", "#ea580c", "#db2777", "#7c3aed"];

function avatarBg(agent: Agent): string {
  const c = (agent.profile?.color ?? "").toLowerCase();
  if (c === "blue" || c === "sky") return "#2563eb";
  if (c === "green" || c === "emerald" || c === "mint") return "#16a34a";
  if (c === "amber" || c === "orange" || c === "yellow") return "#d97706";
  if (c === "rose" || c === "red" || c === "pink") return "#e11d48";
  if (c === "purple" || c === "violet" || c === "indigo") return "#4f46e5";
  if (c === "teal" || c === "cyan") return "#0d9488";
  let h = 0;
  for (let i = 0; i < agent.name.length; i++) h = (h * 31 + agent.name.charCodeAt(i)) | 0;
  return PALETTE[Math.abs(h) % PALETTE.length];
}

function statusRank(s: Agent["status"]): number {
  if (s === "running") return 0;
  if (s === "starting") return 1;
  if (s === "idle") return 2;
  if (s === "error") return 3;
  return 4;
}

export function HomeChatsView({
  agents,
  nodes,
  hubInfo,
  fallbackHost,
  openQuestions,
  avatarUrls,
  snapshotSavedAt,
  isConnected,
  refreshing,
  onRefresh,
  onOpenAgent,
  onOpenInbox,
  onOpenNewChat,
}: {
  agents: Agent[];
  nodes: RemoteNodeInfo[];
  hubInfo?: HubInfo;
  fallbackHost?: string;
  openQuestions: HumanItem[];
  avatarUrls: Record<string, string>;
  snapshotSavedAt?: string;
  isConnected: boolean;
  refreshing: boolean;
  onRefresh: () => void;
  onOpenAgent: (agentId: string) => void;
  onOpenInbox: () => void;
  onOpenNewChat: () => void;
}) {
  const [query, setQuery] = useState("");
  const [showArchived, setShowArchived] = useState(false);

  const pendingByAgent = useMemo(() => {
    const map = new Map<string, number>();
    for (const q of openQuestions) {
      map.set(q.from, (map.get(q.from) ?? 0) + 1);
    }
    return map;
  }, [openQuestions]);

  const groups = useMemo(() => {
    const q = query.trim().toLowerCase();
    const filtered = agents.filter((a) => {
      if (!showArchived && a.archived_at) return false;
      if (!q) return true;
      const nName = nodeDisplayName(a.node, nodes, hubInfo, fallbackHost).toLowerCase();
      return (
        a.name.toLowerCase().includes(q) ||
        (a.topic ?? "").toLowerCase().includes(q) ||
        shortWorkspace(a).toLowerCase().includes(q) ||
        (a.model ?? "").toLowerCase().includes(q) ||
        a.plugin.toLowerCase().includes(q) ||
        nName.includes(q)
      );
    });

    const buckets = new Map<string, Agent[]>();
    for (const a of filtered) {
      const key = (a.node ?? "").trim() || "local";
      const list = buckets.get(key) ?? [];
      list.push(a);
      buckets.set(key, list);
    }

    return Array.from(buckets.entries())
      .map(([nodeKey, list]) => {
        const info = nodes.find((n) => (nodeKey === "local" ? n.local : n.id === nodeKey || n.name === nodeKey || n.hostname === nodeKey));
        const isLocal = nodeKey === "local" || Boolean(info?.local);
        list.sort((a, b) => {
          const r = statusRank(a.status) - statusRank(b.status);
          if (r !== 0) return r;
          return (b.created_at || "").localeCompare(a.created_at || "");
        });
        return {
          key: nodeKey,
          title: nodeDisplayName(nodeKey, nodes, hubInfo, fallbackHost),
          os: info?.os ?? (isLocal ? hubInfo?.machine?.os ?? "windows" : "macos"),
          online: info ? info.online : isLocal,
          isLocal,
          agents: list,
        };
      })
      .sort((a, b) => {
        if (a.isLocal !== b.isLocal) return a.isLocal ? -1 : 1;
        return a.title.localeCompare(b.title);
      });
  }, [agents, nodes, hubInfo, fallbackHost, query, showArchived]);

  return (
    <>
      <div className="nav-bar">
        <div className="nav-title-wrap">
          <h1 className="nav-title">对话</h1>
          {!isConnected && snapshotSavedAt && (
            <div className="nav-subtitle">展示缓存快照（{relativeTime(snapshotSavedAt)}）</div>
          )}
        </div>
        <button type="button" className="icon-btn" title="刷新" onClick={onRefresh} disabled={refreshing}>
          <RefreshCw size={17} style={refreshing ? { animation: "spin 1s linear infinite" } : undefined} />
        </button>
        <button type="button" className="icon-btn primary" title="新建对话" disabled={!isConnected} onClick={onOpenNewChat}>
          <Plus size={19} />
        </button>
      </div>

      <div className="page-scroll">
        {openQuestions.length > 0 && (
          <button
            type="button"
            className="card"
            onClick={onOpenInbox}
            style={{
              width: "100%",
              textAlign: "left",
              display: "flex",
              alignItems: "center",
              gap: 10,
              borderColor: "var(--warn)",
              background: "var(--warn-soft)",
            }}
          >
            <Bell size={18} style={{ color: "var(--warn)", flex: "none" }} />
            <div style={{ flex: 1, minWidth: 0 }}>
              <div style={{ fontSize: 14, fontWeight: 700, color: "var(--text)" }}>
                有 {openQuestions.length} 项待你决定
              </div>
              <div
                style={{
                  fontSize: 12.5,
                  color: "var(--text-2)",
                  whiteSpace: "nowrap",
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                }}
              >
                {openQuestions[0].text}
              </div>
            </div>
            <span className="chip warn">去处理</span>
          </button>
        )}

        <div style={{ display: "flex", gap: 8, alignItems: "center", marginBottom: 10 }}>
          <div style={{ position: "relative", flex: 1 }}>
            <Search
              size={15}
              style={{
                position: "absolute",
                left: 11,
                top: "50%",
                transform: "translateY(-50%)",
                color: "var(--text-3)",
              }}
            />
            <input
              className="field-input"
              style={{ paddingLeft: 34, paddingTop: 9, paddingBottom: 9, fontSize: 14 }}
              placeholder="搜索 Agent、项目、模型或机器…"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
          </div>
          <button
            type="button"
            className="secondary-btn"
            style={{ padding: "8px 10px", fontSize: 12.5, flex: "none" }}
            onClick={() => setShowArchived((v) => !v)}
          >
            {showArchived ? "隐藏归档" : "含归档"}
          </button>
        </div>

        {groups.length === 0 ? (
          <div className="empty-state">
            <h3>{query ? "没有匹配的对话" : "还没有对话"}</h3>
            <p>{query ? "试试换个关键词，或切换「含归档」。" : "点右上角「+」在 Windows 中心或 Mac 上发起一个新对话。"}</p>
          </div>
        ) : (
          groups.map((g) => (
            <div key={g.key}>
              <div className="section-label">
                {g.os === "macos" ? <Laptop size={14} /> : g.os === "linux" ? <Server size={14} /> : <Monitor size={14} />}
                <span>{g.title}</span>
                {!g.online && <span className="chip warn">离线</span>}
              </div>
              <div className="agent-list">
                {g.agents.map((agent) => {
                  const pending = pendingByAgent.get(agent.id) ?? 0;
                  const imgUrl = agent.profile?.avatar_image ? avatarUrls[agent.profile.avatar_image] : undefined;
                  const ws = shortWorkspace(agent);
                  return (
                    <button key={agent.id} type="button" className="agent-row" onClick={() => onOpenAgent(agent.id)}>
                      <span className="avatar-circle" style={{ background: avatarBg(agent) }}>
                        {imgUrl ? (
                          <img src={imgUrl} alt={agent.name} />
                        ) : (
                          (agent.profile?.avatar || agent.name.slice(0, 2)).toUpperCase()
                        )}
                      </span>
                      <div className="agent-meta">
                        <div className="agent-row-top">
                          <span className="agent-name">{agent.name}</span>
                          {agent.archived_at && <span className="chip">已归档</span>}
                          <span style={{ flex: 1 }} />
                          {pending > 0 && <span className="chip danger">{pending} 待答</span>}
                          <span className={`dot ${agent.status}`} />
                        </div>
                        {agent.topic && <div className="agent-topic">{agent.topic}</div>}
                        <div className="agent-sub">
                          {ws && (
                            <span style={{ display: "inline-flex", alignItems: "center", gap: 3 }}>
                              <Folder size={12} />
                              {ws}
                            </span>
                          )}
                          {(agent.model || agent.plugin) && (
                            <span className="mono" style={{ overflow: "hidden", textOverflow: "ellipsis" }}>
                              {agent.model || agent.plugin}
                            </span>
                          )}
                        </div>
                      </div>
                    </button>
                  );
                })}
              </div>
            </div>
          ))
        )}
      </div>
    </>
  );
}
