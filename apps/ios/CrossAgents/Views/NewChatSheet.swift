import SwiftUI

struct NewChatSheet: View {
    @Environment(AppStore.self) private var store
    @Environment(\.dismiss) private var dismiss

    let onCreated: (Agent) -> Void

    @State private var selectedNodeId = "local"
    @State private var cwd = ""
    @State private var plugins: [PluginInfo] = []
    @State private var selectedPluginName = ""
    @State private var selectedModel = ""
    @State private var selectedEffort = ""
    @State private var firstMessage = ""

    @State private var isLoadingPlugins = false
    @State private var isCreating = false
    @State private var showingDirPicker = false
    @State private var errorMessage: String?

    private var onlineNodes: [RemoteNodeInfo] {
        let active = store.nodes.filter(\.online)
        if active.isEmpty {
            return [RemoteNodeInfo(
                id: "local",
                name: store.pairing?.host ?? "中心电脑",
                os: store.hubInfo?.machine?.os ?? "windows",
                arch: "",
                hostname: store.hubInfo?.machine?.hostname ?? (store.pairing?.host ?? "中心电脑"),
                shell: "",
                defaultCwd: "",
                chatsDir: nil,
                version: store.hubInfo?.version ?? "",
                connectedAt: "",
                local: true,
                online: true,
                plugins: []
            )]
        }
        return active.sorted { lhs, rhs in
            if lhs.local != rhs.local { return lhs.local }
            return lhs.displayName < rhs.displayName
        }
    }

    private var selectedNodeName: String {
        onlineNodes.first(where: { $0.id == selectedNodeId })?.displayName ?? store.nodeDisplayName(for: selectedNodeId)
    }

    private var installedPlugins: [PluginInfo] {
        let installed = plugins.filter(\.isInstalled)
        return installed.isEmpty ? plugins : installed
    }

    private var currentPlugin: PluginInfo? {
        installedPlugins.first { $0.name == selectedPluginName }
    }

    private var availableModels: [String] {
        guard let plugin = currentPlugin else { return [] }
        return plugin.models.isEmpty ? plugin.allModels : plugin.models
    }

    private var availableEfforts: [String] {
        currentPlugin?.availableEfforts(for: selectedModel.isEmpty ? nil : selectedModel) ?? []
    }

