import Foundation

// MARK: - Hub Info & Profile

struct HubMachine: Codable, Sendable, Equatable {
    let os: String
    let hostname: String
}

struct HubInfo: Codable, Sendable, Equatable {
    let version: String
    let commit: String?
    let debug: Bool
    let api: String
    let pid: UInt32
    let home: String
    let startedAt: String
    let features: [String]
    let machine: HubMachine?

    enum CodingKeys: String, CodingKey {
        case version, commit, debug, api, pid, home, features, machine
        case startedAt = "started_at"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        version = try c.decode(String.self, forKey: .version)
        commit = try c.decodeIfPresent(String.self, forKey: .commit)
        debug = try c.decodeIfPresent(Bool.self, forKey: .debug) ?? false
        api = try c.decodeIfPresent(String.self, forKey: .api) ?? "1"
        pid = try c.decodeIfPresent(UInt32.self, forKey: .pid) ?? 0
        home = try c.decodeIfPresent(String.self, forKey: .home) ?? ""
        startedAt = try c.decodeIfPresent(String.self, forKey: .startedAt) ?? ""
        features = try c.decodeIfPresent([String].self, forKey: .features) ?? []
        machine = try c.decodeIfPresent(HubMachine.self, forKey: .machine)
    }
}

struct UserProfile: Codable, Sendable, Equatable {
    let name: String
    let avatar: String?
}

// MARK: - Lightweight JSON helper for agent config

enum JSONValue: Codable, Sendable, Equatable {
    case string(String)
    case number(Double)
    case bool(Bool)
    case object([String: JSONValue])
    case array([JSONValue])
    case null

    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() {
            self = .null
        } else if let b = try? c.decode(Bool.self) {
            self = .bool(b)
        } else if let n = try? c.decode(Double.self) {
            self = .number(n)
        } else if let s = try? c.decode(String.self) {
            self = .string(s)
        } else if let a = try? c.decode([JSONValue].self) {
            self = .array(a)
        } else if let o = try? c.decode([String: JSONValue].self) {
            self = .object(o)
        } else {
            self = .null
        }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .string(let s): try c.encode(s)
        case .number(let n): try c.encode(n)
        case .bool(let b): try c.encode(b)
        case .object(let o): try c.encode(o)
        case .array(let a): try c.encode(a)
        case .null: try c.encodeNil()
        }
    }

    var stringValue: String? {
        if case .string(let s) = self { return s }
        return nil
    }

    subscript(key: String) -> JSONValue? {
        if case .object(let o) = self { return o[key] }
        return nil
    }
}

// MARK: - Agents

enum AgentStatus: String, Codable, Sendable, Equatable {
    case stopped
    case starting
    case idle
    case running
    case error
    case unknown

    init(from decoder: Decoder) throws {
        let raw = try decoder.singleValueContainer().decode(String.self)
        self = AgentStatus(rawValue: raw) ?? .unknown
    }

    var label: String {
        switch self {
        case .running: return "运行中"
        case .starting: return "启动中"
        case .idle: return "空闲"
        case .stopped: return "已停止"
        case .error: return "异常"
        case .unknown: return "未知"
        }
    }
}

struct AgentProfile: Codable, Sendable, Equatable {
    var displayName: String?
    var title: String?
    var avatar: String?
    var avatarImage: String?
    var color: String?
    var persona: String?

    enum CodingKeys: String, CodingKey {
        case displayName = "display_name"
        case title, avatar
        case avatarImage = "avatar_image"
        case color, persona
    }

    init(
        displayName: String? = nil,
        title: String? = nil,
        avatar: String? = nil,
        avatarImage: String? = nil,
        color: String? = nil,
        persona: String? = nil
    ) {
        self.displayName = displayName
        self.title = title
        self.avatar = avatar
        self.avatarImage = avatarImage
        self.color = color
        self.persona = persona
    }
}

struct Agent: Codable, Sendable, Equatable, Identifiable {
    let id: String
    var name: String
    var plugin: String
    var driver: String?
    var model: String?
    var home: String
    var cwd: String
    var config: JSONValue?
    var status: AgentStatus
    var createdAt: String
    var profile: AgentProfile
    var topic: String?
    var archivedAt: String?
    var node: String?

