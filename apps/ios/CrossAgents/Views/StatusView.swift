import SwiftUI

struct StatusView: View {
    @Environment(AppStore.self) private var store
    @State private var confirmingForget = false

    var body: some View {
        NavigationStack {
            List {
                connectionSection
                nodesSection
                quotaSection
            }
            .listStyle(.insetGrouped)
            .navigationTitle("概览")
            .refreshable {
                await store.refreshAll()
            }
            .alert("忘记这台中心？", isPresented: $confirmingForget) {
                Button("取消", role: .cancel) {}
                Button("忘记并退出配对", role: .destructive) {
                    store.forget()
                }
            } message: {
                Text("这只会删除本机 Keychain 中的配对凭据，不会影响中心电脑上正在运行的 Agent。你可以随时重新扫码配对。")
            }
        }
    }

    // MARK: - Connection Section

    private var connectionSection: some View {
        Section("当前中心连接") {
            if let pairing = store.pairing {
                HStack(spacing: 12) {
                    Image(systemName: "desktopcomputer")
                        .font(.title2)
                        .foregroundStyle(.indigo)
                        .frame(width: 36)
                    VStack(alignment: .leading, spacing: 3) {
                        Text(store.hubInfo?.machine?.hostname.isEmpty == false ? store.hubInfo!.machine!.hostname : pairing.host)
                            .font(.headline)
                        Text("本机设备：\(pairing.deviceName)\(pairing.level.map { " · \($0)" } ?? "")")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        if let version = store.hubInfo?.version {
                            Text("中心版本 v\(version)")
                                .font(.caption2.monospaced())
                                .foregroundStyle(.tertiary)
                        }
                    }
                    Spacer()
                    tunnelPathBadge
                }
                .padding(.vertical, 2)

                Button {
                    store.reconnect()
                } label: {
                    Label("重新建立隧道连接", systemImage: "arrow.triangle.2.circlepath")
                }

                Button(role: .destructive) {
                    confirmingForget = true
                } label: {
                    Label("忘记此中心并重新配对", systemImage: "link.badge.plus")
                }
            }
        }
    }

    @ViewBuilder
    private var tunnelPathBadge: some View {
        switch store.phase {
        case .connected(let direct):
            Text(direct ? "直连" : "经中继")
                .font(.caption.weight(.semibold))
                .foregroundStyle(direct ? .green : .blue)
                .padding(.horizontal, 9)
                .padding(.vertical, 4)
                .background((direct ? Color.green : Color.blue).opacity(0.14), in: Capsule())
        case .connecting:
            Text("连接中")
                .font(.caption.weight(.medium))
                .foregroundStyle(.indigo)
        case .reconnecting:
            Text("重连中")
                .font(.caption.weight(.medium))
                .foregroundStyle(.orange)
        case .revoked:
            Text("已吊销")
                .font(.caption.weight(.semibold))
                .foregroundStyle(.red)
        case .unpaired:
            EmptyView()
        }
    }

    // MARK: - Online Nodes Section

    private var nodesSection: some View {
        Section {
            if store.nodes.isEmpty {
                Text("暂无节点信息")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            } else {
                ForEach(store.nodes) { node in
                    VStack(alignment: .leading, spacing: 8) {
                        HStack(spacing: 10) {
                            Image(systemName: node.sfSymbol)
                                .font(.title3)
                                .foregroundStyle(node.online ? .primary : .secondary)
                                .frame(width: 26)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(node.displayName)
                                    .font(.subheadline.weight(.semibold))
                                Text("\(node.os)/\(node.arch)\(node.version.isEmpty ? "" : " · v\(node.version)")")
                                    .font(.caption2.monospaced())
                                    .foregroundStyle(.secondary)
                            }
                            Spacer()
                            HStack(spacing: 5) {
                                Circle()
                                    .fill(node.online ? Color.green : Color.gray)
                                    .frame(width: 8, height: 8)
                                Text(node.online ? "在线" : "离线")
                                    .font(.caption)
                                    .foregroundStyle(node.online ? .green : .secondary)
                            }
                        }

                        let installed = node.plugins.filter(\.installed)
                        if !installed.isEmpty {
                            FlowLayout(spacing: 6) {
                                ForEach(installed) { plugin in
                                    Text(plugin.version.map { "\(plugin.name) \($0)" } ?? plugin.name)
                                        .font(.caption2.monospaced())
                                        .padding(.horizontal, 8)
                                        .padding(.vertical, 3)
                                        .background(Color(.tertiarySystemFill), in: Capsule())
                                }
                            }
                        }
                    }
                    .padding(.vertical, 4)
                }
            }
        } header: {
            Text("执行机器与节点 (\(store.nodes.filter(\.online).count)/\(store.nodes.count) 在线)")
        }
    }

    // MARK: - Subscription Quota Section

    private var quotaSection: some View {
        Section("订阅配额与额度") {
            let accounts = store.quota?.accounts ?? []
            if accounts.isEmpty {
                Text("暂无已接入的订阅账号配额数据")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            } else {
                ForEach(accounts) { item in
                    AccountQuotaRow(item: item)
                }
            }
        }
    }
}

