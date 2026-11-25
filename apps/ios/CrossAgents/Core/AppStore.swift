import Foundation
import Observation
import UIKit

enum ConnectionPhase: Equatable {
    case unpaired
    case connecting
    case connected(direct: Bool)
    case reconnecting(String)
    case revoked(String)

    var isConnected: Bool {
        if case .connected = self { return true }
        return false
    }

    var isDirect: Bool? {
        if case .connected(let direct) = self { return direct }
        return nil
    }
}

@MainActor
@Observable
final class AppStore {
    var pairing: PairingRecord?
    var phase: ConnectionPhase = .unpaired

    var hubInfo: HubInfo?
    var userProfile: UserProfile?
    var userAvatarImage: UIImage?

    var agents: [Agent] = []
    var nodes: [RemoteNodeInfo] = []
    var localPlugins: [PluginInfo] = []
    var questions: [HumanItem] = []
    var quota: QuotaResponse?

    /// Cached live transcripts keyed by agent id.
    var transcripts: [String: Transcript] = [:]
    /// Cached hub gallery avatars keyed by avatar hash id.
    var avatarImages: [String: UIImage] = [:]
    /// Cached message image attachments keyed by "\(agentId)/\(attachmentId)".
    var attachmentImages: [String: UIImage] = [:]

    var lastError: String?
    var isRefreshing = false

    private let tunnel = TunnelRunner()
    private var client: HubClient?
    private var tunnelTask: Task<Void, Never>?
    private var sseTask: Task<Void, Never>?
    private var lastEventSeq: UInt64?
    private var loadingAvatars = Set<String>()
    private var loadingAttachments = Set<String>()
    private var lastRefreshTime: Date?

    init() {
        if let saved = KeychainStore.load() {
            self.pairing = saved
            startConnection(with: saved)
        }
    }

    // MARK: - Computed Helpers

    var openQuestions: [HumanItem] {
        questions.filter(\.isOpen).sorted { $0.createdAt > $1.createdAt }
    }

    var answeredQuestions: [HumanItem] {
        questions.filter { !$0.isOpen }.sorted { ($0.answeredAt ?? $0.createdAt) > ($1.answeredAt ?? $1.createdAt) }
    }

    func openQuestions(for agentId: String) -> [HumanItem] {
        openQuestions.filter { $0.from == agentId }
    }

    func agent(by id: String) -> Agent? {
        agents.first { $0.id == id }
    }

