import { useState } from "react";
import { CheckCircle2, RefreshCw } from "lucide-react";
import { MarkdownView } from "../components/MarkdownView";
import { extractErrorMessage, nodeDisplayName, relativeTime } from "../store";
import type { HubTransport } from "../transport";
import type { Agent, HubInfo, HumanItem, RemoteNodeInfo } from "../types";

function actionBadge(item: HumanItem): { label: string; cls: string } {
  if (item.action === "approval") return { label: "审批", cls: "danger" };
  if (item.action === "budget") return { label: "预算", cls: "warn" };
  if (item.action === "work") return { label: "任务提案", cls: "accent" };
  return { label: item.kind === "notice" ? "通知" : "提问", cls: "accent" };
}

export function InboxView({
  questions,
  agents,
  nodes,
  hubInfo,
  fallbackHost,
  highlightId,
  transport,
  isConnected,
  onRefresh,
  onUpdatedQuestion,
}: {
  questions: HumanItem[];
  agents: Agent[];
  nodes: RemoteNodeInfo[];
  hubInfo?: HubInfo;
  fallbackHost?: string;
  highlightId?: string;
  transport: HubTransport | null;
  isConnected: boolean;
  onRefresh: () => void;
  onUpdatedQuestion: (item: HumanItem) => void;
}) {
  const [tab, setTab] = useState<"open" | "done">("open");
  const openItems = questions.filter((q) => q.status === "open");
  const doneItems = questions.filter((q) => q.status !== "open");
  const visible = tab === "open" ? openItems : doneItems;

  return (
    <>
      <div className="nav-bar">
        <div className="nav-title-wrap">
          <h1 className="nav-title">待办</h1>
        </div>
        <button type="button" className="icon-btn" onClick={onRefresh} title="刷新">
          <RefreshCw size={17} />
        </button>
      </div>

      <div className="page-scroll">
        <div className="segmented">
          <button type="button" className={tab === "open" ? "active" : ""} onClick={() => setTab("open")}>
            待处理 ({openItems.length})
          </button>
          <button type="button" className={tab === "done" ? "active" : ""} onClick={() => setTab("done")}>
            已处理 ({doneItems.length})
          </button>
        </div>

        {visible.length === 0 ? (
          <div className="empty-state">
            <CheckCircle2 size={36} style={{ color: "var(--ok)" }} />
            <h3>{tab === "open" ? "暂无待处理审批或提问" : "暂无历史记录"}</h3>
            <p>
              {tab === "open"
                ? "任意机器上的 Agent 发起提问或请求审批时，都会第一时间出现在这里并触发锁屏提醒。"
                : "你已回复或忽略的事项会留存在这里方便核查。"}
            </p>
          </div>
        ) : (
          visible.map((item) => (
            <QuestionCard
              key={item.id}
              item={item}
              agent={agents.find((a) => a.id === item.from)}
              nodes={nodes}
              hubInfo={hubInfo}
              fallbackHost={fallbackHost}
              highlighted={highlightId === item.id}
              transport={transport}
              isConnected={isConnected}
              onRefresh={onRefresh}
              onUpdated={onUpdatedQuestion}
            />
          ))
        )}
      </div>
    </>
  );
}

