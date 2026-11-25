import AVFoundation
import SwiftUI
import UIKit

struct QRScannerView: View {
    let onScanned: (String) -> Void
    @Environment(\.dismiss) private var dismiss

    @State private var cameraStatus: AVAuthorizationStatus = AVCaptureDevice.authorizationStatus(for: .video)
    @State private var torchOn = false
    @State private var statusHint = "将电脑端 CrossAgents 的配对二维码置于框内"
    @State private var hasCamera = AVCaptureDevice.default(for: .video) != nil

    var body: some View {
        NavigationStack {
            ZStack {
                Color.black.ignoresSafeArea()

                if !hasCamera {
                    unavailableView(
                        title: "当前设备无可用相机",
                        message: "如果你正在使用 iOS 模拟器，请返回上一页直接粘贴电脑端复制的 ca1-... 配对码。"
                    )
                } else if cameraStatus == .authorized {
                    QRCameraPreview(torchOn: $torchOn) { code in
                        let trimmed = code.trimmingCharacters(in: .whitespacesAndNewlines)
                        if trimmed.hasPrefix("ca1-") {
                            let generator = UINotificationFeedbackGenerator()
                            generator.notificationOccurred(.success)
                            onScanned(trimmed)
                            dismiss()
                        } else {
                            statusHint = "这不是 CrossAgents 配对二维码（应以 ca1- 开头）"
                        }
                    }
                    .ignoresSafeArea()

                    scannerOverlay
                } else if cameraStatus == .notDetermined {
                    ProgressView("请求相机权限…")
                        .tint(.white)
                        .foregroundStyle(.white)
                } else {
                    unavailableView(
                        title: "需要相机权限",
                        message: "请在系统设置中允许 CrossAgents 访问相机以扫描配对二维码。",
                        showSettings: true
                    )
                }
            }
            .navigationTitle("扫码配对")
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(.black.opacity(0.6), for: .navigationBar)
            .toolbarColorScheme(.dark, for: .navigationBar)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("取消") { dismiss() }
                }
                if hasCamera && cameraStatus == .authorized {
                    ToolbarItem(placement: .primaryAction) {
                        Button {
                            torchOn.toggle()
                        } label: {
                            Image(systemName: torchOn ? "flashlight.on.fill" : "flashlight.off.fill")
                        }
                    }
                }
            }
            .task {
                if hasCamera && cameraStatus == .notDetermined {
                    let granted = await AVCaptureDevice.requestAccess(for: .video)
                    cameraStatus = granted ? .authorized : .denied
                }
            }
        }
    }

    private var scannerOverlay: some View {
        VStack(spacing: 24) {
            Spacer()
            RoundedRectangle(cornerRadius: 20, style: .continuous)
                .strokeBorder(.white.opacity(0.9), lineWidth: 2.5)
                .frame(width: 250, height: 250)
                .background(.clear)
                .shadow(color: .black.opacity(0.5), radius: 12)

            Text(statusHint)
                .font(.subheadline.weight(.medium))
                .foregroundStyle(.white)
                .multilineTextAlignment(.center)
                .padding(.horizontal, 20)
                .padding(.vertical, 10)
                .background(.black.opacity(0.62), in: Capsule())
                .padding(.horizontal, 24)
            Spacer()
        }
    }

    private func unavailableView(title: String, message: String, showSettings: Bool = false) -> some View {
        VStack(spacing: 14) {
            Image(systemName: "qrcode.viewfinder")
                .font(.system(size: 48))
                .foregroundStyle(.secondary)
            Text(title)
                .font(.headline)
                .foregroundStyle(.white)
            Text(message)
                .font(.subheadline)
                .foregroundStyle(.white.opacity(0.75))
                .multilineTextAlignment(.center)
                .padding(.horizontal, 32)
            if showSettings, let url = URL(string: UIApplication.openSettingsURLString) {
                Button("前往系统设置") {
                    UIApplication.shared.open(url)
                }
                .buttonStyle(.borderedProminent)
                .padding(.top, 6)
            }
        }
    }
}

private struct QRCameraPreview: UIViewControllerRepresentable {
    @Binding var torchOn: Bool
    let onDetect: (String) -> Void

    func makeUIViewController(context: Context) -> QRCameraViewController {
        let vc = QRCameraViewController()
        vc.onDetect = onDetect
        return vc
    }

    func updateUIViewController(_ uiViewController: QRCameraViewController, context: Context) {
        uiViewController.setTorch(torchOn)
    }
}

private final class QRCameraViewController: UIViewController, AVCaptureMetadataOutputObjectsDelegate {
    var onDetect: ((String) -> Void)?

    private let session = AVCaptureSession()
    private var previewLayer: AVCaptureVideoPreviewLayer?
    private var hasDelivered = false

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black

        guard let device = AVCaptureDevice.default(for: .video),
              let input = try? AVCaptureDeviceInput(device: device),
              session.canAddInput(input)
        else { return }

        session.addInput(input)
        let output = AVCaptureMetadataOutput()
        guard session.canAddOutput(output) else { return }
        session.addOutput(output)
        output.setMetadataObjectsDelegate(self, queue: .main)
        output.metadataObjectTypes = [.qr]

        let preview = AVCaptureVideoPreviewLayer(session: session)
        preview.videoGravity = .resizeAspectFill
        preview.frame = view.bounds
        view.layer.addSublayer(preview)
        previewLayer = preview

        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            self?.session.startRunning()
        }
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        previewLayer?.frame = view.bounds
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        setTorch(false)
        if session.isRunning {
            DispatchQueue.global(qos: .userInitiated).async { [weak self] in
                self?.session.stopRunning()
            }
        }
    }

    func setTorch(_ on: Bool) {
        guard let device = AVCaptureDevice.default(for: .video), device.hasTorch else { return }
        try? device.lockForConfiguration()
        device.torchMode = on ? .on : .off
        device.unlockForConfiguration()
    }

    func metadataOutput(
        _ output: AVCaptureMetadataOutput,
        didOutput metadataObjects: [AVMetadataObject],
        from connection: AVCaptureConnection
    ) {
        guard !hasDelivered else { return }
        for obj in metadataObjects {
            guard let readable = obj as? AVMetadataMachineReadableCodeObject,
                  let text = readable.stringValue
            else { continue }
            if text.trimmingCharacters(in: .whitespacesAndNewlines).hasPrefix("ca1-") {
                hasDelivered = true
                session.stopRunning()
            }
            onDetect?(text)
            break
        }
    }
}
