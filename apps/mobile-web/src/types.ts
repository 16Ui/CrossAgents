export type AgentStatus = "stopped" | "starting" | "idle" | "running" | "error";

export interface HubMachine {
  os: string;
  hostname: string;
}

export interface HubInfo {
  version: string;
  api: string;
  home: string;
  started_at: string;
  features: string[];
  machine?: HubMachine;
}

export interface UserProfile {
  name: string;
  avatar: string | null;
}

export interface AgentProfile {
  display_name: string | null;
  title: string | null;
  avatar: string | null;
  avatar_image: string | null;
  color: string | null;
  persona: string | null;
}

export interface Agent {
  id: string;
  name: string;
  plugin: string;
  driver: string | null;
  model: string | null;
  home: string;
  cwd: string;
  config?: Record<string, unknown>;
  status: AgentStatus;
  created_at: string;
  profile: AgentProfile;
  topic: string | null;
  archived_at: string | null;
  node: string | null;
}

export interface Attachment {
  id: string;
  name: string;
  mime: string;
  size: number;
  path?: string;
}

export interface AttachmentUpload {
  name: string;
  mime: string;
  data: string;
}

export interface AttachmentContent {
  id: string;
  mime: string;
  size: number;
  data: string;
}

export interface FileChange {
  path: string;
  kind: string;
  diff: string;
}

export interface ToolDetail {
  command?: string;
  cwd?: string;
  exit_code?: number;
  duration_ms?: number;
  output?: string;
  changes?: FileChange[];
  arguments?: string;
  result?: string;
  error?: string;
}

export type TranscriptItem =
  | { type: "output"; stream: "text" | "thought"; text: string }
  | {
      type: "tool_call";
      tool_call_id: string;
      title: string;
      kind: string | null;
      status: "pending" | "in_progress" | "completed" | "failed" | string;
      output?: string;
      detail?: ToolDetail;
    }
  | { type: "permission"; kind: string; title: string; decision: string }
  | { type: "steer"; message_id: string; text: string; attachments?: Attachment[]; read: boolean }
  | { type: "warning"; message: string }
  | { type: "error"; message: string };

export interface TranscriptTurn {
  message_id: string;
  input: string;
  from: { kind: string; agent_id?: string } | null;
  sent_at: string;
  started_at: string | null;
  completed_at: string | null;
  stop_reason: string | null;
  items: TranscriptItem[];
  attachments?: Attachment[];
  notices?: string[];
  queued_reason?: string;
  withdrawn?: boolean;
}

export interface DeliveryStatus {
  state: "idle" | "starting" | "turn" | "held" | string;
  queued: number;
}

export interface Capabilities {
  streaming: boolean;
  interrupt: boolean;
  steer: boolean;
  set_model: boolean;
  compaction: boolean;
}

export interface Transcript {
  agent: Agent;
  driver: string | null;
  capabilities: Capabilities | null;
  delivery: DeliveryStatus;
  turns: TranscriptTurn[];
  error?: string;
  last_seq: number;
}

export interface HumanItem {
  id: string;
  kind: "question" | "notice" | string;
  from: string;
  run_id: string | null;
  task_id: string | null;
  text: string;
  detail: string | null;
  options: string[];
  default: string | null;
  timeout_at: string | null;
  status: "open" | "answered" | "defaulted" | "dismissed" | string;
  answer: string | null;
  answered_at: string | null;
  action: "budget" | "approval" | "work" | null;
  created_at: string;
}

export interface NodePluginInfo {
  name: string;
  installed: boolean;
  version?: string;
  driver?: string;
  models: string[];
  efforts: string[];
  efforts_by_model: Record<string, string[]>;
}

export interface RemoteNodeInfo {
  id: string;
  name: string;
  os: string;
  arch: string;
  hostname: string;
  shell: string;
  default_cwd: string;
  chats_dir?: string;
  version: string;
  connected_at: string;
  local: boolean;
  online: boolean;
  plugins: NodePluginInfo[];
}

export interface NodeDirsResponse {
  node: string;
  os: string;
  path: string;
  parent?: string;
  dirs: { name: string; path: string }[];
  truncated: boolean;
}

export interface PluginInfo {
  name: string;
  models: string[];
  all_models: string[];
  driver: string;
  efforts: string[];
  efforts_by_model: Record<string, string[]>;
  cli?: { installed: boolean; version?: string };
}

export interface RateWindow {
  name: string;
  used_percent: number;
  window_seconds: number | null;
  resets_at: string | null;
  remaining?: number;
  total?: number;
}

export interface LimitGroup {
  id: string;
  name: string | null;
  model: string | null;
  windows: RateWindow[];
  reached: boolean;
}

export interface AccountQuota {
  account: { id: string; provider: string; label: string };
  status: {
    logged_in: boolean;
    email: string | null;
    plan: string | null;
    rate_limits: RateWindow[];
    limit_reached: boolean;
    credits: { has_credits: boolean; unlimited: boolean; balance: string | null } | null;
    limits: LimitGroup[];
    note: string | null;
  };
  agents: string[];
  at_risk?: string;
  egress_block?: string;
}

export interface QuotaResponse {
  accounts: AccountQuota[];
  providers: string[];
}

export interface PairingRecord {
  mode: "iroh" | "direct";
  // iroh fields
  endpoint?: string;
  relay?: string | null;
  customRelay?: boolean;
  addrs?: string[];
  // direct HTTPS fallback fields
  baseUrl?: string;
  // shared identity
  host: string;
  deviceId: string;
  deviceName: string;
  token: string;
  level?: string;
  pairedAt: string;
}

export interface InvitePreview {
  ok: boolean;
  error?: string;
  host?: string;
  expires?: number;
  has_relay?: boolean;
  relay?: string | null;
  custom_relay?: boolean;
  lan_addrs?: number;
}

export type ConnectionState =
  | { kind: "unpaired" }
  | { kind: "connecting"; detail: string }
  | { kind: "connected"; mode: "iroh" | "direct" }
  | { kind: "relay_unreachable"; message: string }
  | { kind: "hub_offline"; message: string }
  | { kind: "revoked"; message: string };

export interface HubEvent {
  seq: number;
  ts: string;
  type: string;
  data?: Record<string, unknown>;
}
