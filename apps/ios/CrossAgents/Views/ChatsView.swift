import SwiftUI

struct ChatsView: View {
    @Environment(AppStore.self) private var store

    @State private var searchText = ""
    @State private var showArchived = false
    @State private var showingNewChat = false
    @State private var navigationPath = NavigationPath()

    private struct MachineGroup: Identifiable {
        let id: String
        let title: String
        let sfSymbol: String
        let online: Bool
        let isLocal: Bool
        let agents: [Agent]
    }

    private var groupedAgents: [MachineGroup] {
        let filtered = store.agents.filter { agent in
            if !showArchived && agent.isArchived { return false }
            guard !searchText.trimmingCharacters(in: .whitespaces).isEmpty else { return true }
            let q = searchText.lowercased()
            let nodeLabel = store.nodeDisplayName(for: agent.node).lowercased()
            return agent.name.lowercased().contains(q)
                || (agent.topic?.lowercased().contains(q) ?? false)
                || agent.shortWorkspace.lowercased().contains(q)
                || (agent.model?.lowercased().contains(q) ?? false)
                || agent.plugin.lowercased().contains(q)
                || nodeLabel.contains(q)
        }

        let buckets = Dictionary(grouping: filtered) { $0.effectiveNodeId }
        return buckets.map { nodeKey, list in
            let info = store.nodeInfo(for: nodeKey)
            let isLocal = nodeKey == "local" || (info?.local ?? false)
            let sortedAgents = list.sorted { lhs, rhs in
                let lp = statusPriority(lhs.status)
                let rp = statusPriority(rhs.status)
                if lp != rp { return lp < rp }
                return lhs.createdAt > rhs.createdAt
            }
            return MachineGroup(
                id: nodeKey,
                title: store.nodeDisplayName(for: nodeKey),
                sfSymbol: info?.sfSymbol ?? (isLocal ? "desktopcomputer" : "laptopcomputer"),
                online: info?.online ?? isLocal,
                isLocal: isLocal,
                agents: sortedAgents
            )
        }
        .sorted { lhs, rhs in
            if lhs.isLocal != rhs.isLocal { return lhs.isLocal }
            return lhs.title.localizedCaseInsensitiveCompare(rhs.title) == .orderedAscending
        }
    }

    private func statusPriority(_ status: AgentStatus) -> Int {
        switch status {
        case .running: return 0
        case .starting: return 1
        case .idle: return 2
        case .error: return 3
        case .stopped, .unknown: return 4
        }
    }

    var body: some View {
        NavigationStack(path: $navigationPath) {
            Group {
                if store.agents.isEmpty && !store.phase.isConnected {
                    ContentUnavailableView(
                        "正在连接中心",
                        systemImage: "antenna.radiowaves.left.and.right",
                        description: Text("连接建立后会自动同步各台机器上的 Agent 对话。")
                    )
                } else if groupedAgents.isEmpty {
                    ContentUnavailableView {
                        Label(searchText.isEmpty ? "还没有对话" : "没有匹配的对话", systemImage: "bubble.left.and.bubble.right")
                    } description: {
                        Text(searchText.isEmpty ? "点右上角「+」在 Windows 中心或 Mac 上新建一个 Agent 对话。" : "试试换个关键词，或在菜单里开启「显示已归档」。")
                    } actions: {
                        if searchText.isEmpty {
                            Button("新建对话") { showingNewChat = true }
                                .buttonStyle(.borderedProminent)
                        }
                    }
                } else {
                    List {
                        ForEach(groupedAgents) { group in
                            Section {
                                ForEach(group.agents) { agent in
                                    NavigationLink(value: agent.id) {
                                        AgentRow(
                                            agent: agent,
                                            pendingQuestions: store.openQuestions(for: agent.id).count
                                        )
                                    }
                                }
                            } header: {
                                HStack(spacing: 6) {
                                    Image(systemName: group.sfSymbol)
                                    Text(group.title)
                                    if !group.online {
                                        Text("离线")
                                            .font(.caption2)
                                            .foregroundStyle(.orange)
                                    }
                                }
                                .font(.caption.weight(.semibold))
                            }
                        }
                    }
                    .listStyle(.insetGrouped)
                }
            }
            .navigationTitle("对话")
            .searchable(text: $searchText, prompt: "搜索 Agent、项目、模型或机器")
            .refreshable {
                await store.refreshAll()
            }
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Menu {
                        Toggle("显示已归档对话", isOn: $showArchived)
                        Button {
                            Task { await store.refreshAll() }
                        } label: {
                            Label("立即刷新", systemImage: "arrow.clockwise")
                        }
                    } label: {
                        Image(systemName: "line.3.horizontal.decrease.circle")
                    }
                }
                ToolbarItem(placement: .primaryAction) {
                    Button {
                        showingNewChat = true
                    } label: {
                        Image(systemName: "plus")
                            .fontWeight(.semibold)
                    }
                    .disabled(!store.phase.isConnected)
                }
            }
            .navigationDestination(for: String.self) { agentId in
                SessionView(agentId: agentId)
            }
            .sheet(isPresented: $showingNewChat) {
                NewChatSheet { created in
                    navigationPath.append(created.id)
                }
            }
        }
    }
}

private struct AgentRow: View {
    let agent: Agent
    let pendingQuestions: Int

    var body: some View {
        HStack(spacing: 12) {
            AgentAvatarView(agent: agent, size: 44)

            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 6) {
                    Text(agent.name)
                        .font(.headline)
                        .lineLimit(1)
                    if agent.isArchived {
                        Text("已归档")
                            .font(.caption2)
                            .padding(.horizontal, 6)
                            .padding(.vertical, 1)
                            .background(.secondary.opacity(0.16), in: Capsule())
                    }
                    Spacer(minLength: 4)
                    if pendingQuestions > 0 {
                        Text("\(pendingQuestions) 待答")
                            .font(.caption2.weight(.semibold))
                            .foregroundStyle(.white)
                            .padding(.horizontal, 7)
                            .padding(.vertical, 2)
                            .background(.red, in: Capsule())
                    }
                    StatusDot(status: agent.status)
                }

                if let topic = agent.topic, !topic.isEmpty {
                    Text(topic)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }

                HStack(spacing: 6) {
                    if !agent.shortWorkspace.isEmpty {
                        Label(agent.shortWorkspace, systemImage: "folder")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                    if let model = agent.model, !model.isEmpty {
                        Text(model)
                            .font(.caption2.monospaced())
                            .foregroundStyle(.secondary)
                            .padding(.horizontal, 6)
                            .padding(.vertical, 2)
                            .background(Color(.tertiarySystemFill), in: Capsule())
                            .lineLimit(1)
                    } else if !agent.plugin.isEmpty {
                        Text(agent.plugin)
                            .font(.caption2.monospaced())
                            .foregroundStyle(.secondary)
                    }
                }
            }
        }
        .padding(.vertical, 3)
    }
}