    func nodeDisplayName(for nodeId: String?) -> String {
        let key = (nodeId ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        if key.isEmpty || key == "local" {
            if let local = nodes.first(where: \.local) {
                return local.displayName
            }
            if let host = hubInfo?.machine?.hostname, !host.isEmpty {
                return "\(host)（中心）"
            }
            return pairing?.host ?? "中心电脑"
        }
        if let found = nodes.first(where: { $0.id == key || $0.name == key || $0.hostname == key }) {
            return found.displayName
        }
        return key
    }

    func nodeInfo(for nodeId: String?) -> RemoteNodeInfo? {
        let key = (nodeId ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        if key.isEmpty || key == "local" {
            return nodes.first(where: \.local)
        }
        return nodes.first { $0.id == key || $0.name == key || $0.hostname == key }
    }

    // MARK: - Pairing & Tunnel Lifecycle

    func pair(code: String, deviceName: String) async throws {
        let record = try await LinkBridge.redeem(code: code, deviceName: deviceName)
        try KeychainStore.save(record)
        self.pairing = record
        self.lastError = nil
        startConnection(with: record)
    }

    func reconnect() {
        guard let pairing else { return }
        lastEventSeq = nil
        startConnection(with: pairing)
    }

    func forget() {
        stopConnection()
        KeychainStore.clear()
        pairing = nil
        phase = .unpaired
        hubInfo = nil
        userProfile = nil
        userAvatarImage = nil
        agents = []
        nodes = []
        localPlugins = []
        questions = []
        quota = nil
        transcripts = [:]
        avatarImages = [:]
        attachmentImages = [:]
        lastError = nil
        lastEventSeq = nil
    }

    func handleSceneBecameActive() {
        guard let pairing else { return }
        if case .revoked = phase { return }
        Task {
            if let client {
                do {
                    _ = try await client.info()
                    await refreshAll()
                    if sseTask == nil || sseTask?.isCancelled == true {
                        startSSE(client: client)
                    }
                    return
                } catch {
                    // Tunnel likely slept while iOS suspended the app — rebuild cleanly.
                }
            }
            startConnection(with: pairing)
        }
    }

    private func stopConnection() {
        sseTask?.cancel()
        sseTask = nil
        tunnelTask?.cancel()
        tunnelTask = nil
        tunnel.stop()
        client = nil
    }

    private func startConnection(with record: PairingRecord) {
        stopConnection()
        phase = .connecting
        lastError = nil

        let stream = tunnel.start(record: record)
        tunnelTask = Task { [weak self] in
            guard let self else { return }
            for await event in stream {
                if Task.isCancelled { break }
                await self.handleTunnelEvent(event, record: record)
            }
        }
    }

    private func handleTunnelEvent(_ event: LinkEvent, record: PairingRecord) async {
        switch event {
        case .listening(let port):
            let newClient = HubClient(port: port, token: record.token)
            self.client = newClient
            Task { [weak self] in
                guard let self else { return }
                await self.bootstrap(client: newClient)
                self.startSSE(client: newClient)
            }
        case .connected(let direct):
            phase = .connected(direct: direct)
            lastError = nil
            if let client, lastRefreshTime == nil {
                await bootstrap(client: client)
            }
        case .path(let direct):
            phase = .connected(direct: direct)
        case .unreachable(let error):
            phase = .reconnecting(error)
        case .unauthorized(let error):
            phase = .revoked(error)
            sseTask?.cancel()
            sseTask = nil
        }
    }

    private func bootstrap(client: HubClient) async {
        do {
            async let infoTask = client.info()
            async let agentsTask = client.listAgents()
            async let nodesTask = client.listNodes()
            async let pluginsTask = client.listPlugins()
            async let questionsTask = client.questions(all: true)
            async let quotaTask = client.quota()
            async let profileTask = client.profile()

            let info = try await infoTask
            self.hubInfo = info
            if !phase.isConnected {
                phase = .connected(direct: phase.isDirect ?? false)
            }

            if let loadedAgents = try? await agentsTask {
                self.agents = loadedAgents
                prefetchAvatars(for: loadedAgents)
            }
            if let loadedNodes = try? await nodesTask {
                self.nodes = loadedNodes
            }
            if let loadedPlugins = try? await pluginsTask {
                self.localPlugins = loadedPlugins
            }
            if let loadedQuestions = try? await questionsTask {
                self.questions = loadedQuestions
            }
            if let loadedQuota = try? await quotaTask {
                self.quota = loadedQuota
            }
            if let loadedProfile = try? await profileTask {
                applyProfile(loadedProfile)
            }
            lastRefreshTime = Date()
            lastError = nil
        } catch HubClientError.unauthorized(let msg) {
            phase = .revoked(msg)
        } catch {
            // Tunnel might still be completing initial QUIC handshake; SSE loop will recover.
            if case .connected = phase {
                lastError = error.localizedDescription
            }
        }
    }

    func refreshAll() async {
        guard let client else {
            reconnect()
            return
        }
        isRefreshing = true
        defer { isRefreshing = false }
        await bootstrap(client: client)
    }

    private func startSSE(client: HubClient) {
        sseTask?.cancel()
        sseTask = Task { [weak self] in
            var backoffSeconds: UInt64 = 1
            while !Task.isCancelled {
                guard let self else { return }
                do {
                    let stream = client.streamEvents(since: self.lastEventSeq)
                    var receivedAny = false
                    for try await event in stream {
                        if Task.isCancelled { return }
                        if !receivedAny {
                            receivedAny = true
                            backoffSeconds = 1
                            if !self.phase.isConnected {
                                self.phase = .connected(direct: self.phase.isDirect ?? false)
                            }
                        }
                        if event.seq > 0 {
                            self.lastEventSeq = max(self.lastEventSeq ?? 0, event.seq)
                        }
                        self.applySSE(event)
                    }
                } catch HubClientError.unauthorized(let msg) {
                    self.phase = .revoked(msg)
                    return
                } catch {
                    if Task.isCancelled { return }
                    if case .revoked = self.phase { return }
                    self.phase = .reconnecting("事件流重连中…")
                }

                try? await Task.sleep(nanoseconds: backoffSeconds * 1_000_000_000)
                backoffSeconds = min(backoffSeconds * 2, 10)
                if !Task.isCancelled {
                    await self.bootstrap(client: client)
                }
            }
        }
    }

    // MARK: - Live SSE Reducer

    private func applySSE(_ event: HubSSEEvent) {
        switch event.payload {
        case .agentCreated(let agent):
            upsertAgent(agent)
        case .agentUpdated(let agent):
            upsertAgent(agent)
            if var t = transcripts[agent.id] {
                t.agent = agent
                transcripts[agent.id] = t
            }
        case .agentRemoved(let id):
            agents.removeAll { $0.id == id }
            transcripts.removeValue(forKey: id)
        case .agentStatusChanged(let id, let to):
            if let idx = agents.firstIndex(where: { $0.id == id }) {
                agents[idx].status = to
            }
            if var t = transcripts[id] {
                t.agent.status = to
                transcripts[id] = t
            }
        case .agentDeliveryChanged(let id, let delivery):
            if var t = transcripts[id] {
                t.delivery = delivery
                transcripts[id] = t
            }
        case .agentInput(let id, let messageId, let text, let from, let attachments, let notices, let queuedReason):
            mutateTranscript(id) { t in
                if let existing = t.turns.firstIndex(where: { $0.messageId == messageId }) {
                    t.turns[existing].input = text
                    t.turns[existing].queuedReason = queuedReason
                } else {
                    t.turns.append(TranscriptTurn(
                        messageId: messageId,
                        input: text,
                        from: from,
                        sentAt: event.ts,
                        startedAt: nil,
                        completedAt: nil,
                        stopReason: nil,
                        items: [],
                        attachments: attachments,
                        notices: notices,
                        queuedReason: queuedReason,
                        withdrawn: false
                    ))
                }
            }
        case .agentInputWithdrawn(let id, let messageId):
            mutateTranscript(id) { t in
                if let idx = t.turns.firstIndex(where: { $0.messageId == messageId }) {
                    t.turns[idx].withdrawn = true
                }
            }
        case .agentSteered(let id, let messageId, let text, let attachments):
            mutateTranscript(id) { t in
                guard let idx = activeTurnIndex(in: t) else { return }
                t.turns[idx].items.append(.steer(messageId: messageId, text: text, attachments: attachments, read: false))
            }
        case .agentSteerRead(let id, let messageId):
            mutateTranscript(id) { t in
                for turnIdx in t.turns.indices {
                    for itemIdx in t.turns[turnIdx].items.indices {
                        if case .steer(let mid, let text, let att, _) = t.turns[turnIdx].items[itemIdx], mid == messageId {
                            t.turns[turnIdx].items[itemIdx] = .steer(messageId: mid, text: text, attachments: att, read: true)
                        }
                    }
                }
            }
        case .agentTurnStarted(let id, let messageId):
            mutateTranscript(id) { t in
                if let idx = t.turns.firstIndex(where: { $0.messageId == messageId }) {
                    t.turns[idx].startedAt = event.ts
                    t.turns[idx].queuedReason = nil
                }
            }
        case .agentOutput(let id, let stream, let text):
            mutateTranscript(id) { t in
                guard let idx = activeTurnIndex(in: t) else { return }
                if let last = t.turns[idx].items.last,
                   case .output(let lastStream, let existing) = last,
                   lastStream == stream {
                    t.turns[idx].items[t.turns[idx].items.count - 1] = .output(stream: stream, text: existing + text)
                } else {
                    t.turns[idx].items.append(.output(stream: stream, text: text))
                }
            }
        case .agentToolCall(let id, let item):
            mutateTranscript(id) { t in
                guard let idx = activeTurnIndex(in: t) else { return }
                if let existingIdx = t.turns[idx].items.lastIndex(where: {
                    if case .toolCall(let tc) = $0 { return tc.toolCallId == item.toolCallId }
                    return false
                }) {
                    t.turns[idx].items[existingIdx] = .toolCall(item)
                } else {
                    t.turns[idx].items.append(.toolCall(item))
                }
            }
        case .agentToolCallUpdated(let id, let toolCallId, let title, let status):
            mutateTranscript(id) { t in
                guard let idx = activeTurnIndex(in: t) else { return }
                if let itemIdx = t.turns[idx].items.lastIndex(where: {
                    if case .toolCall(let tc) = $0 { return tc.toolCallId == toolCallId }
                    return false
                }), case .toolCall(var tc) = t.turns[idx].items[itemIdx] {
                    if let title { tc.title = title }
                    if let status { tc.status = status }
                    t.turns[idx].items[itemIdx] = .toolCall(tc)
                }
            }
        case .agentToolOutput(let id, let toolCallId, let text):
            mutateTranscript(id) { t in
                guard let idx = activeTurnIndex(in: t) else { return }
                if let itemIdx = t.turns[idx].items.lastIndex(where: {
                    if case .toolCall(let tc) = $0 { return tc.toolCallId == toolCallId }
                    return false
                }), case .toolCall(var tc) = t.turns[idx].items[itemIdx] {
                    tc.output = (tc.output ?? "") + text
                    t.turns[idx].items[itemIdx] = .toolCall(tc)
                }
            }
        case .agentToolDetail(let id, let toolCallId, let detail):
            mutateTranscript(id) { t in
                guard let idx = activeTurnIndex(in: t) else { return }
                if let itemIdx = t.turns[idx].items.lastIndex(where: {
                    if case .toolCall(let tc) = $0 { return tc.toolCallId == toolCallId }
                    return false
                }), case .toolCall(var tc) = t.turns[idx].items[itemIdx] {
                    tc.detail = detail
                    t.turns[idx].items[itemIdx] = .toolCall(tc)
                }
            }
        case .agentPermission(let id, let kind, let title, let decision):
            mutateTranscript(id) { t in
                guard let idx = activeTurnIndex(in: t) else { return }
                t.turns[idx].items.append(.permission(kind: kind, title: title, decision: decision))
            }
        case .agentWarning(let id, let message):
            mutateTranscript(id) { t in
                guard let idx = activeTurnIndex(in: t) else { return }
                t.turns[idx].items.append(.warning(message: message))
            }
        case .agentError(let id, let message):
            mutateTranscript(id) { t in
                t.error = message
                if let idx = activeTurnIndex(in: t) {
                    t.turns[idx].items.append(.error(message: message))
                }
            }
        case .agentTurnCompleted(let id, let messageId, let stopReason):
            mutateTranscript(id) { t in
                if let idx = t.turns.firstIndex(where: { $0.messageId == messageId }) {
                    t.turns[idx].completedAt = event.ts
                    t.turns[idx].stopReason = stopReason
                }
            }
        case .agentModelChanged(let id, let model, _):
            if let idx = agents.firstIndex(where: { $0.id == id }) {
                agents[idx].model = model
            }
            if var t = transcripts[id] {
                t.agent.model = model
                transcripts[id] = t
            }
        case .humanAsked(let item), .humanAnswered(let item):
            upsertQuestion(item)
        case .humanDismissed(let id):
            if let idx = questions.firstIndex(where: { $0.id == id }) {
                questions[idx].status = "dismissed"
            }
        case .nodeChanged:
            Task { await refreshNodes() }
        case .quotaUpdated:
            Task { await refreshQuota() }
        case .profileUpdated:
            Task { await refreshProfile() }
        case .pluginChanged:
            Task { await refreshPlugins() }
        case .deviceRevoked(let deviceId):
            if deviceId == pairing?.deviceId {
                phase = .revoked("该设备已在中心被吊销")
                sseTask?.cancel()
            }
        case .ignored:
            break
        }
    }

    private func activeTurnIndex(in transcript: Transcript) -> Int? {
        if let running = transcript.turns.lastIndex(where: { $0.startedAt != nil && $0.completedAt == nil }) {
            return running
        }
        return transcript.turns.indices.last
    }

    private func mutateTranscript(_ agentId: String, _ mutate: (inout Transcript) -> Void) {
        guard var current = transcripts[agentId] else { return }
        mutate(&current)
        transcripts[agentId] = current
    }

    private func upsertAgent(_ agent: Agent) {
        if let idx = agents.firstIndex(where: { $0.id == agent.id }) {
            agents[idx] = agent
        } else {
            agents.insert(agent, at: 0)
        }
        prefetchAvatars(for: [agent])
    }

    private func upsertQuestion(_ item: HumanItem) {
        if let idx = questions.firstIndex(where: { $0.id == item.id }) {
            questions[idx] = item
        } else {
            questions.insert(item, at: 0)
        }
    }

    // MARK: - Actions

    func loadTranscript(agentId: String) async throws -> Transcript {
        guard let client else { throw HubClientError.transport("尚未连接到中心") }
        let t = try await client.transcript(agentId: agentId)
        transcripts[agentId] = t
        upsertAgent(t.agent)
        if lastEventSeq == nil || t.lastSeq > (lastEventSeq ?? 0) {
            lastEventSeq = t.lastSeq
        }
        return t
    }

    func sendMessage(
        agentId: String,
        text: String,
        images: [UIImage] = [],
        now: Bool = false,
        steer: Bool = false
    ) async throws -> SendMessageResponse {
        guard let client else { throw HubClientError.transport("尚未连接到中心") }
        var uploads: [AttachmentUpload] = []
        for (idx, img) in images.enumerated() {
            if let encoded = ImageCodec.prepareUpload(image: img, name: "photo-\(idx + 1).jpg") {
                uploads.append(encoded)
            }
        }
        let req = SendMessageRequest(
            text: text,
            now: now ? true : nil,
            steer: steer ? true : nil,
            attachments: uploads.isEmpty ? nil : uploads,
            clientMessageId: UUID().uuidString.lowercased()
        )
        return try await client.sendMessage(agentId: agentId, req: req)
    }

    func sendMessageNow(agentId: String, messageId: String) async throws {
        guard let client else { throw HubClientError.transport("尚未连接到中心") }
        _ = try await client.sendMessageNow(agentId: agentId, messageId: messageId)
    }

    func cancelAgent(agentId: String) async throws {
        guard let client else { throw HubClientError.transport("尚未连接到中心") }
        try await client.cancelAgent(agentId: agentId)
    }

    func compactAgent(agentId: String) async throws {
        guard let client else { throw HubClientError.transport("尚未连接到中心") }
        try await client.compactAgent(agentId: agentId)
    }

    func setModel(agentId: String, model: String?, effort: String?) async throws {
        guard let client else { throw HubClientError.transport("尚未连接到中心") }
        let updated = try await client.setModel(agentId: agentId, model: model, effort: effort)
        upsertAgent(updated)
        if var t = transcripts[agentId] {
            t.agent = updated
            transcripts[agentId] = t
        }
    }

    func archiveAgent(agentId: String, archived: Bool) async throws {
        guard let client else { throw HubClientError.transport("尚未连接到中心") }
        let updated = try await client.archiveAgent(agentId: agentId, archived: archived)
        upsertAgent(updated)
    }

    func fetchPlugins(node: String?) async throws -> [PluginInfo] {
        guard let client else { throw HubClientError.transport("尚未连接到中心") }
        return try await client.listPlugins(node: node)
    }

    func browseDirs(node: String, path: String = "") async throws -> NodeDirsResponse {
        guard let client else { throw HubClientError.transport("尚未连接到中心") }
        return try await client.nodeDirs(node: node, path: path)
    }

    func createChat(
        node: String?,
        cwd: String,
        plugin: String,
        model: String?,
        effort: String?,
        firstMessage: String?
    ) async throws -> Agent {
        guard let client else { throw HubClientError.transport("尚未连接到中心") }
        var configDict: [String: JSONValue] = [:]
        if let effort, !effort.isEmpty {
            configDict["effort"] = .string(effort)
        }
        let trimmedMsg = firstMessage?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let topic = trimmedMsg.isEmpty ? nil : String(trimmedMsg.prefix(48))
        let targetNode = (node == nil || node == "local" || node?.isEmpty == true) ? nil : node

        let req = CreateAgentRequest(
            name: nil,
            topic: topic,
            plugin: plugin,
            driver: nil,
            model: model,
            cwd: cwd.trimmingCharacters(in: .whitespacesAndNewlines),
            config: .object(configDict),
            profile: AgentProfile(),
            node: targetNode
        )
        let created = try await client.createAgent(req)
        upsertAgent(created)
        if !trimmedMsg.isEmpty {
            _ = try await sendMessage(agentId: created.id, text: trimmedMsg)
        }
        return created
    }

    func answerQuestion(id: String, answer: String) async throws {
        guard let client else { throw HubClientError.transport("尚未连接到中心") }
        let updated = try await client.answerQuestion(id: id, answer: answer)
        upsertQuestion(updated)
    }

    func dismissQuestion(id: String) async throws {
        guard let client else { throw HubClientError.transport("尚未连接到中心") }
        try await client.dismissQuestion(id: id)
        if let idx = questions.firstIndex(where: { $0.id == id }) {
            questions[idx].status = "dismissed"
        }
    }

    // MARK: - Media & Targeted Refreshes

    func loadAvatarIfNeeded(id: String) {
        guard avatarImages[id] == nil, !loadingAvatars.contains(id), let client else { return }
        loadingAvatars.insert(id)
        Task {
            defer { loadingAvatars.remove(id) }
            if let content = try? await client.avatar(id: id),
               let img = ImageCodec.decodeBase64Image(content.data) {
                avatarImages[id] = img
            }
        }
    }

    func loadAttachmentImageIfNeeded(agentId: String, attachmentId: String) {
        let key = "\(agentId)/\(attachmentId)"
        guard attachmentImages[key] == nil, !loadingAttachments.contains(key), let client else { return }
        loadingAttachments.insert(key)
        Task {
            defer { loadingAttachments.remove(key) }
            if let content = try? await client.attachment(agentId: agentId, id: attachmentId),
               let img = ImageCodec.decodeBase64Image(content.data) {
                attachmentImages[key] = img
            }
        }
    }

    private func prefetchAvatars(for agents: [Agent]) {
        for agent in agents {
            if let imgId = agent.profile.avatarImage, !imgId.isEmpty {
                loadAvatarIfNeeded(id: imgId)
            }
        }
    }

    private func applyProfile(_ profile: UserProfile) {
        self.userProfile = profile
        if let raw = profile.avatar, !raw.isEmpty {
            self.userAvatarImage = ImageCodec.decodeBase64Image(raw)
        } else {
            self.userAvatarImage = nil
        }
    }

    private func refreshNodes() async {
        guard let client, let list = try? await client.listNodes() else { return }
        self.nodes = list
    }

    private func refreshQuota() async {
        guard let client, let q = try? await client.quota() else { return }
        self.quota = q
    }

    private func refreshProfile() async {
        guard let client, let p = try? await client.profile() else { return }
        applyProfile(p)
    }

    private func refreshPlugins() async {
        guard let client, let list = try? await client.listPlugins() else { return }
        self.localPlugins = list
    }
}
