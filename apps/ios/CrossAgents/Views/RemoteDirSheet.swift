import SwiftUI

struct RemoteDirSheet: View {
    @Environment(AppStore.self) private var store
    @Environment(\.dismiss) private var dismiss

    let nodeId: String
    let nodeName: String
    let initialPath: String
    let onSelect: (String) -> Void

    @State private var currentPath = ""
    @State private var manualPath = ""
    @State private var response: NodeDirsResponse?
    @State private var isLoading = false
    @State private var errorMessage: String?

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                HStack(spacing: 8) {
                    TextField("输入路径跳转…", text: $manualPath)
                        .font(.system(.footnote, design: .monospaced))
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .submitLabel(.go)
                        .onSubmit {
                            Task { await load(path: manualPath.trimmingCharacters(in: .whitespacesAndNewlines)) }
                        }
                        .padding(8)
                        .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 8))

                    Button("跳转") {
                        Task { await load(path: manualPath.trimmingCharacters(in: .whitespacesAndNewlines)) }
                    }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 10)
                .background(Color(.systemGroupedBackground))

                List {
                    if let errorMessage {
                        Section {
                            Label(errorMessage, systemImage: "exclamationmark.triangle.fill")
                                .font(.footnote)
                                .foregroundStyle(.red)
                        }
                    }

                    Section {
                        if let parent = response?.parent, !parent.isEmpty {
                            Button {
                                Task { await load(path: parent) }
                            } label: {
                                Label("返回上级目录", systemImage: "arrow.up.left.square")
                                    .foregroundStyle(.primary)
                            }
                        }

                        if isLoading && response == nil {
                            HStack {
                                Spacer()
                                ProgressView("读取 \(nodeName) 目录…")
                                Spacer()
                            }
                        } else if let dirs = response?.dirs, dirs.isEmpty {
                            Text("此目录下没有子文件夹")
                                .font(.subheadline)
                                .foregroundStyle(.secondary)
                        } else if let dirs = response?.dirs {
                            ForEach(dirs) { dir in
                                Button {
                                    Task { await load(path: dir.path) }
                                } label: {
                                    HStack {
                                        Image(systemName: "folder.fill")
                                            .foregroundStyle(.blue)
                                        Text(dir.name)
                                            .foregroundStyle(.primary)
                                            .lineLimit(1)
                                        Spacer()
                                        Image(systemName: "chevron.right")
                                            .font(.caption)
                                            .foregroundStyle(.tertiary)
                                    }
                                }
                            }
                        }
                    } header: {
                        Text(currentPath.isEmpty ? nodeName : currentPath)
                            .font(.caption.monospaced())
                            .textCase(nil)
                    } footer: {
                        if response?.truncated == true {
                            Text("目录项较多，仅展示前一部分子文件夹；也可以直接在上方输入框粘贴完整路径。")
                        }
                    }
                }
                .listStyle(.insetGrouped)
            }
            .navigationTitle("选择目录 · \(nodeName)")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("取消") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("选定此目录") {
                        let chosen = currentPath.isEmpty ? manualPath : currentPath
                        onSelect(chosen)
                        dismiss()
                    }
                    .fontWeight(.semibold)
                    .disabled(currentPath.isEmpty && manualPath.trimmingCharacters(in: .whitespaces).isEmpty)
                }
                ToolbarItem(placement: .bottomBar) {
                    Button("使用默认对话目录（不绑定项目）") {
                        onSelect("")
                        dismiss()
                    }
                    .font(.footnote)
                }
            }
            .task {
                await load(path: initialPath)
            }
        }
    }

    private func load(path: String) async {
        isLoading = true
        errorMessage = nil
        defer { isLoading = false }
        do {
            let res = try await store.browseDirs(node: nodeId, path: path)
            response = res
            currentPath = res.path
            manualPath = res.path
        } catch {
            errorMessage = error.localizedDescription
        }
    }
}