    enum CodingKeys: String, CodingKey {
        case id, name, plugin, driver, model, home, cwd, config, status, profile, topic, node
        case createdAt = "created_at"
        case archivedAt = "archived_at"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        name = try c.decodeIfPresent(String.self, forKey: .name) ?? id
        plugin = try c.decodeIfPresent(String.self, forKey: .plugin) ?? ""
        driver = try c.decodeIfPresent(String.self, forKey: .driver)
        model = try c.decodeIfPresent(String.self, forKey: .model)
        home = try c.decodeIfPresent(String.self, forKey: .home) ?? ""
        cwd = try c.decodeIfPresent(String.self, forKey: .cwd) ?? ""
        config = try c.decodeIfPresent(JSONValue.self, forKey: .config)
        status = try c.decodeIfPresent(AgentStatus.self, forKey: .status) ?? .stopped
        createdAt = try c.decodeIfPresent(String.self, forKey: .createdAt) ?? ""
        profile = try c.decodeIfPresent(AgentProfile.self, forKey: .profile) ?? AgentProfile()
        topic = try c.decodeIfPresent(String.self, forKey: .topic)
        archivedAt = try c.decodeIfPresent(String.self, forKey: .archivedAt)
        node = try c.decodeIfPresent(String.self, forKey: .node)
    }

    var isArchived: Bool {
        archivedAt != nil && !(archivedAt?.isEmpty ?? true)
    }

    var effectiveNodeId: String {
        let trimmed = node?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return trimmed.isEmpty ? "local" : trimmed
    }

    var currentEffort: String? {
        config?["effort"]?.stringValue
    }

    var shortWorkspace: String {
        let base = home.isEmpty ? cwd : home
        guard !base.isEmpty else { return "" }
        let parts = base.split(whereSeparator: { $0 == "/" || $0 == "\\" })
        return parts.last.map(String.init) ?? base
    }
}

struct ListAgentsResponse: Decodable, Sendable {
    let agents: [Agent]
}

struct CreateAgentRequest: Encodable, Sendable {
    var name: String?
    var topic: String?
    var plugin: String
    var driver: String?
    var model: String?
    var cwd: String
    var config: JSONValue
    var profile: AgentProfile
    var node: String?
}

struct SetModelRequest: Encodable, Sendable {
    let model: String?
    let effort: String?
}

struct ArchiveAgentRequest: Encodable, Sendable {
    let archived: Bool
}

// MARK: - Nodes & Remote Directory Browser

struct NodePluginInfo: Codable, Sendable, Equatable, Identifiable {
    let name: String
    let installed: Bool
    let version: String?
    let driver: String?
    let models: [String]
    let efforts: [String]
    let effortsByModel: [String: [String]]

    var id: String { name }

    enum CodingKeys: String, CodingKey {
        case name, installed, version, driver, models, efforts
        case effortsByModel = "efforts_by_model"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        name = try c.decode(String.self, forKey: .name)
        installed = try c.decodeIfPresent(Bool.self, forKey: .installed) ?? false
        version = try c.decodeIfPresent(String.self, forKey: .version)
        driver = try c.decodeIfPresent(String.self, forKey: .driver)
        models = try c.decodeIfPresent([String].self, forKey: .models) ?? []
        efforts = try c.decodeIfPresent([String].self, forKey: .efforts) ?? []
        effortsByModel = try c.decodeIfPresent([String: [String]].self, forKey: .effortsByModel) ?? [:]
    }
}

struct RemoteNodeInfo: Codable, Sendable, Equatable, Identifiable {
    let id: String
    let name: String
    let os: String
    let arch: String
    let hostname: String
    let shell: String
    let defaultCwd: String
    let chatsDir: String?
    let version: String
    let connectedAt: String
    let local: Bool
    let online: Bool
    let plugins: [NodePluginInfo]

    enum CodingKeys: String, CodingKey {
        case id, name, os, arch, hostname, shell, version, local, online, plugins
        case defaultCwd = "default_cwd"
        case chatsDir = "chats_dir"
        case connectedAt = "connected_at"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        name = try c.decodeIfPresent(String.self, forKey: .name) ?? id
        os = try c.decodeIfPresent(String.self, forKey: .os) ?? "unknown"
        arch = try c.decodeIfPresent(String.self, forKey: .arch) ?? ""
        hostname = try c.decodeIfPresent(String.self, forKey: .hostname) ?? name
        shell = try c.decodeIfPresent(String.self, forKey: .shell) ?? ""
        defaultCwd = try c.decodeIfPresent(String.self, forKey: .defaultCwd) ?? ""
        chatsDir = try c.decodeIfPresent(String.self, forKey: .chatsDir)
        version = try c.decodeIfPresent(String.self, forKey: .version) ?? ""
        connectedAt = try c.decodeIfPresent(String.self, forKey: .connectedAt) ?? ""
        local = try c.decodeIfPresent(Bool.self, forKey: .local) ?? false
        online = try c.decodeIfPresent(Bool.self, forKey: .online) ?? false
        plugins = try c.decodeIfPresent([NodePluginInfo].self, forKey: .plugins) ?? []
    }

