import { useEffect, useRef, useState } from "react";
import {
  ArrowLeft,
  ArrowUp,
  Brain,
  CheckCircle2,
  Cpu,
  Hand,
  ImagePlus,
  Loader2,
  Square,
  X,
  XCircle,
} from "lucide-react";
import { MarkdownView } from "../components/MarkdownView";
import {
  availableEfforts,
  compressImageForUpload,
  extractErrorMessage,
  nodeDisplayName,
  relativeTime,
} from "../store";
import type { HubTransport } from "../transport";
import type {
  Agent,
  AttachmentUpload,
  HubInfo,
  HumanItem,
  PluginInfo,
  RemoteNodeInfo,
  Transcript,
  TranscriptItem,
} from "../types";

interface PendingPhoto {
  id: string;
  previewUrl: string;
  upload: AttachmentUpload;
}

export function SessionScreen({
  agentId,
  agent,
  transcript,
  nodes,
  hubInfo,
  fallbackHost,
  pendingQuestions,
  transport,
  onBack,
  onLoadedTranscript,
  onAnsweredQuestion,
}: {
  agentId: string;
  agent?: Agent;
  transcript?: Transcript;
  nodes: RemoteNodeInfo[];
  hubInfo?: HubInfo;
  fallbackHost?: string;
  pendingQuestions: HumanItem[];
  transport: HubTransport | null;
  onBack: () => void;
  onLoadedTranscript: (t: Transcript) => void;
  onAnsweredQuestion: (item: HumanItem) => void;
}) {
  const [draft, setDraft] = useState("");
  const [photos, setPhotos] = useState<PendingPhoto[]>([]);
  const [loading, setLoading] = useState(false);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string>();
  const [attachmentUrls, setAttachmentUrls] = useState<Record<string, string>>({});
  const [zoomUrl, setZoomUrl] = useState<string>();
  const [modelModalOpen, setModelModalOpen] = useState(false);

  // Idempotency key kept stable across retries for the same unsent message.
  const pendingMsgIdRef = useRef<string | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const bottomRef = useRef<HTMLDivElement>(null);

  const activeAgent = transcript?.agent ?? agent;
  const isRunning =
    (transcript?.delivery && transcript.delivery.state !== "idle") ||
    activeAgent?.status === "running" ||
    activeAgent?.status === "starting";

  useEffect(() => {
    if (!transport) return;
    let cancelled = false;
    setLoading(true);
    transport
      .request("GET", `/v1/agents/${encodeURIComponent(agentId)}/transcript`)
      .then((res) => {
        if (cancelled) return;
        if (res.status >= 400) throw new Error(extractErrorMessage(res.status, res.body));
        onLoadedTranscript(JSON.parse(res.body) as Transcript);
        setError(undefined);
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [agentId, transport]);

  // Lazily fetch image attachments referenced in turns.
  useEffect(() => {
    if (!transport || !transcript) return;
    const toFetch: string[] = [];
    for (const turn of transcript.turns) {
      for (const att of turn.attachments ?? []) {
        if (att.mime.startsWith("image/") && !attachmentUrls[att.id]) {
          toFetch.push(att.id);
        }
      }
    }
    for (const attId of toFetch) {
      void transport
        .request("GET", `/v1/agents/${encodeURIComponent(agentId)}/attachments/${encodeURIComponent(attId)}`)
        .then((res) => {
          if (res.status < 400) {
            const parsed = JSON.parse(res.body) as { mime: string; data: string };
            setAttachmentUrls((prev) => ({
              ...prev,
              [attId]: `data:${parsed.mime};base64,${parsed.data}`,
            }));
          }
        })
        .catch(() => {});
    }
  }, [agentId, transcript, transport]);

  const turnsCount = transcript?.turns.length ?? 0;
  const lastTurn = transcript?.turns[turnsCount - 1];
  const lastItemLen = lastTurn?.items.reduce((acc, it) => acc + (it.type === "output" ? it.text.length : 1), 0) ?? 0;

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [turnsCount, lastItemLen]);

  async function onPickFiles(files: FileList | null) {
    if (!files || files.length === 0) return;
    setError(undefined);
    const remaining = Math.max(0, 4 - photos.length);
    const chosen = Array.from(files).slice(0, remaining);
    try {
      const next: PendingPhoto[] = [];
      for (const f of chosen) {
        const compressed = await compressImageForUpload(f);
        next.push({ id: crypto.randomUUID(), ...compressed });
      }
      setPhotos((prev) => [...prev, ...next]);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      if (fileInputRef.current) fileInputRef.current.value = "";
    }
  }

  async function send() {
    if (!transport || sending) return;
    const text = draft.trim();
    if (!text && photos.length === 0) return;

    const clientMessageId = pendingMsgIdRef.current ?? crypto.randomUUID();
    pendingMsgIdRef.current = clientMessageId;

    setSending(true);
    setError(undefined);
    try {
      const canSteer = Boolean(isRunning && transcript?.capabilities?.steer);
      const res = await transport.request("POST", `/v1/agents/${encodeURIComponent(agentId)}/messages`, {
        text: text || "请查看这张图片。",
        ...(canSteer ? { steer: true } : {}),
        ...(photos.length ? { attachments: photos.map((p) => p.upload) } : {}),
        client_message_id: clientMessageId,
      });
      if (res.status >= 400) throw new Error(extractErrorMessage(res.status, res.body));
      pendingMsgIdRef.current = null;
      setDraft("");
      setPhotos([]);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSending(false);
    }
  }

  async function cancelTurn() {
    if (!transport) return;
    try {
      await transport.request("POST", `/v1/agents/${encodeURIComponent(agentId)}/cancel`);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  async function sendNow(messageId: string) {
    if (!transport) return;
    try {
      await transport.request(
        "POST",
        `/v1/agents/${encodeURIComponent(agentId)}/messages/${encodeURIComponent(messageId)}/send-now`,
      );
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }

  const hasInput = Boolean(draft.trim() || photos.length > 0);

  return (
    <div className="app-shell">
      <div className="nav-bar">
        <button type="button" className="icon-btn" onClick={onBack}>
          <ArrowLeft size={19} />
        </button>
        <div className="nav-title-wrap">
          <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
            <h1 className="nav-title" style={{ fontSize: 16 }}>
              {activeAgent?.name ?? "对话"}
            </h1>
            {activeAgent && <span className={`dot ${activeAgent.status}`} />}
          </div>
          <div className="nav-subtitle">
            {nodeDisplayName(activeAgent?.node, nodes, hubInfo, fallbackHost)} ·{" "}
            {activeAgent?.model || activeAgent?.plugin || ""}
          </div>
        </div>
        <button
          type="button"
          className="icon-btn"
          title="切换模型"
          disabled={!transport || !activeAgent}
          onClick={() => setModelModalOpen(true)}
        >
          <Cpu size={17} />
        </button>
      </div>

      {error && (
        <div className="status-banner danger" style={{ paddingTop: 8 }}>
          <span className="status-banner-msg">{error}</span>
          <button type="button" className="status-banner-btn" onClick={() => setError(undefined)}>
            关闭
          </button>
        </div>
      )}

      <div className="session-messages">
        {loading && !transcript ? (
          <div className="empty-state">
            <p>正在同步对话记录…</p>
          </div>
        ) : !transcript || transcript.turns.length === 0 ? (
          <div className="empty-state">
            <h3>{activeAgent?.name ?? "开始对话"}</h3>
            <p>发送一条消息或手机截图即可开始。</p>
          </div>
        ) : (
          transcript.turns.map((turn) => {
            const isQueued = !turn.started_at && !turn.completed_at && !turn.withdrawn;
            return (
              <div key={turn.message_id} style={{ display: "flex", flexDirection: "column", gap: 10 }}>
                {(turn.input || (turn.attachments && turn.attachments.length > 0)) && (
                  <div className="turn-user-wrap">
                    {turn.attachments && turn.attachments.length > 0 && (
                      <div style={{ display: "flex", gap: 6, flexWrap: "wrap", justifyContent: "flex-end" }}>
                        {turn.attachments.map((att) =>
                          attachmentUrls[att.id] ? (
                            <img
                              key={att.id}
                              src={attachmentUrls[att.id]}
                              alt={att.name}
                              onClick={() => setZoomUrl(attachmentUrls[att.id])}
                              style={{
                                width: 112,
                                height: 112,
                                objectFit: "cover",
                                borderRadius: 12,
                                cursor: "pointer",
                              }}
                            />
                          ) : (
                            <span key={att.id} className="chip">
                              {att.name}
                            </span>
                          ),
                        )}
                      </div>
                    )}
                    {turn.input && (
                      <div className={`turn-user-bubble ${turn.withdrawn ? "withdrawn" : ""}`}>{turn.input}</div>
                    )}
                    <div style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 11, color: "var(--text-3)" }}>
                      {turn.withdrawn && <span>已撤回</span>}
                      {isQueued && (
                        <>
                          <span style={{ color: "var(--warn)" }}>排队中</span>
                          <button
                            type="button"
                            className="chip accent"
                            onClick={() => void sendNow(turn.message_id)}
                          >
                            立即插队
                          </button>
                        </>
                      )}
                      <span>{relativeTime(turn.sent_at)}</span>
                    </div>
                  </div>
                )}

                {turn.items.length > 0 && (
                  <div className="turn-assistant-card">
                    {turn.items.map((item, idx) => (
                      <ItemView key={idx} item={item} />
                    ))}
                  </div>
                )}
              </div>
            );
          })
        )}

        {transcript?.error && (
          <div className="chip danger" style={{ borderRadius: 10, padding: "9px 11px", whiteSpace: "normal" }}>
            {transcript.error}
          </div>
        )}

        {isRunning && (
          <div style={{ display: "flex", alignItems: "center", gap: 8, fontSize: 12.5, color: "var(--text-2)", padding: "0 4px" }}>
            <Loader2 size={14} style={{ animation: "spin 1s linear infinite" }} />
            <span>{transcript?.delivery.state === "starting" ? "Agent 正在启动…" : "Agent 正在思考并执行…"}</span>
          </div>
        )}
        <div ref={bottomRef} />
      </div>

      {pendingQuestions[0] && transport && (
        <InlineQuestionBanner
          question={pendingQuestions[0]}
          transport={transport}
          onAnswered={onAnsweredQuestion}
        />
      )}

      <div className="composer-wrap">
        {photos.length > 0 && (
          <div className="composer-photos">
            {photos.map((p) => (
              <div key={p.id} className="composer-photo-item">
                <img src={p.previewUrl} alt={p.upload.name} />
                <button
                  type="button"
                  className="composer-photo-del"
                  onClick={() => setPhotos((prev) => prev.filter((x) => x.id !== p.id))}
                >
                  <X size={12} />
                </button>
              </div>
            ))}
          </div>
        )}

        <div className="composer-row">
          <input
            ref={fileInputRef}
            type="file"
            accept="image/*"
            multiple
            style={{ display: "none" }}
            onChange={(e) => void onPickFiles(e.target.files)}
          />
          <button
            type="button"
            className="icon-btn"
            disabled={photos.length >= 4 || sending || !transport}
            onClick={() => fileInputRef.current?.click()}
            title="选择图片"
          >
            <ImagePlus size={19} />
          </button>

          <textarea
            className="composer-input"
            rows={1}
            placeholder={isRunning ? "继续补充或排队发送…" : `发消息给 ${activeAgent?.name ?? "Agent"}…`}
            value={draft}
            onChange={(e) => {
              setDraft(e.target.value);
              pendingMsgIdRef.current = null;
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                e.preventDefault();
                void send();
              }
            }}
          />

          {isRunning && !hasInput ? (
            <button
              type="button"
              className="icon-btn"
              style={{ background: "var(--danger)", color: "#fff" }}
              onClick={() => void cancelTurn()}
              title="停止"
            >
              <Square size={15} fill="currentColor" />
            </button>
          ) : (
            <button
              type="button"
              className="icon-btn primary"
              disabled={!hasInput || sending || !transport}
              onClick={() => void send()}
              title="发送"
            >
              <ArrowUp size={19} />
            </button>
          )}
        </div>
      </div>

      {zoomUrl && (
        <div
          className="qr-fullscreen"
          style={{ justifyContent: "center", alignItems: "center", padding: 16 }}
          onClick={() => setZoomUrl(undefined)}
        >
          <img src={zoomUrl} alt="预览" style={{ maxWidth: "100%", maxHeight: "90dvh", objectFit: "contain" }} />
        </div>
      )}

      {modelModalOpen && activeAgent && transport && (
        <ModelSwitcherModal
          agent={activeAgent}
          transport={transport}
          onClose={() => setModelModalOpen(false)}
          onSaved={(updated) => {
            if (transcript) onLoadedTranscript({ ...transcript, agent: updated });
            setModelModalOpen(false);
          }}
        />
      )}
    </div>
  );
}

function ItemView({ item }: { item: TranscriptItem }) {
  switch (item.type) {
    case "output":
      if (item.stream === "thought") {
        return (
          <details className="fold-card">
            <summary>
              <Brain size={14} style={{ color: "var(--text-2)" }} />
              <span>思考过程（{item.text.length} 字）</span>
            </summary>
            <div className="fold-body">{item.text}</div>
          </details>
        );
      }
      return <MarkdownView text={item.text} />;

    case "tool_call": {
      const done = item.status === "completed";
      const failed = item.status === "failed";
      const output = item.detail?.output || item.output || item.detail?.result;
      return (
        <details className="fold-card">
          <summary>
            {done ? (
              <CheckCircle2 size={14} style={{ color: "var(--ok)", flex: "none" }} />
            ) : failed ? (
              <XCircle size={14} style={{ color: "var(--danger)", flex: "none" }} />
            ) : (
              <Loader2 size={14} style={{ color: "var(--accent)", flex: "none" }} />
            )}
            <span style={{ flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
              {item.title}
            </span>
            <span className={`chip ${done ? "ok" : failed ? "danger" : "accent"}`}>
              {done ? "完成" : failed ? "失败" : "运行中"}
            </span>
          </summary>
          <div className="fold-body mono" style={{ fontSize: 11.5 }}>
            {item.detail?.command && <div style={{ marginBottom: 6, color: "var(--text)" }}>$ {item.detail.command}</div>}
            {item.detail?.changes && item.detail.changes.length > 0 && (
              <div style={{ marginBottom: 6 }}>
                {item.detail.changes.map((c, i) => (
                  <div key={i}>
                    [{c.kind}] {c.path}
                  </div>
                ))}
              </div>
            )}
            {output && <div>{output.slice(0, 2400)}</div>}
            {item.detail?.error && <div style={{ color: "var(--danger)" }}>{item.detail.error}</div>}
          </div>
        </details>
      );
    }

    case "permission":
      return (
        <div className="chip" style={{ alignSelf: "flex-start" }}>
          权限「{item.title}」：{item.decision}
        </div>
      );

    case "steer":
      return (
        <div className="chip accent" style={{ borderRadius: 10, padding: "7px 10px", whiteSpace: "normal" }}>
          插话：{item.text}（{item.read ? "已读" : "待读"}）
        </div>
      );

    case "warning":
      return (
        <div className="chip warn" style={{ borderRadius: 10, padding: "7px 10px", whiteSpace: "normal" }}>
          {item.message}
        </div>
      );

    case "error":
      return (
        <div className="chip danger" style={{ borderRadius: 10, padding: "7px 10px", whiteSpace: "normal" }}>
          {item.message}
        </div>
      );
  }
}

function InlineQuestionBanner({
  question,
  transport,
  onAnswered,
}: {
  question: HumanItem;
  transport: HubTransport;
  onAnswered: (item: HumanItem) => void;
}) {
  const [reply, setReply] = useState("");
  const [busy, setBusy] = useState(false);

  async function submit(answer: string) {
    if (busy || !answer.trim()) return;
    setBusy(true);
    try {
      const res = await transport.request("POST", `/v1/questions/${encodeURIComponent(question.id)}/answer`, {
        answer: answer.trim(),
      });
      if (res.status < 400) {
        onAnswered(JSON.parse(res.body) as HumanItem);
        setReply("");
      }
    } finally {
      setBusy(false);
    }
  }

  return (
    <div
      className="card"
      style={{
        margin: "0 10px 6px",
        borderColor: "var(--warn)",
        boxShadow: "0 4px 16px rgba(0,0,0,0.12)",
        display: "flex",
        flexDirection: "column",
        gap: 8,
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: 6, fontSize: 12, fontWeight: 700, color: "var(--warn)" }}>
        <Hand size={14} />
        <span>Agent 需要你的决定</span>
      </div>
      <div style={{ fontSize: 13.5, lineHeight: 1.45 }}>{question.text}</div>
      {question.options.length > 0 && (
        <div className="pill-wrap">
          {question.options.map((opt) => (
            <button key={opt} type="button" className="option-pill" disabled={busy} onClick={() => void submit(opt)}>
              {opt}
            </button>
          ))}
        </div>
      )}
      <div style={{ display: "flex", gap: 6 }}>
        <input
          className="field-input"
          style={{ padding: "7px 10px", fontSize: 13.5 }}
          placeholder="或输入自定义回复…"
          value={reply}
          onChange={(e) => setReply(e.target.value)}
          disabled={busy}
        />
        <button
          type="button"
          className="secondary-btn"
          disabled={busy || !reply.trim()}
          onClick={() => void submit(reply)}
        >
          回复
        </button>
      </div>
    </div>
  );
}

function ModelSwitcherModal({
  agent,
  transport,
  onClose,
  onSaved,
}: {
  agent: Agent;
  transport: HubTransport;
  onClose: () => void;
  onSaved: (agent: Agent) => void;
}) {
  const [plugin, setPlugin] = useState<PluginInfo>();
  const [model, setModel] = useState(agent.model ?? "");
  const [effort, setEffort] = useState(
    typeof agent.config?.effort === "string" ? agent.config.effort : "",
  );
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    const q = agent.node ? `?node=${encodeURIComponent(agent.node)}` : "";
    void transport.request("GET", `/v1/plugins${q}`).then((res) => {
      if (res.status < 400) {
        const parsed = JSON.parse(res.body) as { plugins: PluginInfo[] };
        setPlugin(parsed.plugins.find((p) => p.name === agent.plugin));
      }
    });
  }, [agent.node, agent.plugin, transport]);

  const models = plugin ? (plugin.models.length ? plugin.models : plugin.all_models) : [];
  const efforts = availableEfforts(plugin, model);

  async function save() {
    setSaving(true);
    try {
      const res = await transport.request("POST", `/v1/agents/${encodeURIComponent(agent.id)}/model`, {
        model: model || null,
        effort: effort || undefined,
      });
      if (res.status < 400) {
        onSaved(JSON.parse(res.body) as Agent);
      }
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="sheet-backdrop" onClick={onClose}>
      <div className="sheet-panel" onClick={(e) => e.stopPropagation()}>
        <div className="sheet-head">
          <strong>切换模型（{agent.plugin}）</strong>
          <button type="button" className="icon-btn" onClick={onClose}>
            <X size={18} />
          </button>
        </div>
        <div className="sheet-body" style={{ display: "flex", flexDirection: "column", gap: 12 }}>
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
          <button type="button" className="primary-btn" disabled={saving} onClick={() => void save()}>
            {saving ? "保存中…" : "保存"}
          </button>
        </div>
      </div>
    </div>
  );
}
