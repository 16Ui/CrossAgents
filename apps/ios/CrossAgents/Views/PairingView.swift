import SwiftUI
import UIKit

struct PairingView: View {
    @Environment(AppStore.self) private var store

    @State private var code = ""
    @State private var deviceName = UIDevice.current.name
    @State private var showingScanner = false
    @State private var isPairing = false
    @State private var errorMessage: String?

    private var decodedPreview: Result<InvitePreview, LinkError>? {
        let trimmed = code.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        return LinkBridge.decodeInvite(trimmed)
    }

    private var canSubmit: Bool {
        guard !isPairing, case .success(let preview) = decodedPreview else { return false }
        return !preview.isExpired
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 24) {
                    header

                    VStack(spacing: 12) {
                        Button {
                            showingScanner = true
                        } label: {
                            Label("扫描电脑端二维码配对", systemImage: "qrcode.viewfinder")
                                .font(.headline)
                                .frame(maxWidth: .infinity)
                                .padding(.vertical, 14)
                        }
                        .buttonStyle(.borderedProminent)
                        .disabled(isPairing)

                        Text("在中心电脑打开 CrossAgents → 设置 → 连接 → 让别的电脑连进来 → 生成配对码")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .multilineTextAlignment(.center)
                    }

                    VStack(alignment: .leading, spacing: 14) {
                        HStack {
                            Text("或粘贴配对码")
                                .font(.subheadline.weight(.semibold))
                            Spacer()
                            Button {
                                if let pasted = UIPasteboard.general.string {
                                    code = pasted.trimmingCharacters(in: .whitespacesAndNewlines)
                                    errorMessage = nil
                                }
                            } label: {
                                Label("粘贴", systemImage: "doc.on.clipboard")
                                    .font(.caption.weight(.medium))
                            }
                            .buttonStyle(.bordered)
                            .controlSize(.small)
                            .disabled(isPairing)
                        }

                        TextField("ca1-…", text: $code, axis: .vertical)
                            .font(.system(.footnote, design: .monospaced))
                            .lineLimit(2...5)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                            .padding(10)
                            .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 10))
                            .disabled(isPairing)

                        previewStatusView

                        Divider()

                        VStack(alignment: .leading, spacing: 6) {
                            Text("这台手机的设备名")
                                .font(.caption.weight(.medium))
                                .foregroundStyle(.secondary)
                            TextField("例如：Link 的 iPhone", text: $deviceName)
                                .padding(10)
                                .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 10))
                                .disabled(isPairing)
                        }

                        if let errorMessage {
                            HStack(alignment: .top, spacing: 8) {
                                Image(systemName: "exclamationmark.triangle.fill")
                                    .foregroundStyle(.red)
                                Text(errorMessage)
                                    .font(.footnote)
                                    .foregroundStyle(.red)
                            }
                            .padding(10)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .background(.red.opacity(0.1), in: RoundedRectangle(cornerRadius: 10))
                        }

                        Button {
                            Task { await performPair() }
                        } label: {
                            HStack(spacing: 8) {
                                if isPairing {
                                    ProgressView()
                                        .tint(.white)
                                }
                                Text(isPairing ? "正在建立打洞连接…" : "配对并连接")
                                    .font(.headline)
                            }
                            .frame(maxWidth: .infinity)
                            .padding(.vertical, 12)
                        }
                        .buttonStyle(.borderedProminent)
                        .disabled(!canSubmit)

                        if isPairing {
                            Text("首次打洞连接可能需要数秒，跨网环境会自动走中继。")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                                .frame(maxWidth: .infinity, alignment: .center)
                        }
                    }
                    .padding(16)
                    .background(Color(.systemBackground), in: RoundedRectangle(cornerRadius: 16))
                    .shadow(color: .black.opacity(0.05), radius: 8, y: 2)
                }
                .padding(20)
            }
            .background(Color(.systemGroupedBackground))
            .navigationBarHidden(true)
            .fullScreenCover(isPresented: $showingScanner) {
                QRScannerView { scanned in
                    code = scanned
                    errorMessage = nil
                    Task { await performPair() }
                }
            }
        }
    }

    private var header: some View {
        VStack(spacing: 12) {
            ZStack {
                RoundedRectangle(cornerRadius: 22, style: .continuous)
                    .fill(.indigo.gradient)
                    .frame(width: 76, height: 76)
                    .shadow(color: .indigo.opacity(0.35), radius: 12, y: 6)
                Image(systemName: "network")
                    .font(.system(size: 34, weight: .semibold))
                    .foregroundStyle(.white)
            }
            .padding(.top, 16)

            Text("CrossAgents")
                .font(.largeTitle.bold())
            Text("随时随地查看并指挥你的多机 Agent 团队")
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
        }
    }

    @ViewBuilder
    private var previewStatusView: some View {
        if let decodedPreview {
            switch decodedPreview {
            case .success(let preview):
                let expired = preview.isExpired
                HStack(alignment: .top, spacing: 8) {
                    Image(systemName: expired ? "clock.badge.exclamationmark" : "checkmark.seal.fill")
                        .foregroundStyle(expired ? .red : .green)
                    VStack(alignment: .leading, spacing: 2) {
                        Text("目标中心：\(preview.host)")
                            .font(.footnote.weight(.semibold))
                        if expired {
                            Text("该配对码已过期，请在中心电脑重新生成。")
                                .font(.caption)
                                .foregroundStyle(.red)
                        } else {
                            let routeHint = preview.hasRelay ? "支持外网中继与局域网直连" : "仅支持同一局域网直连"
                            Text("\(routeHint)\(preview.lanAddrs > 0 ? " · \(preview.lanAddrs) 个直连地址" : "")")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                }
                .padding(10)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background((expired ? Color.red : Color.green).opacity(0.1), in: RoundedRectangle(cornerRadius: 10))
            case .failure(let err):
                HStack(spacing: 8) {
                    Image(systemName: "xmark.circle.fill")
                        .foregroundStyle(.orange)
                    Text(err.localizedDescription)
                        .font(.caption)
                        .foregroundStyle(.orange)
                }
            }
        }
    }

    private func performPair() async {
        guard canSubmit else { return }
        isPairing = true
        errorMessage = nil
        defer { isPairing = false }
        do {
            try await store.pair(code: code, deviceName: deviceName)
        } catch {
            errorMessage = error.localizedDescription
        }
    }
}
