import SwiftUI
import UIKit

// MARK: - Avatars & Status

struct AgentAvatarView: View {
    @Environment(AppStore.self) private var store
    let agent: Agent
    var size: CGFloat = 42

    var body: some View {
        Group {
            if let imgId = agent.profile.avatarImage, !imgId.isEmpty {
                if let img = store.avatarImages[imgId] {
                    Image(uiImage: img)
                        .resizable()
                        .scaledToFill()
                } else {
                    fallbackBadge
                        .task(id: imgId) {
                            store.loadAvatarIfNeeded(id: imgId)
                        }
                }
            } else {
                fallbackBadge
            }
        }
        .frame(width: size, height: size)
        .clipShape(Circle())
    }

    private var fallbackBadge: some View {
        ZStack {
            Circle().fill(badgeColor.gradient)
            Text(initials)
                .font(.system(size: size * 0.42, weight: .semibold))
                .foregroundStyle(.white)
        }
    }

    private var initials: String {
        if let emoji = agent.profile.avatar, !emoji.isEmpty {
            return String(emoji.prefix(2))
        }
        let clean = agent.name.trimmingCharacters(in: .whitespacesAndNewlines)
        return String((clean.isEmpty ? "A" : clean).prefix(2)).uppercased()
    }

    private var badgeColor: Color {
        switch (agent.profile.color ?? "").lowercased() {
        case "blue", "sky": return .blue
        case "green", "emerald", "mint": return .green
        case "amber", "orange", "yellow": return .orange
        case "rose", "red", "pink": return .pink
        case "purple", "violet", "indigo": return .indigo
        case "teal", "cyan": return .teal
        default:
            let palette: [Color] = [.indigo, .blue, .teal, .orange, .pink, .purple]
            let hash = abs(agent.name.hashValue)
            return palette[hash % palette.count]
        }
    }
}

struct UserAvatarView: View {
    @Environment(AppStore.self) private var store
    var size: CGFloat = 28

    var body: some View {
        Group {
            if let img = store.userAvatarImage {
                Image(uiImage: img)
                    .resizable()
                    .scaledToFill()
            } else {
                ZStack {
                    Circle().fill(Color.accentColor.gradient)
                    Text(String((store.userProfile?.name.isEmpty == false ? store.userProfile!.name : "我").prefix(1)))
                        .font(.system(size: size * 0.45, weight: .semibold))
                        .foregroundStyle(.white)
                }
            }
        }
        .frame(width: size, height: size)
        .clipShape(Circle())
    }
}

struct StatusDot: View {
    let status: AgentStatus
    @State private var pulsing = false

    var body: some View {
        ZStack {
            if status == .running || status == .starting {
                Circle()
                    .fill(dotColor.opacity(0.35))
                    .frame(width: 14, height: 14)
                    .scaleEffect(pulsing ? 1.35 : 0.75)
                    .opacity(pulsing ? 0.1 : 0.75)
                    .animation(.easeInOut(duration: 1.1).repeatForever(autoreverses: false), value: pulsing)
                    .onAppear { pulsing = true }
            }
            Circle()
                .fill(dotColor)
                .frame(width: 8, height: 8)
        }
        .frame(width: 14, height: 14)
    }

    private var dotColor: Color {
        switch status {
        case .running: return .green
        case .starting: return .orange
        case .idle: return .blue.opacity(0.65)
        case .error: return .red
        case .stopped, .unknown: return .gray.opacity(0.45)
        }
    }
}

// MARK: - Markdown + Fenced Code Renderer

struct MarkdownContentView: View {
    let text: String

    private enum Block: Identifiable {
        case prose(id: Int, content: String)
        case code(id: Int, language: String, content: String)

        var id: Int {
            switch self {
            case .prose(let id, _), .code(let id, _, _): return id
            }
        }
    }