    var displayName: String {
        if local {
            return hostname.isEmpty ? "本机中心" : "\(hostname)（中心）"
        }
        return name.isEmpty ? hostname : name
    }

    var sfSymbol: String {
        switch os.lowercased() {
        case "macos", "darwin": return "laptopcomputer"
        case "windows": return "desktopcomputer"
        case "linux": return "server.rack"
        default: return "cpu"
        }
    }
}

struct ListNodesResponse: Decodable, Sendable {
    let nodes: [RemoteNodeInfo]
}

struct NodeDirEntry: Codable, Sendable, Equatable, Identifiable {
    let name: String
    let path: String
    var id: String { path }
}

struct NodeDirsResponse: Codable, Sendable, Equatable {
    let node: String
    let os: String
    let path: String
    let parent: String?
    let dirs: [NodeDirEntry]
    let truncated: Bool
}

// MARK: - Plugins & Capabilities

struct AttachmentSupport: Codable, Sendable, Equatable {
    let image: Bool
    let pdf: Bool
    let text: Bool
}

struct Capabilities: Codable, Sendable, Equatable {
    let streaming: Bool
    let interrupt: Bool
    let steer: Bool
    let setModel: Bool
    let compaction: Bool
    let attachments: AttachmentSupport?

    enum CodingKeys: String, CodingKey {
        case streaming, interrupt, steer, compaction, attachments
        case setModel = "set_model"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        streaming = try c.decodeIfPresent(Bool.self, forKey: .streaming) ?? true
        interrupt = try c.decodeIfPresent(Bool.self, forKey: .interrupt) ?? true
        steer = try c.decodeIfPresent(Bool.self, forKey: .steer) ?? false
        setModel = try c.decodeIfPresent(Bool.self, forKey: .setModel) ?? true
        compaction = try c.decodeIfPresent(Bool.self, forKey: .compaction) ?? false
        attachments = try c.decodeIfPresent(AttachmentSupport.self, forKey: .attachments)
    }
}

struct CliInfo: Codable, Sendable, Equatable {
    let installed: Bool
    let version: String?
    let probing: Bool?
}

struct PluginInfo: Codable, Sendable, Equatable, Identifiable {
    let name: String
    let models: [String]
    let allModels: [String]
    let driver: String
    let capabilities: Capabilities?
    let efforts: [String]
    let effortsByModel: [String: [String]]
    let cli: CliInfo?

    var id: String { name }

    enum CodingKeys: String, CodingKey {
        case name, models, driver, capabilities, efforts, cli
        case allModels = "all_models"
        case effortsByModel = "efforts_by_model"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        name = try c.decode(String.self, forKey: .name)
        models = try c.decodeIfPresent([String].self, forKey: .models) ?? []
        allModels = try c.decodeIfPresent([String].self, forKey: .allModels) ?? models
        driver = try c.decodeIfPresent(String.self, forKey: .driver) ?? ""
        capabilities = try c.decodeIfPresent(Capabilities.self, forKey: .capabilities)
        efforts = try c.decodeIfPresent([String].self, forKey: .efforts) ?? []
        effortsByModel = try c.decodeIfPresent([String: [String]].self, forKey: .effortsByModel) ?? [:]
        cli = try c.decodeIfPresent(CliInfo.self, forKey: .cli)
    }

    var isInstalled: Bool {
        cli?.installed ?? true
    }

    func availableEfforts(for model: String?) -> [String] {
        if let model, let specific = effortsByModel[model] {
            return specific
        }
        return efforts
    }
}

struct ListPluginsResponse: Decodable, Sendable {
    let plugins: [PluginInfo]
}

// MARK: - Messaging & Transcripts

struct AttachmentUpload: Encodable, Sendable {
    let name: String
    let mime: String
    let data: String
}

struct Attachment: Codable, Sendable, Equatable, Identifiable {
    let id: String
    let name: String
    let mime: String
    let size: Int
    let path: String?

    var isImage: Bool {
        mime.lowercased().hasPrefix("image/")
    }
}

struct AttachmentContent: Codable, Sendable, Equatable {
    let id: String
    let mime: String
    let size: Int
    let data: String
}

struct SendMessageRequest: Encodable, Sendable {
    let text: String
    var now: Bool?
    var steer: Bool?
    var attachments: [AttachmentUpload]?
    var clientMessageId: String?