function QuestionCard({
  item,
  agent,
  nodes,
  hubInfo,
  fallbackHost,
  highlighted,
  transport,
  isConnected,
  onRefresh,
  onUpdated,
}: {
  item: HumanItem;
  agent?: Agent;
  nodes: RemoteNodeInfo[];
  hubInfo?: HubInfo;
  fallbackHost?: string;
  highlighted: boolean;
  transport: HubTransport | null;
  isConnected: boolean;
  onRefresh: () => void;
  onUpdated: (item: HumanItem) => void;
}) {
  const [reply, setReply] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const badge = actionBadge(item);

  async function answer(text: string) {
    if (!transport || busy || !text.trim()) return;
    setBusy(true);
    setError(undefined);
    try {
      // Re-verify live Hub state first so stale lock-screen notifications never approve twice.
      const res = await transport.request("POST", `/v1/questions/${encodeURIComponent(item.id)}/answer`, {
        answer: text.trim(),
      });
      if (res.status === 409 || res.status === 404) {
        onRefresh();
        setError("该请求已在桌面端处理或已过期，已同步最新状态。");
        return;
      }
      if (res.status >= 400) throw new Error(extractErrorMessage(res.status, res.body));
      onUpdated(JSON.parse(res.body) as HumanItem);
      setReply("");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function dismiss() {
    if (!transport || busy) return;
    setBusy(true);
    setError(undefined);
    try {
      const res = await transport.request("POST", `/v1/questions/${encodeURIComponent(item.id)}/dismiss`);
      if (res.status >= 400 && res.status !== 409 && res.status !== 404) {
        throw new Error(extractErrorMessage(res.status, res.body));
      }
      onUpdated({ ...item, status: "dismissed" });
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div
      className="card"
      style={{
        display: "flex",
        flexDirection: "column",
        gap: 10,
        borderColor: highlighted ? "var(--accent)" : undefined,
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ fontSize: 14.5, fontWeight: 700 }}>{agent?.name ?? item.from}</div>
          <div style={{ fontSize: 11.5, color: "var(--text-2)" }}>
            {nodeDisplayName(agent?.node, nodes, hubInfo, fallbackHost)} · {relativeTime(item.created_at)}
          </div>
        </div>
        <span className={`chip ${badge.cls}`}>{badge.label}</span>
      </div>

      <MarkdownView text={item.text} />

      {item.detail && (
        <details className="fold-card">
          <summary>查看详细内容</summary>
          <div className="fold-body">{item.detail}</div>
        </details>
      )}

      {item.status === "open" ? (
        <>
          {!isConnected && (
            <div className="chip warn" style={{ borderRadius: 8, padding: "6px 9px" }}>
              正在重连中心，请待连接恢复后再确认批准
            </div>
          )}

          {item.options.length > 0 && (
            <div className="pill-wrap">
              {item.options.map((opt) => (
                <button
                  key={opt}
                  type="button"
                  className="option-pill"
                  disabled={busy || !isConnected}
                  onClick={() => void answer(opt)}
                >
                  {opt}
                </button>
              ))}
            </div>
          )}

          <div style={{ display: "flex", gap: 6 }}>
            <input
              className="field-input"
              style={{ padding: "8px 10px", fontSize: 14 }}
              placeholder="输入自定义回复…"
              value={reply}
              onChange={(e) => setReply(e.target.value)}
              disabled={busy || !isConnected}
            />
            <button
              type="button"
              className="primary-btn"
              style={{ width: "auto", padding: "8px 14px", fontSize: 13.5 }}
              disabled={busy || !isConnected || !reply.trim()}
              onClick={() => void answer(reply)}
            >
              发送
            </button>
          </div>

          <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", fontSize: 12 }}>
            <span style={{ color: "var(--text-3)" }}>
              {item.default ? `超时默认：${item.default}` : ""}
            </span>
            <button
              type="button"
              style={{ color: "var(--danger)", fontWeight: 600 }}
              disabled={busy || !isConnected}
              onClick={() => void dismiss()}
            >
              忽略
            </button>
          </div>
        </>
      ) : (
        <div className="chip" style={{ borderRadius: 8, padding: "7px 10px", justifyContent: "space-between" }}>
          <span>{item.answer ? `已回复：${item.answer}` : item.status === "dismissed" ? "已忽略" : `状态：${item.status}`}</span>
          <span>{relativeTime(item.answered_at)}</span>
        </div>
      )}

      {error && (
        <div className="chip danger" style={{ borderRadius: 8, padding: "7px 10px", whiteSpace: "normal" }}>
          {error}
        </div>
      )}
    </div>
  );
}
