import SwiftUI

struct MainTabView: View {
    @Environment(AppStore.self) private var store
    @State private var selectedTab = 0

    var body: some View {
        Group {
            if store.pairing == nil {
                PairingView()
            } else {
                VStack(spacing: 0) {
                    ConnectionStatusBanner()
                    TabView(selection: $selectedTab) {
                        ChatsView()
                            .tabItem {
                                Label("对话", systemImage: "bubble.left.and.bubble.right.fill")
                            }
                            .tag(0)

                        InboxView()
                            .tabItem {
                                Label("待办", systemImage: "bell.badge.fill")
                            }
                            .badge(store.openQuestions.count)
                            .tag(1)

                        StatusView()
                            .tabItem {
                                Label("概览", systemImage: "gauge.with.dots.needle.67percent")
                            }
                            .tag(2)
                    }
                }
            }
        }
        .animation(.easeInOut(duration: 0.2), value: store.pairing == nil)
    }
}

private struct ConnectionStatusBanner: View {
    @Environment(AppStore.self) private var store

    var body: some View {
        switch store.phase {
        case .connected, .unpaired:
            EmptyView()
        case .connecting:
            banner(
                color: .indigo,
                icon: "antenna.radiowaves.left.and.right",
                text: "正在连接中心 \(store.pairing?.host ?? "")…",
                actionTitle: nil,
                action: nil
            )
        case .reconnecting(let reason):
            banner(
                color: .orange,
                icon: "wifi.exclamationmark",
                text: reason.isEmpty ? "正在重连中心…" : reason,
                actionTitle: "重试",
                action: { store.reconnect() }
            )
        case .revoked(let reason):
            banner(
                color: .red,
                icon: "lock.slash.fill",
                text: reason,
                actionTitle: "重新配对",
                action: { store.forget() }
            )
        }
    }

    private func banner(
        color: Color,
        icon: String,
        text: String,
        actionTitle: String?,
        action: (() -> Void)?
    ) -> some View {
        HStack(spacing: 8) {
            Image(systemName: icon)
                .font(.caption.weight(.semibold))
            Text(text)
                .font(.caption.weight(.medium))
                .lineLimit(1)
            Spacer(minLength: 4)
            if let actionTitle, let action {
                Button(actionTitle, action: action)
                    .font(.caption.weight(.semibold))
                    .padding(.horizontal, 10)
                    .padding(.vertical, 4)
                    .background(.white.opacity(0.22), in: Capsule())
            }
        }
        .foregroundStyle(.white)
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .background(color.gradient)
    }
}