    enum CodingKeys: String, CodingKey {
        case text, now, steer, attachments
        case clientMessageId = "client_message_id"
    }
}

struct SendMessageResponse: Decodable, Sendable {
    let messageId: String
    let queued: Bool
    let steered: Bool
    let queuedReason: String?

    enum CodingKeys: String, CodingKey {
        case queued, steered
        case messageId = "message_id"
        case queuedReason = "queued_reason"
    }
}

struct DeliveryStatus: Codable, Sendable, Equatable {
    let state: String // idle, starting, turn, held
    let queued: Int

    var isBusy: Bool {
        state != "idle"
    }
}

struct TranscriptUsage: Codable, Sendable, Equatable {
    let used: Int
    let size: Int
    let costUsd: Double?

    enum CodingKeys: String, CodingKey {
        case used, size
        case costUsd = "cost_usd"
    }
}

struct FileChange: Codable, Sendable, Equatable {
    let path: String
    let kind: String
    let diff: String
}

struct ToolDetail: Codable, Sendable, Equatable {
    let command: String?
    let cwd: String?
    let exitCode: Int?
    let durationMs: Int?
    let output: String?
    let changes: [FileChange]
    let arguments: String?
    let result: String?
    let error: String?

    enum CodingKeys: String, CodingKey {
        case command, cwd, output, changes, arguments, result, error
        case exitCode = "exit_code"
        case durationMs = "duration_ms"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        command = try c.decodeIfPresent(String.self, forKey: .command)
        cwd = try c.decodeIfPresent(String.self, forKey: .cwd)
        exitCode = try c.decodeIfPresent(Int.self, forKey: .exitCode)
        durationMs = try c.decodeIfPresent(Int.self, forKey: .durationMs)
        output = try c.decodeIfPresent(String.self, forKey: .output)
        changes = try c.decodeIfPresent([FileChange].self, forKey: .changes) ?? []
        arguments = try c.decodeIfPresent(String.self, forKey: .arguments)
        result = try c.decodeIfPresent(String.self, forKey: .result)
        error = try c.decodeIfPresent(String.self, forKey: .error)
    }
}

struct ToolCallItem: Sendable, Equatable {
    let toolCallId: String
    var title: String
    var kind: String?
    var status: String // pending, in_progress, completed, failed
    var output: String?
    var detail: ToolDetail?
}

enum OutputStreamKind: String, Codable, Sendable, Equatable {
    case text
    case thought
}

struct MessageSource: Codable, Sendable, Equatable {
    let kind: String
    let agentId: String?
    let runId: String?

    enum CodingKeys: String, CodingKey {
        case kind
        case agentId = "agent_id"
        case runId = "run_id"
    }
}

enum TranscriptItem: Decodable, Sendable, Equatable {
    case output(stream: OutputStreamKind, text: String)
    case toolCall(ToolCallItem)
    case permission(kind: String, title: String, decision: String)
    case steer(messageId: String, text: String, attachments: [Attachment]?, read: Bool)
    case warning(message: String)
    case error(message: String)
    case unknown(String)

    private enum CodingKeys: String, CodingKey {
        case type, stream, text, title, kind, status, output, detail, decision, message, attachments, read
        case toolCallId = "tool_call_id"
        case messageId = "message_id"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let type = try c.decodeIfPresent(String.self, forKey: .type) ?? "unknown"
        switch type {
        case "output":
            let stream = try c.decodeIfPresent(OutputStreamKind.self, forKey: .stream) ?? .text
            let text = try c.decodeIfPresent(String.self, forKey: .text) ?? ""
            self = .output(stream: stream, text: text)
        case "tool_call":
            let item = ToolCallItem(
                toolCallId: try c.decodeIfPresent(String.self, forKey: .toolCallId) ?? UUID().uuidString,
                title: try c.decodeIfPresent(String.self, forKey: .title) ?? "工具调用",
                kind: try c.decodeIfPresent(String.self, forKey: .kind),
                status: try c.decodeIfPresent(String.self, forKey: .status) ?? "in_progress",
                output: try c.decodeIfPresent(String.self, forKey: .output),
                detail: try c.decodeIfPresent(ToolDetail.self, forKey: .detail)
            )
            self = .toolCall(item)
        case "permission":
            self = .permission(
                kind: try c.decodeIfPresent(String.self, forKey: .kind) ?? "",
                title: try c.decodeIfPresent(String.self, forKey: .title) ?? "权限",
                decision: try c.decodeIfPresent(String.self, forKey: .decision) ?? ""
            )
        case "steer":
            self = .steer(
                messageId: try c.decodeIfPresent(String.self, forKey: .messageId) ?? "",
                text: try c.decodeIfPresent(String.self, forKey: .text) ?? "",
                attachments: try c.decodeIfPresent([Attachment].self, forKey: .attachments),
                read: try c.decodeIfPresent(Bool.self, forKey: .read) ?? false
            )
        case "warning":
            self = .warning(message: try c.decodeIfPresent(String.self, forKey: .message) ?? "")
        case "error":
            self = .error(message: try c.decodeIfPresent(String.self, forKey: .message) ?? "")
        default:
            self = .unknown(type)
        }
    }
}