    var body: some View {
        NavigationStack {
            Form {
                Section("目标机器") {
                    Picker("运行机器", selection: $selectedNodeId) {
                        ForEach(onlineNodes) { node in
                            Label(node.displayName, systemImage: node.sfSymbol)
                                .tag(node.id)
                        }
                    }

                    Button {
                        showingDirPicker = true
                    } label: {
                        HStack {
                            Label("工作目录", systemImage: "folder")
                                .foregroundStyle(.primary)
                            Spacer()
                            Text(cwd.isEmpty ? "默认对话目录" : cwd)
                                .font(.footnote.monospaced())
                                .foregroundStyle(.secondary)
                                .lineLimit(1)
                                .truncationMode(.head)
                            Image(systemName: "chevron.right")
                                .font(.caption)
                                .foregroundStyle(.tertiary)
                        }
                    }
                }

                Section("Agent 与模型") {
                    if isLoadingPlugins {
                        HStack {
                            ProgressView()
                            Text("正在读取该机器的可用 CLI…")
                                .font(.subheadline)
                                .foregroundStyle(.secondary)
                        }
                    } else if installedPlugins.isEmpty {
                        Text("这台机器暂未检测到可用 CLI 插件。")
                            .font(.subheadline)
                            .foregroundStyle(.secondary)
                    } else {
                        Picker("CLI / 引擎", selection: $selectedPluginName) {
                            ForEach(installedPlugins) { plugin in
                                Text(plugin.name).tag(plugin.name)
                            }
                        }

                        if !availableModels.isEmpty {
                            Picker("模型", selection: $selectedModel) {
                                Text("默认模型").tag("")
                                ForEach(availableModels, id: \.self) { model in
                                    Text(model).tag(model)
                                }
                            }
                        }

                        if !availableEfforts.isEmpty {
                            Picker("推理力度", selection: $selectedEffort) {
                                Text("默认").tag("")
                                ForEach(availableEfforts, id: \.self) { effort in
                                    Text(effort).tag(effort)
                                }
                            }
                        }
                    }
                }

                Section {
                    TextField("可选：直接输入第一条指令，创建后立即发送…", text: $firstMessage, axis: .vertical)
                        .lineLimit(3...6)
                } header: {
                    Text("开场消息（可选）")
                }

                if let errorMessage {
                    Section {
                        Text(errorMessage)
                            .font(.footnote)
                            .foregroundStyle(.red)
                    }
                }
            }
            .navigationTitle("新建对话")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("取消") { dismiss() }
                        .disabled(isCreating)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button {
                        Task { await create() }
                    } label: {
                        if isCreating {
                            ProgressView()
                        } else {
                            Text("创建").fontWeight(.semibold)
                        }
                    }
                    .disabled(isCreating || selectedPluginName.isEmpty)
                }
            }
            .sheet(isPresented: $showingDirPicker) {
                RemoteDirSheet(
                    nodeId: selectedNodeId,
                    nodeName: selectedNodeName,
                    initialPath: cwd
                ) { chosen in
                    cwd = chosen
                }
            }
            .task {
                if let local = onlineNodes.first(where: \.local) {
                    selectedNodeId = local.id
                } else if let first = onlineNodes.first {
                    selectedNodeId = first.id
                }
                await loadPlugins(for: selectedNodeId)
            }
            .onChange(of: selectedNodeId) { _, newNode in
                cwd = ""
                Task { await loadPlugins(for: newNode) }
            }
            .onChange(of: selectedPluginName) { _, _ in
                syncModelAndEffortDefaults()
            }
            .onChange(of: selectedModel) { _, _ in
                if !availableEfforts.contains(selectedEffort) {
                    selectedEffort = ""
                }
            }
        }
    }

    private func loadPlugins(for nodeId: String) async {
        isLoadingPlugins = true
        errorMessage = nil
        defer { isLoadingPlugins = false }
        do {
            let loaded = try await store.fetchPlugins(node: nodeId == "local" ? nil : nodeId)
            plugins = loaded
            let installed = loaded.filter(\.isInstalled)
            let pool = installed.isEmpty ? loaded : installed
            if !pool.contains(where: { $0.name == selectedPluginName }) {
                selectedPluginName = pool.first?.name ?? ""
            }
            syncModelAndEffortDefaults()
        } catch {
            plugins = store.localPlugins
            errorMessage = error.localizedDescription
        }
    }

    private func syncModelAndEffortDefaults() {
        let models = availableModels
        if !models.contains(selectedModel) {
            selectedModel = models.first ?? ""
        }
        let efforts = availableEfforts
        if !efforts.contains(selectedEffort) {
            selectedEffort = ""
        }
    }

    private func create() async {
        guard !selectedPluginName.isEmpty else { return }
        isCreating = true
        errorMessage = nil
        defer { isCreating = false }
        do {
            let created = try await store.createChat(
                node: selectedNodeId,
                cwd: cwd,
                plugin: selectedPluginName,
                model: selectedModel.isEmpty ? nil : selectedModel,
                effort: selectedEffort.isEmpty ? nil : selectedEffort,
                firstMessage: firstMessage
            )
            dismiss()
            onCreated(created)
        } catch {
            errorMessage = error.localizedDescription
        }
    }
}

private extension RemoteNodeInfo {
    init(
        id: String,
        name: String,
        os: String,
        arch: String,
        hostname: String,
        shell: String,
        defaultCwd: String,
        chatsDir: String?,
        version: String,
        connectedAt: String,
        local: Bool,
        online: Bool,
        plugins: [NodePluginInfo]
    ) {
        self.id = id
        self.name = name
        self.os = os
        self.arch = arch
        self.hostname = hostname
        self.shell = shell
        self.defaultCwd = defaultCwd
        self.chatsDir = chatsDir
        self.version = version
        self.connectedAt = connectedAt
        self.local = local
        self.online = online
        self.plugins = plugins
    }
}