    private var blocks: [Block] {
        var result: [Block] = []
        let lines = text.components(separatedBy: "\n")
        var inCode = false
        var codeLang = ""
        var buffer: [String] = []
        var nextId = 0

        func flushProse() {
            let joined = buffer.joined(separator: "\n").trimmingCharacters(in: .whitespacesAndNewlines)
            if !joined.isEmpty {
                result.append(.prose(id: nextId, content: joined))
                nextId += 1
            }
            buffer.removeAll()
        }

        func flushCode() {
            result.append(.code(id: nextId, language: codeLang, content: buffer.joined(separator: "\n")))
            nextId += 1
            buffer.removeAll()
            codeLang = ""
        }

        for line in lines {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("```") {
                if inCode {
                    flushCode()
                    inCode = false
                } else {
                    flushProse()
                    codeLang = String(trimmed.dropFirst(3)).trimmingCharacters(in: .whitespaces)
                    inCode = true
                }
            } else {
                buffer.append(line)
            }
        }
        if inCode {
            flushCode()
        } else {
            flushProse()
        }
        return result
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            ForEach(blocks) { block in
                switch block {
                case .prose(_, let content):
                    proseText(content)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                case .code(_, let language, let content):
                    CodeBlockCard(language: language, code: content)
                }
            }
        }
    }

    private func proseText(_ raw: String) -> Text {
        if let attributed = try? AttributedString(
            markdown: raw,
            options: .init(interpretedSyntax: .inlineOnlyPreservingWhitespace)
        ) {
            return Text(attributed)
        }
        return Text(raw)
    }
}

private struct CodeBlockCard: View {
    let language: String
    let code: String
    @State private var copied = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text(language.isEmpty ? "code" : language)
                    .font(.caption2.monospaced().weight(.semibold))
                    .foregroundStyle(.secondary)
                Spacer()
                Button {
                    UIPasteboard.general.string = code
                    copied = true
                    Task {
                        try? await Task.sleep(nanoseconds: 1_400_000_000)
                        copied = false
                    }
                } label: {
                    Label(copied ? "已复制" : "复制", systemImage: copied ? "checkmark" : "doc.on.doc")
                        .font(.caption2.weight(.medium))
                }
                .buttonStyle(.plain)
                .foregroundStyle(copied ? .green : .secondary)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .background(Color(.tertiarySystemFill))

            ScrollView(.horizontal, showsIndicators: true) {
                Text(code)
                    .font(.system(.caption, design: .monospaced))
                    .textSelection(.enabled)
                    .padding(10)
            }
        }
        .background(Color(.secondarySystemBackground))
        .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .strokeBorder(Color(.separator).opacity(0.45), lineWidth: 0.8)
        )
    }
}

// MARK: - Wrapping Flow Layout for Pills & Chips

struct FlowLayout: Layout {
    var spacing: CGFloat = 8

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let maxWidth = proposal.width ?? .infinity
        var x: CGFloat = 0
        var y: CGFloat = 0
        var lineHeight: CGFloat = 0
        var totalWidth: CGFloat = 0

        for subview in subviews {
            let size = subview.sizeThatFits(.unspecified)
            if x + size.width > maxWidth && x > 0 {
                x = 0
                y += lineHeight + spacing
                lineHeight = 0
            }
            x += size.width + spacing
            lineHeight = max(lineHeight, size.height)
            totalWidth = max(totalWidth, min(x, maxWidth))
        }
        return CGSize(width: totalWidth, height: y + lineHeight)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var x = bounds.minX
        var y = bounds.minY
        var lineHeight: CGFloat = 0

        for subview in subviews {
            let size = subview.sizeThatFits(.unspecified)
            if x + size.width > bounds.maxX && x > bounds.minX {
                x = bounds.minX
                y += lineHeight + spacing
                lineHeight = 0
            }
            subview.place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(size))
            x += size.width + spacing
            lineHeight = max(lineHeight, size.height)
        }
    }
}