struct TranscriptTurn: Decodable, Sendable, Equatable, Identifiable {
    let messageId: String
    var input: String
    var from: MessageSource?
    var sentAt: String
    var startedAt: String?
    var completedAt: String?
    var stopReason: String?
    var items: [TranscriptItem]
    var attachments: [Attachment]?
    var notices: [String]?
    var queuedReason: String?
    var withdrawn: Bool?

    var id: String { messageId }

    enum CodingKeys: String, CodingKey {
        case input, from, items, attachments, notices, withdrawn
        case messageId = "message_id"
        case sentAt = "sent_at"
        case startedAt = "started_at"
        case completedAt = "completed_at"
        case stopReason = "stop_reason"
        case queuedReason = "queued_reason"
    }

    init(
        messageId: String,
        input: String,
        from: MessageSource?,
        sentAt: String,
        startedAt: String?,
        completedAt: String?,
        stopReason: String?,
        items: [TranscriptItem],
        attachments: [Attachment]?,
        notices: [String]?,
        queuedReason: String?,
        withdrawn: Bool?
    ) {
        self.messageId = messageId
        self.input = input
        self.from = from
        self.sentAt = sentAt
        self.startedAt = startedAt
        self.completedAt = completedAt
        self.stopReason = stopReason
        self.items = items
        self.attachments = attachments
        self.notices = notices
        self.queuedReason = queuedReason
        self.withdrawn = withdrawn
    }

    var isQueued: Bool {
        startedAt == nil && completedAt == nil && withdrawn != true
    }

    var isRunning: Bool {
        startedAt != nil && completedAt == nil
    }
}

struct Transcript: Decodable, Sendable, Equatable {
    var agent: Agent
    var driver: String?
    var capabilities: Capabilities?
    var delivery: DeliveryStatus
    var turns: [TranscriptTurn]
    var usage: TranscriptUsage?
    var conversationCostUsd: Double?
    var error: String?
    var lastSeq: UInt64

    enum CodingKeys: String, CodingKey {
        case agent, driver, capabilities, delivery, turns, usage, error
        case conversationCostUsd = "conversation_cost_usd"
        case lastSeq = "last_seq"
    }
}

// MARK: - Human Questions & Approvals (Inbox)

struct HumanItem: Codable, Sendable, Equatable, Identifiable {
    let id: String
    let kind: String // question, notice
    let from: String // agent id
    let runId: String?
    let taskId: String?
    let text: String
    let detail: String?
    let options: [String]
    let defaultAnswer: String?
    let timeoutAt: String?
    var status: String // open, answered, defaulted, dismissed
    var answer: String?
    var answeredAt: String?
    let action: String? // budget, approval, work
    let createdAt: String

    enum CodingKeys: String, CodingKey {
        case id, kind, from, text, detail, options, status, answer, action
        case runId = "run_id"
        case taskId = "task_id"
        case defaultAnswer = "default"
        case timeoutAt = "timeout_at"
        case answeredAt = "answered_at"
        case createdAt = "created_at"
    }

    var isOpen: Bool { status == "open" }

    var actionBadge: String {
        switch action {
        case "approval": return "审批"
        case "budget": return "预算"
        case "work": return "任务提案"
        default: return kind == "notice" ? "通知" : "提问"
        }
    }
}

struct ListQuestionsResponse: Decodable, Sendable {
    let items: [HumanItem]
}

struct AnswerRequest: Encodable, Sendable {
    let answer: String
}

// MARK: - Subscription Quota

struct AccountInfo: Codable, Sendable, Equatable, Identifiable {
    let id: String
    let provider: String
    let label: String
}

struct RateWindow: Codable, Sendable, Equatable, Identifiable {
    let name: String
    let usedPercent: Double
    let windowSeconds: Int?
    let resetsAt: String?
    let remaining: Double?
    let total: Double?

    var id: String { "\(name)-\(windowSeconds ?? 0)-\(resetsAt ?? "")" }

