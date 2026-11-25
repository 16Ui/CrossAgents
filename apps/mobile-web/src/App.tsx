import { useCallback, useEffect, useRef, useState } from "react";
import { Bell, Gauge, MessageSquare } from "lucide-react";
import { registerServiceWorker } from "./push";
import {
  applyTranscriptEvent,
  clearPairing,
  loadPairing,
  loadSnapshot,
  savePairing,
  saveSnapshot,
} from "./store";
import { connectTransport, TransportError, type EventHandle, type HubTransport } from "./transport";
import type {
  Agent,
  ConnectionState,
  HubEvent,
  HubInfo,
  HumanItem,
  PairingRecord,
  QuotaResponse,
  RemoteNodeInfo,
  Transcript,
  UserProfile,
} from "./types";
import { HomeChatsView } from "./views/HomeChatsView";
import { InboxView } from "./views/InboxView";
import { NewChatModal } from "./views/NewChatModal";
import { PairingScreen } from "./views/PairingScreen";
import { SessionScreen } from "./views/SessionScreen";
import { StatusView } from "./views/StatusView";

type Tab = "chats" | "inbox" | "status";

export function App() {
  const [pairing, setPairing] = useState<PairingRecord | null>(() => loadPairing());
  const [connection, setConnection] = useState<ConnectionState>(() =>
    loadPairing() ? { kind: "connecting", detail: "正在连接中心…" } : { kind: "unpaired" },
  );

  const initialSnap = useRef(loadSnapshot());
  const [snapshotSavedAt, setSnapshotSavedAt] = useState<string | undefined>(initialSnap.current?.savedAt);
  const [hubInfo, setHubInfo] = useState<HubInfo | undefined>(initialSnap.current?.hubInfo);
  const [agents, setAgents] = useState<Agent[]>(initialSnap.current?.agents ?? []);
  const [questions, setQuestions] = useState<HumanItem[]>(initialSnap.current?.questions ?? []);
  const [nodes, setNodes] = useState<RemoteNodeInfo[]>(initialSnap.current?.nodes ?? []);
  const [quota, setQuota] = useState<QuotaResponse | undefined>(initialSnap.current?.quota);
  const [profile, setProfile] = useState<UserProfile | undefined>(initialSnap.current?.profile);

  const [transcripts, setTranscripts] = useState<Record<string, Transcript>>({});
  const [avatarUrls, setAvatarUrls] = useState<Record<string, string>>({});

  const [tab, setTab] = useState<Tab>("chats");
  const [openAgentId, setOpenAgentId] = useState<string | null>(null);
  const [highlightQuestionId, setHighlightQuestionId] = useState<string>();
  const [showingNewChat, setShowingNewChat] = useState(false);
  const [refreshing, setRefreshing] = useState(false);

  const transportRef = useRef<HubTransport | null>(null);
  const [transport, setTransport] = useState<HubTransport | null>(null);
  const sseRef = useRef<EventHandle | null>(null);
  const lastSeqRef = useRef<number | undefined>(undefined);
  const reconnectTimerRef = useRef<number | null>(null);
  const connectEpochRef = useRef(0);
  const pairingRef = useRef<PairingRecord | null>(pairing);
  pairingRef.current = pairing;

  // Register Service Worker & handle lock-screen notification deep links.
  useEffect(() => {
    void registerServiceWorker();
    const parseHash = () => {
      if (window.location.hash.startsWith("#inbox")) {
        setOpenAgentId(null);
        setTab("inbox");
        const q = new URLSearchParams(window.location.hash.split("?")[1] || "").get("q");
        if (q) setHighlightQuestionId(q);
      }
    };
    parseHash();
    window.addEventListener("hashchange", parseHash);

    const onSwMsg = (ev: MessageEvent) => {
      const msg = ev.data as { type?: string; question_id?: string } | undefined;
      if (msg?.type === "OPEN_QUESTION") {
        setOpenAgentId(null);
        setTab("inbox");
        if (msg.question_id) setHighlightQuestionId(msg.question_id);
      }
    };
    navigator.serviceWorker?.addEventListener("message", onSwMsg);
    return () => {
      window.removeEventListener("hashchange", parseHash);
      navigator.serviceWorker?.removeEventListener("message", onSwMsg);
    };
  }, []);

  const persistSnapshot = useCallback(
    (next: {
      hubInfo?: HubInfo;
      agents: Agent[];
      questions: HumanItem[];
      nodes: RemoteNodeInfo[];
      quota?: QuotaResponse;
      profile?: UserProfile;
    }) => {
      const savedAt = new Date().toISOString();
      setSnapshotSavedAt(savedAt);
      saveSnapshot({ savedAt, ...next });
    },
    [],
  );

  const fetchAvatars = useCallback((t: HubTransport, list: Agent[]) => {
    const ids = new Set<string>();
    for (const a of list) {
      if (a.profile?.avatar_image) ids.add(a.profile.avatar_image);
    }
    for (const id of ids) {
      setAvatarUrls((prev) => {
        if (prev[id]) return prev;
        void t
          .request("GET", `/v1/avatars/${encodeURIComponent(id)}`)
          .then((res) => {
            if (res.status < 400) {
              const parsed = JSON.parse(res.body) as { mime: string; data: string };
              setAvatarUrls((p) => ({ ...p, [id]: `data:${parsed.mime};base64,${parsed.data}` }));
            }
          })
          .catch(() => {});
        return prev;
      });
    }
  }, []);

  const bootstrap = useCallback(
    async (t: HubTransport) => {
      setRefreshing(true);
      try {
        const [infoRes, agentsRes, questionsRes, nodesRes, quotaRes, profileRes] = await Promise.all([
          t.request("GET", "/v1/info"),
          t.request("GET", "/v1/agents"),
          t.request("GET", "/v1/questions?all=true"),
          t.request("GET", "/v1/nodes"),
          t.request("GET", "/v1/quota"),
          t.request("GET", "/v1/profile"),
        ]);

        const nextInfo = infoRes.status < 400 ? (JSON.parse(infoRes.body) as HubInfo) : hubInfo;
        const nextAgents =
          agentsRes.status < 400 ? ((JSON.parse(agentsRes.body) as { agents: Agent[] }).agents ?? []) : agents;
        const nextQuestions =
          questionsRes.status < 400
            ? ((JSON.parse(questionsRes.body) as { items: HumanItem[] }).items ?? [])
            : questions;
        const nextNodes =
          nodesRes.status < 400 ? ((JSON.parse(nodesRes.body) as { nodes: RemoteNodeInfo[] }).nodes ?? []) : nodes;
        const nextQuota = quotaRes.status < 400 ? (JSON.parse(quotaRes.body) as QuotaResponse) : quota;
        const nextProfile = profileRes.status < 400 ? (JSON.parse(profileRes.body) as UserProfile) : profile;

        if (nextInfo) setHubInfo(nextInfo);
        setAgents(nextAgents);
        setQuestions(nextQuestions);
        setNodes(nextNodes);
        if (nextQuota) setQuota(nextQuota);
        if (nextProfile) setProfile(nextProfile);
        fetchAvatars(t, nextAgents);
        persistSnapshot({
          hubInfo: nextInfo,
          agents: nextAgents,
          questions: nextQuestions,
          nodes: nextNodes,
          quota: nextQuota,
          profile: nextProfile,
        });
      } finally {
        setRefreshing(false);
      }
    },
    [agents, fetchAvatars, hubInfo, nodes, persistSnapshot, profile, questions, quota],
  );

  const handleEvent = useCallback(
    (raw: string, t: HubTransport) => {
      let ev: HubEvent;
      try {
        ev = JSON.parse(raw) as HubEvent;
      } catch {
        return;
      }
      if (ev.seq && ev.seq > (lastSeqRef.current ?? 0)) {
        lastSeqRef.current = ev.seq;
      }
      const data = (ev.data ?? {}) as Record<string, unknown>;

      switch (ev.type) {
        case "agent.created":
        case "agent.updated": {
          const agent = data.agent as Agent | undefined;
          if (!agent) break;
          setAgents((prev) => {
            const idx = prev.findIndex((a) => a.id === agent.id);
            if (idx >= 0) {
              const copy = [...prev];
              copy[idx] = agent;
              return copy;
            }
            return [agent, ...prev];
          });
          fetchAvatars(t, [agent]);
          break;
        }
        case "agent.removed": {
          const id = data.id as string;
          setAgents((prev) => prev.filter((a) => a.id !== id));
          break;
        }
        case "agent.status_changed": {
          const id = data.id as string;
          const to = data.to as Agent["status"];
          setAgents((prev) => prev.map((a) => (a.id === id ? { ...a, status: to } : a)));
          break;
        }
        case "agent.model_changed": {
          const id = data.id as string;
          const model = (data.model as string | null) ?? null;
          setAgents((prev) => prev.map((a) => (a.id === id ? { ...a, model } : a)));
          break;
        }
        case "human.asked":
        case "human.answered": {
          const item = data.item as HumanItem | undefined;
          if (!item) break;
          setQuestions((prev) => {
            const idx = prev.findIndex((q) => q.id === item.id);
            if (idx >= 0) {
              const copy = [...prev];
              copy[idx] = item;
              return copy;
            }
            return [item, ...prev];
          });
          break;
        }
        case "human.dismissed": {
          const id = data.id as string;
          setQuestions((prev) => prev.map((q) => (q.id === id ? { ...q, status: "dismissed" } : q)));
          break;
        }
        case "node.changed": {
          void t
            .request("GET", "/v1/nodes")
            .then((r) => r.status < 400 && setNodes((JSON.parse(r.body) as { nodes: RemoteNodeInfo[] }).nodes))
            .catch(() => {});
          break;
        }
        case "account.quota_updated":
        case "account.created":
        case "account.removed": {
          void t
            .request("GET", "/v1/quota")
            .then((r) => r.status < 400 && setQuota(JSON.parse(r.body) as QuotaResponse))
            .catch(() => {});
          break;
        }
        case "device.revoked": {
          const dev = data.device as { id?: string } | undefined;
          if (dev?.id && dev.id === pairingRef.current?.deviceId) {
            sseRef.current?.cancel();
            setConnection({ kind: "revoked", message: "该设备授权已在中心被吊销，请重新扫码配对" });
          }
          break;
        }
      }

      if (ev.type.startsWith("agent.")) {
        const targetId = (data.id as string | undefined) ?? (data.agent as Agent | undefined)?.id;
        if (targetId) {
          setTranscripts((prev) => {
            const cur = prev[targetId];
            if (!cur) return prev;
            return { ...prev, [targetId]: applyTranscriptEvent(cur, ev) };
          });
        }
      }
    },
    [fetchAvatars],
  );

  const teardown = useCallback(async () => {
    if (reconnectTimerRef.current) {
      window.clearTimeout(reconnectTimerRef.current);
      reconnectTimerRef.current = null;
    }
    sseRef.current?.cancel();
    sseRef.current = null;
    const prev = transportRef.current;
    transportRef.current = null;
    setTransport(null);
    if (prev) await prev.close();
  }, []);

  const connect = useCallback(
    async (record: PairingRecord) => {
      const epoch = ++connectEpochRef.current;
      await teardown();
      setConnection({
        kind: "connecting",
        detail: record.mode === "iroh" ? "正在经加密中继连接中心…" : `正在连接 ${record.host}…`,
      });

      try {
        const t = await connectTransport(record);
        if (epoch !== connectEpochRef.current) {
          await t.close();
          return;
        }
        transportRef.current = t;
        setTransport(t);
        setConnection({ kind: "connected", mode: t.mode });

        await bootstrap(t);
        if (epoch !== connectEpochRef.current) return;

        sseRef.current = await t.subscribeEvents(
          lastSeqRef.current,
          (raw) => handleEvent(raw, t),
          (err) => {
            if (epoch !== connectEpochRef.current) return;
            if (err.code === "unauthorized") {
              setConnection({ kind: "revoked", message: err.message });
              return;
            }
            setConnection({ kind: "hub_offline", message: "实时连接已断开，正在重连…" });
            reconnectTimerRef.current = window.setTimeout(() => {
              if (pairingRef.current && epoch === connectEpochRef.current) {
                void connect(pairingRef.current);
              }
            }, 3000);
          },
        );
      } catch (err) {
        if (epoch !== connectEpochRef.current) return;
        const te = err instanceof TransportError ? err : new TransportError("hub_offline", String(err));
        if (te.code === "unauthorized") {
          setConnection({ kind: "revoked", message: te.message });
        } else if (te.code === "relay_unreachable") {
          setConnection({ kind: "relay_unreachable", message: te.message });
        } else {
          setConnection({ kind: "hub_offline", message: te.message });
        }
      }
    },
    [bootstrap, handleEvent, teardown],
  );

  useEffect(() => {
    if (!pairing) {
      void teardown();
      setConnection({ kind: "unpaired" });
      return;
    }
    void connect(pairing);
    return () => {
      connectEpochRef.current++;
      void teardown();
    };
  }, [pairing]);

  // Recover automatically when returning from iOS background / lock screen.
  useEffect(() => {
    const onWake = () => {
      if (document.visibilityState === "hidden") return;
      const rec = pairingRef.current;
      if (!rec) return;
      const cur = transportRef.current;
      if (!cur || cur.isClosed()) {
        void connect(rec);
      } else {
        void cur
          .request("GET", "/v1/info")
          .then((r) => {
            if (r.status < 400) void bootstrap(cur);
            else void connect(rec);
          })
          .catch(() => void connect(rec));
      }
    };
    document.addEventListener("visibilitychange", onWake);
    window.addEventListener("online", onWake);
    return () => {
      document.removeEventListener("visibilitychange", onWake);
      window.removeEventListener("online", onWake);
    };
  }, [bootstrap, connect]);

  if (!pairing) {
    return (
      <PairingScreen
        onPaired={(record) => {
          savePairing(record);
          lastSeqRef.current = undefined;
          setPairing(record);
        }}
      />
    );
  }

  const openQuestions = questions.filter((q) => q.status === "open");
  const isConnected = connection.kind === "connected";

  if (openAgentId) {
    return (
      <SessionScreen
        agentId={openAgentId}
        agent={agents.find((a) => a.id === openAgentId)}
        transcript={transcripts[openAgentId]}
        nodes={nodes}
        hubInfo={hubInfo}
        fallbackHost={pairing.host}
        pendingQuestions={openQuestions.filter((q) => q.from === openAgentId)}
        transport={transport}
        onBack={() => setOpenAgentId(null)}
        onLoadedTranscript={(t) => {
          setTranscripts((prev) => ({ ...prev, [t.agent.id]: t }));
          if (t.last_seq > (lastSeqRef.current ?? 0)) lastSeqRef.current = t.last_seq;
        }}
        onAnsweredQuestion={(updated) => {
          setQuestions((prev) => prev.map((q) => (q.id === updated.id ? updated : q)));
        }}
      />
    );
  }

  const banner =
    connection.kind === "connecting"
      ? { tone: "info", text: connection.detail, action: undefined }
      : connection.kind === "relay_unreachable"
        ? { tone: "warn", text: `中继不可达：${connection.message}`, action: "重试" }
        : connection.kind === "hub_offline"
          ? { tone: "warn", text: connection.message, action: "重连" }
          : connection.kind === "revoked"
            ? { tone: "danger", text: connection.message, action: "重新配对" }
            : null;

  return (
    <div className={`app-shell ${banner ? "has-banner" : ""}`}>
      {banner && (
        <div className={`status-banner ${banner.tone}`}>
          <span className="status-banner-msg">{banner.text}</span>
          {banner.action && (
            <button
              type="button"
              className="status-banner-btn"
              onClick={() => {
                if (connection.kind === "revoked") {
                  clearPairing();
                  setPairing(null);
                } else {
                  void connect(pairing);
                }
              }}
            >
              {banner.action}
            </button>
          )}
        </div>
      )}

      {tab === "chats" && (
        <HomeChatsView
          agents={agents}
          nodes={nodes}
          hubInfo={hubInfo}
          fallbackHost={pairing.host}
          openQuestions={openQuestions}
          avatarUrls={avatarUrls}
          snapshotSavedAt={snapshotSavedAt}
          isConnected={isConnected}
          refreshing={refreshing}
          onRefresh={() => (transport ? void bootstrap(transport) : void connect(pairing))}
          onOpenAgent={(id) => setOpenAgentId(id)}
          onOpenInbox={() => setTab("inbox")}
          onOpenNewChat={() => setShowingNewChat(true)}
        />
      )}

      {tab === "inbox" && (
        <InboxView
          questions={questions}
          agents={agents}
          nodes={nodes}
          hubInfo={hubInfo}
          fallbackHost={pairing.host}
          highlightId={highlightQuestionId}
          transport={transport}
          isConnected={isConnected}
          onRefresh={() => (transport ? void bootstrap(transport) : void connect(pairing))}
          onUpdatedQuestion={(updated) => {
            setQuestions((prev) => prev.map((q) => (q.id === updated.id ? updated : q)));
          }}
        />
      )}

      {tab === "status" && (
        <StatusView
          pairing={pairing}
          connection={connection}
          hubInfo={hubInfo}
          nodes={nodes}
          quota={quota}
          transport={transport}
          onReconnect={() => void connect(pairing)}
          onRefresh={() => (transport ? void bootstrap(transport) : void connect(pairing))}
          onForget={() => {
            clearPairing();
            setPairing(null);
            setAgents([]);
            setQuestions([]);
            setNodes([]);
            setQuota(undefined);
            setTranscripts({});
          }}
        />
      )}

      <nav className="tab-bar" aria-label="主导航">
        <button type="button" className={`tab-btn ${tab === "chats" ? "active" : ""}`} onClick={() => setTab("chats")}>
          <span className="tab-icon-wrap">
            <MessageSquare size={22} />
          </span>
          <span>对话</span>
        </button>
        <button type="button" className={`tab-btn ${tab === "inbox" ? "active" : ""}`} onClick={() => setTab("inbox")}>
          <span className="tab-icon-wrap">
            <Bell size={22} />
            {openQuestions.length > 0 && <span className="badge-pill">{openQuestions.length}</span>}
          </span>
          <span>待办</span>
        </button>
        <button type="button" className={`tab-btn ${tab === "status" ? "active" : ""}`} onClick={() => setTab("status")}>
          <span className="tab-icon-wrap">
            <Gauge size={22} />
          </span>
          <span>概览</span>
        </button>
      </nav>

      {showingNewChat && transport && (
        <NewChatModal
          transport={transport}
          nodes={nodes}
          hubInfo={hubInfo}
          fallbackHost={pairing.host}
          onClose={() => setShowingNewChat(false)}
          onCreated={(created) => {
            setAgents((prev) => [created, ...prev.filter((a) => a.id !== created.id)]);
            setShowingNewChat(false);
            setOpenAgentId(created.id);
          }}
        />
      )}
    </div>
  );
}