private struct AccountQuotaRow: View {
    let item: AccountQuota

    private var allWindows: [(groupTitle: String?, window: RateWindow)] {
        if !item.status.limits.isEmpty {
            return item.status.limits.flatMap { group in
                let label = group.name ?? group.model
                return group.windows.map { (label, $0) }
            }
        }
        return item.status.rateLimits.map { (nil, $0) }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 6) {
                        Text(item.account.label.isEmpty ? item.account.id : item.account.label)
                            .font(.subheadline.weight(.semibold))
                        Text(item.account.provider)
                            .font(.caption2.monospaced())
                            .padding(.horizontal, 6)
                            .padding(.vertical, 2)
                            .background(Color(.tertiarySystemFill), in: Capsule())
                        if let plan = item.status.plan, !plan.isEmpty {
                            Text(plan)
                                .font(.caption2.weight(.medium))
                                .padding(.horizontal, 6)
                                .padding(.vertical, 2)
                                .background(Color.indigo.opacity(0.14), in: Capsule())
                                .foregroundStyle(.indigo)
                        }
                    }
                    if let email = item.status.email, !email.isEmpty {
                        Text(email)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
                Spacer()
                if item.status.limitReached {
                    Text("已达上限")
                        .font(.caption2.weight(.semibold))
                        .foregroundStyle(.red)
                        .padding(.horizontal, 7)
                        .padding(.vertical, 3)
                        .background(.red.opacity(0.14), in: Capsule())
                } else if !item.status.loggedIn {
                    Text("未登录")
                        .font(.caption2)
                        .foregroundStyle(.orange)
                }
            }

            if let atRisk = item.atRisk, !atRisk.isEmpty {
                Label(atRisk, systemImage: "exclamationmark.shield.fill")
                    .font(.caption)
                    .foregroundStyle(.red)
            }

            if let egressBlock = item.egressBlock, !egressBlock.isEmpty {
                Label(egressBlock, systemImage: "globe.badge.chevron.backward")
                    .font(.caption)
                    .foregroundStyle(.orange)
            }

            if allWindows.isEmpty {
                if let credits = item.status.credits, credits.hasCredits {
                    Text(credits.unlimited ? "无限额度" : "余额：\(credits.balance ?? "-")")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                } else if let note = item.status.note, !note.isEmpty {
                    Text(note)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            } else {
                ForEach(Array(allWindows.enumerated()), id: \.offset) { _, entry in
                    RateWindowBar(groupTitle: entry.groupTitle, window: entry.window)
                }
            }
        }
        .padding(.vertical, 4)
    }
}

private struct RateWindowBar: View {
    let groupTitle: String?
    let window: RateWindow

    private var tint: Color {
        if window.usedPercent >= 90 { return .red }
        if window.usedPercent >= 70 { return .orange }
        return .green
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                Text([groupTitle, window.name].compactMap { $0 }.filter { !$0.isEmpty }.joined(separator: " · "))
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Spacer()
                if let rem = window.remaining, let tot = window.total, tot > 0 {
                    Text("余 \(Int(rem)) / \(Int(tot))")
                        .font(.caption.monospacedDigit().weight(.medium))
                } else {
                    Text("剩余 \(window.remainingPercent)%")
                        .font(.caption.monospacedDigit().weight(.medium))
                        .foregroundStyle(tint)
                }
            }

            ProgressView(value: window.clampedUsedFraction)
                .tint(tint)

            if let resetText = DateHelpers.countdown(until: window.resetsAt) {
                Text(resetText)
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
            }
        }
    }
}