    enum CodingKeys: String, CodingKey {
        case name, remaining, total
        case usedPercent = "used_percent"
        case windowSeconds = "window_seconds"
        case resetsAt = "resets_at"
    }

    var clampedUsedFraction: Double {
        min(max(usedPercent / 100.0, 0.0), 1.0)
    }

    var remainingPercent: Int {
        max(0, min(100, Int((100.0 - usedPercent).rounded())))
    }
}

struct LimitGroup: Codable, Sendable, Equatable, Identifiable {
    let id: String
    let name: String?
    let model: String?
    let windows: [RateWindow]
    let reached: Bool
}

struct CreditsInfo: Codable, Sendable, Equatable {
    let hasCredits: Bool
    let unlimited: Bool
    let balance: String?

    enum CodingKeys: String, CodingKey {
        case unlimited, balance
        case hasCredits = "has_credits"
    }
}

struct AccountStatus: Codable, Sendable, Equatable {
    let loggedIn: Bool
    let authMode: String?
    let email: String?
    let plan: String?
    let rateLimits: [RateWindow]
    let limitReached: Bool
    let credits: CreditsInfo?
    let limits: [LimitGroup]
    let asOf: String?
    let note: String?

    enum CodingKeys: String, CodingKey {
        case email, plan, credits, limits, note
        case loggedIn = "logged_in"
        case authMode = "auth_mode"
        case rateLimits = "rate_limits"
        case limitReached = "limit_reached"
        case asOf = "as_of"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        loggedIn = try c.decodeIfPresent(Bool.self, forKey: .loggedIn) ?? false
        authMode = try c.decodeIfPresent(String.self, forKey: .authMode)
        email = try c.decodeIfPresent(String.self, forKey: .email)
        plan = try c.decodeIfPresent(String.self, forKey: .plan)
        rateLimits = try c.decodeIfPresent([RateWindow].self, forKey: .rateLimits) ?? []
        limitReached = try c.decodeIfPresent(Bool.self, forKey: .limitReached) ?? false
        credits = try c.decodeIfPresent(CreditsInfo.self, forKey: .credits)
        limits = try c.decodeIfPresent([LimitGroup].self, forKey: .limits) ?? []
        asOf = try c.decodeIfPresent(String.self, forKey: .asOf)
        note = try c.decodeIfPresent(String.self, forKey: .note)
    }
}

struct AccountQuota: Codable, Sendable, Equatable, Identifiable {
    let account: AccountInfo
    let status: AccountStatus
    let agents: [String]
    let atRisk: String?
    let egressBlock: String?

    var id: String { account.id }

    enum CodingKeys: String, CodingKey {
        case account, status, agents
        case atRisk = "at_risk"
        case egressBlock = "egress_block"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        account = try c.decode(AccountInfo.self, forKey: .account)
        status = try c.decode(AccountStatus.self, forKey: .status)
        agents = try c.decodeIfPresent([String].self, forKey: .agents) ?? []
        atRisk = try c.decodeIfPresent(String.self, forKey: .atRisk)
        egressBlock = try c.decodeIfPresent(String.self, forKey: .egressBlock)
    }
}

struct QuotaResponse: Decodable, Sendable {
    let accounts: [AccountQuota]
    let providers: [String]
}

// MARK: - API Errors

struct ApiErrorEnvelope: Decodable, Sendable {
    struct Body: Decodable, Sendable {
        let code: String
        let message: String
    }
    let error: Body
}

// MARK: - Live SSE Events

struct HubSSEEvent: Decodable, Sendable {
    let seq: UInt64
    let ts: String
    let type: String
    let payload: Payload

    enum Payload: Sendable {
        case agentCreated(Agent)
        case agentUpdated(Agent)
        case agentRemoved(id: String)
        case agentStatusChanged(id: String, to: AgentStatus)
        case agentDeliveryChanged(id: String, delivery: DeliveryStatus)
        case agentInput(id: String, messageId: String, text: String, from: MessageSource?, attachments: [Attachment]?, notices: [String]?, queuedReason: String?)
        case agentInputWithdrawn(id: String, messageId: String)
        case agentSteered(id: String, messageId: String, text: String, attachments: [Attachment]?)
        case agentSteerRead(id: String, messageId: String)
        case agentTurnStarted(id: String, messageId: String)
        case agentOutput(id: String, stream: OutputStreamKind, text: String)
        case agentToolCall(id: String, item: ToolCallItem)
        case agentToolCallUpdated(id: String, toolCallId: String, title: String?, status: String?)
        case agentToolOutput(id: String, toolCallId: String, text: String)
        case agentToolDetail(id: String, toolCallId: String, detail: ToolDetail)
        case agentPermission(id: String, kind: String, title: String, decision: String)
        case agentWarning(id: String, message: String)
        case agentError(id: String, message: String)
        case agentTurnCompleted(id: String, messageId: String, stopReason: String)
        case agentModelChanged(id: String, model: String?, effort: String?)
        case humanAsked(HumanItem)
        case humanAnswered(HumanItem)
        case humanDismissed(id: String)
        case nodeChanged
        case quotaUpdated
        case profileUpdated
        case pluginChanged
        case deviceRevoked(deviceId: String)
        case ignored
    }

