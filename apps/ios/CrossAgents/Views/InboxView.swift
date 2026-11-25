import SwiftUI
import UIKit

struct InboxView: View {
    @Environment(AppStore.self) private var store
    @State private var filter = 0 // 0 = 待处理, 1 = 已处理

    private var displayedItems: [HumanItem] {
        filter == 0 ? store.openQuestions : store.answeredQuestions
    }

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                Picker("筛选", selection: $filter) {
                    Text("待处理 (\(store.openQuestions.count))").tag(0)
                    Text("已处理 (\(store.answeredQuestions.count))").tag(1)
                }
                .pickerStyle(.segmented)
                .padding(.horizontal, 16)
                .padding(.vertical, 10)
                .background(Color(.systemGroupedBackground))

                if displayedItems.isEmpty {
                    ContentUnavailableView(
                        filter == 0 ? "暂无待办审批或提问" : "暂无历史记录",
                        systemImage: filter == 0 ? "checkmark.circle" : "tray",
                        description: Text(filter == 0 ? "当任意一台电脑上的 Agent 调用提问或等待你审批时，会实时推送到这里。" : "已回复或忽略的提问会保留在这里方便回顾。")
                    )
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .background(Color(.systemGroupedBackground))
                } else {
                    ScrollView {
                        LazyVStack(spacing: 12) {
                            ForEach(displayedItems) { item in
                                QuestionCardView(item: item)
                            }
                        }
                        .padding(.horizontal, 16)
                        .padding(.vertical, 8)
                    }
                    .background(Color(.systemGroupedBackground))
                    .refreshable {
                        await store.refreshAll()
                    }
                }
            }
            .navigationTitle("待办")
        }
    }
}

private struct QuestionCardView: View {
    @Environment(AppStore.self) private var store
    let item: HumanItem

    @State private var customReply = ""
    @State private var showDetail = false
    @State private var isSubmitting = false
    @State private var errorMessage: String?

    private var askingAgent: Agent? {
        store.agent(by: item.from)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                if let agent = askingAgent {
                    AgentAvatarView(agent: agent, size: 30)
                } else {
                    Image(systemName: "person.crop.circle.badge.questionmark")
                        .font(.title3)
                        .foregroundStyle(.indigo)
                }
                VStack(alignment: .leading, spacing: 1) {
                    Text(askingAgent?.name ?? item.from)
                        .font(.subheadline.weight(.semibold))
                    Text("\(store.nodeDisplayName(for: askingAgent?.node)) · \(DateHelpers.relative(item.createdAt))")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                Text(item.actionBadge)
                    .font(.caption2.weight(.semibold))
                    .padding(.horizontal, 8)
                    .padding(.vertical, 3)
                    .background(badgeColor.opacity(0.15), in: Capsule())
                    .foregroundStyle(badgeColor)
            }

            MarkdownContentView(text: item.text)

            if let detail = item.detail, !detail.isEmpty {
                DisclosureGroup("查看详情", isExpanded: $showDetail) {
                    Text(detail)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.top, 4)
                }
                .font(.caption.weight(.medium))
            }

            if item.isOpen {
                if !item.options.isEmpty {
                    FlowLayout(spacing: 8) {
                        ForEach(item.options, id: \.self) { opt in
                            Button(opt) {
                                Task { await submit(opt) }
                            }
                            .font(.subheadline.weight(.medium))
                            .buttonStyle(.borderedProminent)
                            .controlSize(.small)
                            .disabled(isSubmitting)
                        }
                    }
                }

                HStack(spacing: 8) {
                    TextField("输入自定义回复…", text: $customReply)
                        .font(.subheadline)
                        .padding(.horizontal, 10)
                        .padding(.vertical, 8)
                        .background(Color(.secondarySystemBackground), in: RoundedRectangle(cornerRadius: 10))
                        .disabled(isSubmitting)

                    Button {
                        let trimmed = customReply.trimmingCharacters(in: .whitespacesAndNewlines)
                        guard !trimmed.isEmpty else { return }
                        Task { await submit(trimmed) }
                    } label: {
                        if isSubmitting {
                            ProgressView().controlSize(.small)
                        } else {
                            Text("发送").fontWeight(.semibold)
                        }
                    }
                    .buttonStyle(.borderedProminent)
                    .controlSize(.regular)
                    .disabled(isSubmitting || customReply.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }

                HStack {
                    if let def = item.defaultAnswer, !def.isEmpty {
                        Text("超时默认：\(def)")
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                    Button("忽略", role: .destructive) {
                        Task { await dismissItem() }
                    }
                    .font(.caption)
                    .disabled(isSubmitting)
                }
            } else {
                HStack(spacing: 6) {
                    Image(systemName: item.status == "dismissed" ? "minus.circle" : "checkmark.circle.fill")
                        .foregroundStyle(item.status == "dismissed" ? .secondary : .green)
                    if let answer = item.answer, !answer.isEmpty {
                        Text("已回复：\(answer)")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    } else {
                        Text(item.status == "dismissed" ? "已忽略" : "已处理（\(item.status)）")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                    if let at = item.answeredAt {
                        Text(DateHelpers.relative(at))
                            .font(.caption2)
                            .foregroundStyle(.tertiary)
                    }
                }
                .padding(8)
                .background(Color(.secondarySystemBackground), in: RoundedRectangle(cornerRadius: 8))
            }

            if let errorMessage {
                Text(errorMessage)
                    .font(.caption)
                    .foregroundStyle(.red)
            }
        }
        .padding(14)
        .background(Color(.systemBackground), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
    }

    private var badgeColor: Color {
        switch item.action {
        case "approval": return .red
        case "budget": return .orange
        case "work": return .indigo
        default: return .blue
        }
    }

    private func submit(_ answer: String) async {
        isSubmitting = true
        errorMessage = nil
        defer { isSubmitting = false }
        do {
            try await store.answerQuestion(id: item.id, answer: answer)
            customReply = ""
            let generator = UINotificationFeedbackGenerator()
            generator.notificationOccurred(.success)
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func dismissItem() async {
        isSubmitting = true
        errorMessage = nil
        defer { isSubmitting = false }
        do {
            try await store.dismissQuestion(id: item.id)
        } catch {
            errorMessage = error.localizedDescription
        }
    }
}
