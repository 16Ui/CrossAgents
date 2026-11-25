import { useEffect, useRef, useState } from "react";
import {
  ArrowLeft,
  ArrowUp,
  Check,
  Copy,
  Hand,
  ImagePlus,
  Square,
  X,
} from "lucide-react";
import {
  AgentAvatar,
  agentTone,
  effortLabel,
  FamilyMark,
  groupModels,
  ModelMark,
  OsIcon,
  prettyModel,
  UserAvatar,
  vendorName,
} from "../brands";
import { DiffView, MarkdownView } from "../components/MarkdownView";
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
  UserProfile,
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
  profile,
  avatarUrls,
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
  profile?: UserProfile;
  avatarUrls: Record<string, string>;
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

  const pendingMsgIdRef = useRef<string | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const bottomRef = useRef<HTMLDivElement>(null);

  const activeAgent = transcript?.agent ?? agent;
  const agentImgUrl = activeAgent?.profile?.avatar_image ? avatarUrls[activeAgent.profile.avatar_image] : undefined;
  const nodeInfo = nodes.find((n) =>
    !activeAgent?.node || activeAgent.node === "local"
      ? n.local
      : n.id === activeAgent.node || n.name === activeAgent.node || n.hostname === activeAgent.node,
  );
  const nodeOs = nodeInfo?.os ?? hubInfo?.machine?.os ?? "windows";

  const isRunning =
    (transcript?.delivery && transcript.delivery.state !== "idle") ||
    activeAgent?.status === "running" ||
    activeAgent?.status === "starting";

  // Auto-grow composer textarea
  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 132)}px`;
  }, [draft]);

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

  // Lazily fetch image attachments referenced in turns
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
  const activeTool = lastTurn?.items
    .slice()
    .reverse()
    .find((it) => it.type === "tool_call" && it.status !== "completed" && it.status !== "failed");

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
  const activeEffort = typeof activeAgent?.config?.effort === "string" ? activeAgent.config.effort : "";
  const activeModelText = activeAgent?.model ? prettyModel(activeAgent.model) : activeAgent ? vendorName(activeAgent.plugin) : "";
  const tone = activeAgent ? agentTone(activeAgent.status) : null;

  return (
    <div className="app-shell session-shell">
      <div className="nav-bar">
        <button type="button" className="icon-btn" onClick={onBack} aria-label="返回">
          <ArrowLeft size={18} />
        </button>
        {activeAgent && <AgentAvatar agent={activeAgent} imageUrl={agentImgUrl} size={30} status />}
        <div className="nav-title-wrap">
          <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
            <h1 className="nav-title" style={{ fontSize: 15.5 }}>
              {activeAgent?.name ?? "对话"}
            </h1>
            {tone && tone.color !== "off" && (
              <span className={`chip tiny ${tone.color}`}>{tone.label}</span>
            )}
          </div>
          <div className="nav-subtitle">
            <OsIcon os={nodeOs} size={11} />
            <span>{nodeDisplayName(activeAgent?.node, nodes, hubInfo, fallbackHost)}</span>
            {activeAgent && (
              <>
                <span style={{ opacity: 0.45 }}>·</span>
                <ModelMark plugin={activeAgent.plugin} model={activeAgent.model} size={11} />
                <span style={{ overflow: "hidden", textOverflow: "ellipsis" }}>{activeModelText}</span>
              </>
            )}
          </div>
        </div>
      </div>

      {error && (
        <div className="status-banner danger" style={{ paddingTop: 7 }}>
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
            {activeAgent && <AgentAvatar agent={activeAgent} imageUrl={agentImgUrl} size={48} />}
            <h3>{activeAgent?.name ?? "开始对话"}</h3>
            <p>在下方输入指令或发送截图即可开始。</p>
          </div>
        ) : (
          transcript.turns.map((turn) => {
            const isQueued = !turn.started_at && !turn.completed_at && !turn.withdrawn;
            const plainReply = turn.items
              .filter((it): it is Extract<TranscriptItem, { type: "output" }> => it.type === "output" && it.stream === "text")
              .map((it) => it.text)
              .join("\n\n");

            return (
              <div key={turn.message_id} className="turn-block">
                {(turn.input || (turn.attachments && turn.attachments.length > 0)) && (
                  <div className="person-turn">
                    <div className="person-turn-col">
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
                                  borderRadius: 10,
                                  border: "1px solid var(--border)",
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
                        <div className={`bubble ${turn.withdrawn ? "withdrawn" : ""}`}>{turn.input}</div>
                      )}
                      <div className="bubble-meta">
                        {turn.withdrawn && <span>已撤回</span>}
                        {isQueued && (
                          <>
                            <span style={{ color: "var(--warn-text)" }}>排队中</span>
                            <button
                              type="button"
                              className="chip accent tiny"
                              onClick={() => void sendNow(turn.message_id)}
                            >
                              立即插队
                            </button>
                          </>
                        )}
                        <span>{relativeTime(turn.sent_at)}</span>
                      </div>
                    </div>
                    <UserAvatar image={profile?.avatar} size={26} />
                  </div>
                )}

                {turn.items.length > 0 && (
                  <div className="assistant-turn">
                    {activeAgent && (
                      <div className="assistant-head">
                        <AgentAvatar agent={activeAgent} imageUrl={agentImgUrl} size={20} />
                        <span className="assistant-head-name">{activeAgent.name}</span>
                        <span className="assistant-head-model">
                          <ModelMark plugin={activeAgent.plugin} model={activeAgent.model} size={11} />
                          <span>{activeModelText}</span>
                        </span>
                      </div>
                    )}
                    <div className="assistant-body">
                      {turn.items.map((item, idx) => (
                        <ItemView key={idx} item={item} />
                      ))}
                    </div>
                    {plainReply && turn.completed_at && <CopyReplyRow text={plainReply} />}
                  </div>
                )}
              </div>
            );
          })
        )}

        {transcript?.error && <div className="error-box">{transcript.error}</div>}

        {isRunning && (
          <div className="working-live">
            <div className="working-row">
              <span className="working-spark" aria-hidden="true">
                ✦
              </span>
              <span>{transcript?.delivery.state === "starting" ? "正在启动 Agent…" : "正在思考并执行…"}</span>
            </div>
            {activeTool && activeTool.type === "tool_call" && (
              <div className="working-now">{activeTool.title}</div>
            )}
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

      <div className="composer-dock">
        <div className="composer-box">
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
                    <X size={11} />
                  </button>
                </div>
              ))}
            </div>
          )}

          <textarea
            ref={textareaRef}
            className="composer-textarea"
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

          <div className="composer-toolbar">
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
              className="composer-tool-btn icon-only"
              disabled={photos.length >= 4 || sending || !transport}
              onClick={() => fileInputRef.current?.click()}
              title="选择图片"
            >
              <ImagePlus size={15} />
            </button>

            {activeAgent && (
              <button
                type="button"
                className="composer-tool-btn"
                disabled={!transport}
                onClick={() => setModelModalOpen(true)}
                title="切换模型与推理力度"
              >
                <ModelMark plugin={activeAgent.plugin} model={activeAgent.model} size={13} />
                <span className="composer-model-label">
                  {activeModelText}
                  {activeEffort ? ` · ${effortLabel(activeEffort)}` : ""}
                </span>
              </button>
            )}

            {isRunning && !hasInput ? (
              <button
                type="button"
                className="composer-send-btn stop"
                onClick={() => void cancelTurn()}
                title="中断本轮"
              >
                <Square size={13} fill="currentColor" />
              </button>
            ) : (
              <button
                type="button"
                className="composer-send-btn"
                disabled={!hasInput || sending || !transport}
                onClick={() => void send()}
                title="发送"
              >
                <ArrowUp size={17} strokeWidth={2.2} />
              </button>
            )}
          </div>
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

function CopyReplyRow({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="msg-copy-row">
      <button
        type="button"
        className={`msg-copy-btn ${copied ? "copied" : ""}`}
        onClick={() => {
          navigator.clipboard?.writeText(text).catch(() => {});
          setCopied(true);
          window.setTimeout(() => setCopied(false), 1500);
        }}
      >
        {copied ? <Check size={12} /> : <Copy size={12} />}
        <span>{copied ? "已复制" : "复制原文"}</span>
      </button>
    </div>
  );
}

function ItemView({ item }: { item: TranscriptItem }) {
  switch (item.type) {
    case "output":
      if (item.stream === "thought") {
        return (
          <details className="thought">
            <summary>思考过程（{item.text.length} 字）</summary>
            <div className="thought-body">{item.text}</div>
          </details>
        );
      }
      return <MarkdownView text={item.text} />;

    case "tool_call": {
      const done = item.status === "completed";
      const failed = item.status === "failed";
      const dotCls = done ? "ok" : failed ? "danger" : "accent pulse";
      const output = item.detail?.output || item.output || item.detail?.result;
      const changes = item.detail?.changes ?? [];
      const dur =
        item.detail?.duration_ms != null
          ? item.detail.duration_ms >= 1000
            ? `${(item.detail.duration_ms / 1000).toFixed(1)}s`
            : `${item.detail.duration_ms}ms`
          : null;
      const hasBody = Boolean(item.detail?.command || changes.length > 0 || output || item.detail?.error);

      if (!hasBody) {
        return (
          <div className="tool-step">
            <div style={{ display: "flex", alignItems: "center", gap: 8, padding: "4px 0", fontFamily: "var(--font-mono)", fontSize: 12, color: "var(--text-2)" }}>
              <span className={`dot ${dotCls}`} />
              <span className="tool-title">{item.title}</span>
              {dur && <span className="muted" style={{ fontSize: 11 }}>{dur}</span>}
            </div>
          </div>
        );
      }

      return (
        <details className="tool-step">
          <summary>
            <span className={`dot ${dotCls}`} />
            <span className="tool-title">{item.title}</span>
            {dur && <span className="muted" style={{ fontSize: 11 }}>{dur}</span>}
            {failed && <span className="chip danger tiny">失败</span>}
          </summary>
          <div className="tool-detail">
            {item.detail?.command && <div className="tool-cmd">$ {item.detail.command}</div>}
            {changes.length > 0 && <DiffView changes={changes} />}
            {output && <pre className="tool-out mono">{output.slice(0, 3200)}</pre>}
            {item.detail?.error && <div className="error-box">{item.detail.error}</div>}
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
        <div className="chip accent" style={{ borderRadius: 9, padding: "6px 10px", whiteSpace: "normal", alignSelf: "flex-start" }}>
          插话：{item.text}（{item.read ? "已读" : "待读"}）
        </div>
      );

    case "warning":
      return (
        <div className="chip warn" style={{ borderRadius: 9, padding: "7px 10px", whiteSpace: "normal" }}>
          {item.message}
        </div>
      );

    case "error":
      return <div className="error-box">{item.message}</div>;
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
        margin: "0 12px 4px",
        borderColor: "color-mix(in srgb, var(--warn) 42%, var(--border))",
        borderLeft: "3px solid var(--warn)",
        boxShadow: "var(--shadow-float)",
        display: "flex",
        flexDirection: "column",
        gap: 8,
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: 6, fontSize: 12, fontWeight: 600, color: "var(--warn-text)" }}>
        <Hand size={14} />
        <span>Agent 需要你的决定</span>
      </div>
      <MarkdownView text={question.text} className="compact" />
      {question.options.length > 0 && (
        <div className="pill-wrap">
          {question.options.map((opt, idx) => (
            <button
              key={opt}
              type="button"
              className={`option-pill ${idx === 0 ? "first" : ""}`}
              disabled={busy}
              onClick={() => void submit(opt)}
            >
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
          style={{ padding: "7px 12px", fontSize: 13 }}
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
  const grouped = groupModels(models);
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
          <div style={{ display: "flex", alignItems: "center", gap: 7 }}>
            <ModelMark plugin={agent.plugin} model={model || agent.model} size={16} />
            <strong>切换模型 · {vendorName(agent.plugin)}</strong>
          </div>
          <button type="button" className="icon-btn" onClick={onClose}>
            <X size={17} />
          </button>
        </div>

        <div className="sheet-body" style={{ display: "flex", flexDirection: "column", gap: 14 }}>
          {efforts.length > 0 && (
            <div>
              <div style={{ fontSize: 12, color: "var(--muted)", marginBottom: 6 }}>推理力度</div>
              <div className="choice-grid">
                <button
                  type="button"
                  className={`choice-pill ${effort === "" ? "active" : ""}`}
                  onClick={() => setEffort("")}
                >
                  默认
                </button>
                {efforts.map((ef) => (
                  <button
                    key={ef}
                    type="button"
                    className={`choice-pill ${effort === ef ? "active" : ""}`}
                    onClick={() => setEffort(ef)}
                  >
                    {effortLabel(ef)}
                  </button>
                ))}
              </div>
            </div>
          )}

          <div>
            <div style={{ fontSize: 12, color: "var(--muted)", marginBottom: 6 }}>选择模型</div>
            <button
              type="button"
              className={`model-opt-row ${model === "" ? "active" : ""}`}
              onClick={() => setModel("")}
            >
              <span>默认模型</span>
              {model === "" && <Check size={15} />}
            </button>
            {grouped.map((g) => (
              <div key={g.key}>
                <div className="model-group-head">
                  <FamilyMark familyKey={g.key} size={13} />
                  <span>{g.label}</span>
                </div>
                {g.models.map((m) => (
                  <button
                    key={m}
                    type="button"
                    className={`model-opt-row ${model === m ? "active" : ""}`}
                    onClick={() => setModel(m)}
                  >
                    <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                      {prettyModel(m)}
                    </span>
                    {model === m && <Check size={15} />}
                  </button>
                ))}
              </div>
            ))}
          </div>

          <button type="button" className="primary-btn" disabled={saving} onClick={() => void save()}>
            {saving ? "保存中…" : "应用模型设置"}
          </button>
        </div>
      </div>
    </div>
  );
}