    private enum CodingKeys: String, CodingKey {
        case seq, ts, type, data
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        seq = try c.decodeIfPresent(UInt64.self, forKey: .seq) ?? 0
        ts = try c.decodeIfPresent(String.self, forKey: .ts) ?? ""
        type = try c.decode(String.self, forKey: .type)

        switch type {
        case "agent.created":
            struct D: Decodable { let agent: Agent }
            payload = (try? c.decode(D.self, forKey: .data)).map { .agentCreated($0.agent) } ?? .ignored
        case "agent.updated":
            struct D: Decodable { let agent: Agent }
            payload = (try? c.decode(D.self, forKey: .data)).map { .agentUpdated($0.agent) } ?? .ignored
        case "agent.removed":
            struct D: Decodable { let id: String }
            payload = (try? c.decode(D.self, forKey: .data)).map { .agentRemoved(id: $0.id) } ?? .ignored
        case "agent.status_changed":
            struct D: Decodable { let id: String; let to: AgentStatus }
            payload = (try? c.decode(D.self, forKey: .data)).map { .agentStatusChanged(id: $0.id, to: $0.to) } ?? .ignored
        case "agent.delivery_changed":
            struct D: Decodable { let id: String; let delivery: DeliveryStatus }
            payload = (try? c.decode(D.self, forKey: .data)).map { .agentDeliveryChanged(id: $0.id, delivery: $0.delivery) } ?? .ignored
        case "agent.input":
            struct D: Decodable {
                let id: String
                let message_id: String
                let text: String
                let from: MessageSource?
                let attachments: [Attachment]?
                let notices: [String]?
                let queued_reason: String?
            }
            payload = (try? c.decode(D.self, forKey: .data)).map {
                .agentInput(id: $0.id, messageId: $0.message_id, text: $0.text, from: $0.from, attachments: $0.attachments, notices: $0.notices, queuedReason: $0.queued_reason)
            } ?? .ignored
        case "agent.input_withdrawn":
            struct D: Decodable { let id: String; let message_id: String }
            payload = (try? c.decode(D.self, forKey: .data)).map { .agentInputWithdrawn(id: $0.id, messageId: $0.message_id) } ?? .ignored
        case "agent.steered":
            struct D: Decodable { let id: String; let message_id: String; let text: String; let attachments: [Attachment]? }
            payload = (try? c.decode(D.self, forKey: .data)).map { .agentSteered(id: $0.id, messageId: $0.message_id, text: $0.text, attachments: $0.attachments) } ?? .ignored
        case "agent.steer_read":
            struct D: Decodable { let id: String; let message_id: String }
            payload = (try? c.decode(D.self, forKey: .data)).map { .agentSteerRead(id: $0.id, messageId: $0.message_id) } ?? .ignored
        case "agent.turn_started":
            struct D: Decodable { let id: String; let message_id: String }
            payload = (try? c.decode(D.self, forKey: .data)).map { .agentTurnStarted(id: $0.id, messageId: $0.message_id) } ?? .ignored
        case "agent.output":
            struct D: Decodable { let id: String; let stream: OutputStreamKind; let text: String }
            payload = (try? c.decode(D.self, forKey: .data)).map { .agentOutput(id: $0.id, stream: $0.stream, text: $0.text) } ?? .ignored
        case "agent.tool_call":
            struct D: Decodable { let id: String; let tool_call_id: String; let title: String; let kind: String?; let status: String }
            payload = (try? c.decode(D.self, forKey: .data)).map {
                .agentToolCall(id: $0.id, item: ToolCallItem(toolCallId: $0.tool_call_id, title: $0.title, kind: $0.kind, status: $0.status, output: nil, detail: nil))
            } ?? .ignored
        case "agent.tool_call_updated":
            struct D: Decodable { let id: String; let tool_call_id: String; let title: String?; let status: String? }
            payload = (try? c.decode(D.self, forKey: .data)).map { .agentToolCallUpdated(id: $0.id, toolCallId: $0.tool_call_id, title: $0.title, status: $0.status) } ?? .ignored
        case "agent.tool_output":
            struct D: Decodable { let id: String; let tool_call_id: String; let text: String }
            payload = (try? c.decode(D.self, forKey: .data)).map { .agentToolOutput(id: $0.id, toolCallId: $0.tool_call_id, text: $0.text) } ?? .ignored
        case "agent.tool_detail":
            struct D: Decodable { let id: String; let tool_call_id: String; let detail: ToolDetail }
            payload = (try? c.decode(D.self, forKey: .data)).map { .agentToolDetail(id: $0.id, toolCallId: $0.tool_call_id, detail: $0.detail) } ?? .ignored
        case "agent.permission":
            struct D: Decodable { let id: String; let kind: String; let title: String; let decision: String }
            payload = (try? c.decode(D.self, forKey: .data)).map { .agentPermission(id: $0.id, kind: $0.kind, title: $0.title, decision: $0.decision) } ?? .ignored
        case "agent.warning":
            struct D: Decodable { let id: String; let message: String }
            payload = (try? c.decode(D.self, forKey: .data)).map { .agentWarning(id: $0.id, message: $0.message) } ?? .ignored
        case "agent.error":
            struct D: Decodable { let id: String; let message: String }
            payload = (try? c.decode(D.self, forKey: .data)).map { .agentError(id: $0.id, message: $0.message) } ?? .ignored
        case "agent.turn_completed":
            struct D: Decodable { let id: String; let message_id: String; let stop_reason: String }
            payload = (try? c.decode(D.self, forKey: .data)).map { .agentTurnCompleted(id: $0.id, messageId: $0.message_id, stopReason: $0.stop_reason) } ?? .ignored
        case "agent.model_changed":
            struct D: Decodable { let id: String; let model: String?; let effort: String? }
            payload = (try? c.decode(D.self, forKey: .data)).map { .agentModelChanged(id: $0.id, model: $0.model, effort: $0.effort) } ?? .ignored
        case "human.asked":
            struct D: Decodable { let item: HumanItem }
            payload = (try? c.decode(D.self, forKey: .data)).map { .humanAsked($0.item) } ?? .ignored
        case "human.answered":
            struct D: Decodable { let item: HumanItem }
            payload = (try? c.decode(D.self, forKey: .data)).map { .humanAnswered($0.item) } ?? .ignored
        case "human.dismissed":
            struct D: Decodable { let id: String }
            payload = (try? c.decode(D.self, forKey: .data)).map { .humanDismissed(id: $0.id) } ?? .ignored
        case "node.changed":
            payload = .nodeChanged
        case "account.quota_updated", "account.created", "account.removed":
            payload = .quotaUpdated
        case "profile.updated":
            payload = .profileUpdated
        case "plugin.changed":
            payload = .pluginChanged
        case "device.revoked":
            struct D: Decodable {
                struct Dev: Decodable { let id: String }
                let device: Dev
            }
            payload = (try? c.decode(D.self, forKey: .data)).map { .deviceRevoked(deviceId: $0.device.id) } ?? .ignored
        default:
            payload = .ignored
        }
    }
}

