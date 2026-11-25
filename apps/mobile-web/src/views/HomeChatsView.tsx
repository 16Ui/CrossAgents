import { useMemo, useState } from "react";
import { Bell, ChevronRight, Folder, Plus, RefreshCw, Search, X } from "lucide-react";
import { AgentAvatar, BrandMark, ModelMark, OsIcon, prettyModel, vendorName } from "../brands";
import { nodeDisplayName, relativeTime, shortWorkspace } from "../store";
import type { Agent, HubInfo, HumanItem, RemoteNodeInfo } from "../types";

type FilterMode = "active" | "running" | "archived";

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
  const [filter, setFilter] = useState<FilterMode>("active");

  const pendingByAgent = useMemo(() => {
    const map = new Map<string, number>();
    for (const q of openQuestions) {
      map.set(q.from, (map.get(q.from) ?? 0) + 1);
    }
    return map;
  }, [openQuestions]);

  const counts = useMemo(() => {
    let active = 0;
    let running = 0;
    let archived = 0;
    for (const a of agents) {
      if (a.archived_at) {
        archived++;
      } else {
        active++;
        if (a.status === "running" || a.status === "starting") running++;
      }
    }
    return { active, running, archived };
  }, [agents]);

  const groups = useMemo(() => {
    const q = query.trim().toLowerCase();
    const filtered = agents.filter((a) => {
      if (filter === "active" && a.archived_at) return false;
      if (filter === "running" && (a.archived_at || (a.status !== "running" && a.status !== "starting"))) return false;
      if (filter === "archived" && !a.archived_at) return false;
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
        const info = nodes.find((n) =>
          nodeKey === "local" ? n.local : n.id === nodeKey || n.name === nodeKey || n.hostname === nodeKey,
        );
        const isLocal = nodeKey === "local" || Boolean(info?.local);
        list.sort((a, b) => {
          const pa = pendingByAgent.get(a.id) ?? 0;
          const pb = pendingByAgent.get(b.id) ?? 0;
          if (pa !== pb) return pb - pa;
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
  }, [agents, nodes, hubInfo, fallbackHost, query, filter, pendingByAgent]);

  return (
    <>
      <div className="nav-bar">
        <div className="nav-brand">
          <BrandMark size={21} />
          <div className="nav-title-wrap">
            <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
              <h1 className="nav-title">CrossAgents</h1>
              <span className={`dot ${isConnected ? "ok" : "warn"}`} />
            </div>
            {!isConnected && snapshotSavedAt ? (
              <div className="nav-subtitle">缓存快照 · {relativeTime(snapshotSavedAt)}</div>
            ) : (
              <div className="nav-subtitle">{hubInfo?.machine?.hostname || fallbackHost || "中心在线"}</div>
            )}
          </div>
        </div>
        <button type="button" className="icon-btn" title="刷新" onClick={onRefresh} disabled={refreshing}>
          <RefreshCw size={16} style={refreshing ? { animation: "spin 1s linear infinite" } : undefined} />
        </button>
        <button type="button" className="icon-btn primary" title="新建对话" disabled={!isConnected} onClick={onOpenNewChat}>
          <Plus size={18} />
        </button>
      </div>

      <div className="page-scroll">
        {openQuestions.length > 0 && (
          <button type="button" className="pending-callout" onClick={onOpenInbox}>
            <Bell size={16} style={{ color: "var(--warn)", flex: "none" }} />
            <div style={{ flex: 1, minWidth: 0 }}>
              <div style={{ fontSize: 13.5, fontWeight: 600, color: "var(--text)" }}>
                {openQuestions.length} 项待你审批或回复
              </div>
              <div
                style={{
                  fontSize: 12,
                  color: "var(--text-2)",
                  whiteSpace: "nowrap",
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                  marginTop: 1,
                }}
              >
                {openQuestions[0].text}
              </div>
            </div>
            <ChevronRight size={15} style={{ color: "var(--muted)", flex: "none" }} />
          </button>
        )}

        <div style={{ display: "flex", flexDirection: "column", gap: 8, marginBottom: 10 }}>
          <div className="search-bar">
            <Search size={14} />
            <input
              placeholder="搜索对话、项目、模型或机器…"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
            {query && (
              <button type="button" className="icon-btn" style={{ width: 24, height: 24 }} onClick={() => setQuery("")}>
                <X size={13} />
              </button>
            )}
          </div>

          <div className="segmented">
            <button type="button" className={filter === "active" ? "active" : ""} onClick={() => setFilter("active")}>
              活跃 ({counts.active})
            </button>
            <button type="button" className={filter === "running" ? "active" : ""} onClick={() => setFilter("running")}>
              进行中 ({counts.running})
            </button>
            <button type="button" className={filter === "archived" ? "active" : ""} onClick={() => setFilter("archived")}>
              归档 ({counts.archived})
            </button>
          </div>
        </div>

        {groups.length === 0 ? (
          <div className="empty-state">
            <h3>{query ? "没有匹配的对话" : filter === "running" ? "当前没有运行中的对话" : "暂无对话"}</h3>
            <p>
              {query
                ? "试试换个关键词，或切换顶部分组。"
                : "点右上角「+」在 Windows 中心或 MacBook-Pro 上开启一个新对话。"}
            </p>
          </div>
        ) : (
          groups.map((g) => (
            <div key={g.key}>
              <div className="section-label">
                <OsIcon os={g.os} size={13} />
                <span>{g.title}</span>
                <span style={{ opacity: 0.6 }}>{g.agents.length}</span>
                {!g.online && <span className="chip warn tiny">离线</span>}
              </div>
              <div className="agent-list">
                {g.agents.map((agent) => {
                  const pending = pendingByAgent.get(agent.id) ?? 0;
                  const imgUrl = agent.profile?.avatar_image ? avatarUrls[agent.profile.avatar_image] : undefined;
                  const ws = shortWorkspace(agent);
                  const modelLabel = agent.model ? prettyModel(agent.model) : vendorName(agent.plugin);
                  return (
                    <button key={agent.id} type="button" className="agent-row" onClick={() => onOpenAgent(agent.id)}>
                      <AgentAvatar agent={agent} imageUrl={imgUrl} size={38} status />
                      <div className="agent-meta">
                        <div className="agent-row-top">
                          <span className="agent-name">{agent.name}</span>
                          {agent.archived_at && <span className="chip tiny">归档</span>}
                          <span style={{ flex: 1 }} />
                          {pending > 0 && <span className="chip danger tiny">{pending} 待答</span>}
                        </div>
                        {agent.topic && <div className="agent-topic">{agent.topic}</div>}
                        <div className="agent-sub">
                          <span className="agent-model-inline">
                            <ModelMark plugin={agent.plugin} model={agent.model} size={12} />
                            <span>{modelLabel}</span>
                          </span>
                          {ws && (
                            <>
                              <span style={{ opacity: 0.45 }}>·</span>
                              <span className="agent-ws-inline">
                                <Folder size={11} style={{ flex: "none" }} />
                                <span style={{ overflow: "hidden", textOverflow: "ellipsis" }}>{ws}</span>
                              </span>
                            </>
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