// MARK: - Formatting helpers

enum DateHelpers {
    private static let isoWithFractional: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()

    private static let isoStandard: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime]
        return f
    }()

    static func parse(_ raw: String?) -> Date? {
        guard let raw, !raw.isEmpty else { return nil }
        return isoWithFractional.date(from: raw) ?? isoStandard.date(from: raw)
    }

    static func relative(_ raw: String?) -> String {
        guard let date = parse(raw) else { return "" }
        let seconds = Int(Date().timeIntervalSince(date))
        if seconds < 45 { return "刚刚" }
        if seconds < 3600 { return "\(seconds / 60) 分钟前" }
        if seconds < 86_400 { return "\(seconds / 3600) 小时前" }
        let f = DateFormatter()
        f.dateFormat = "MM-dd HH:mm"
        return f.string(from: date)
    }

    static func countdown(until raw: String?) -> String? {
        guard let date = parse(raw) else { return nil }
        let diff = Int(date.timeIntervalSinceNow)
        if diff <= 0 { return "即将重置" }
        let days = diff / 86_400
        let hours = (diff % 86_400) / 3600
        let mins = (diff % 3600) / 60
        if days > 0 { return "\(days) 天 \(hours) 小时后重置" }
        if hours > 0 { return "\(hours) 小时 \(mins) 分钟后重置" }
        return "\(max(1, mins)) 分钟后重置"
    }
}
