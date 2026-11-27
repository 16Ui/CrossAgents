import Foundation
import CrossAgentsLinkFFI

/// Preview decoded locally from a `ca1-...` pairing code (no network access).
struct InvitePreview: Sendable, Equatable {
    let host: String
    let expires: Int64
    let hasRelay: Bool
    let customRelay: Bool
    let lanAddrs: Int

    var expiresDate: Date {
        Date(timeIntervalSince1970: TimeInterval(expires))
    }

    var isExpired: Bool {
        expiresDate <= Date()
    }
}

enum LinkError: LocalizedError, Sendable {
    case message(String)

    var errorDescription: String? {
        switch self {
        case .message(let text): return text
        }
    }
}

enum LinkEvent: Sendable, Equatable {
    case listening(port: UInt16)
    case connected(direct: Bool)
    case path(direct: Bool)
    case unreachable(error: String)
    case unauthorized(error: String)
}

private struct DecodeInviteResponse: Decodable {
    let ok: Bool
    let error: String?
    let host: String?
    let expires: Int64?
    let has_relay: Bool?
    let custom_relay: Bool?
    let lan_addrs: Int?
}

private struct RedeemResponse: Decodable {
    struct DeviceSummary: Decodable {
        let id: String
        let name: String
        let level: String?
    }
    struct PairedBody: Decodable {
        let device: DeviceSummary
        let token: String
        let host: String
        let endpoint: String
        let relay: String?
        let custom_relay: Bool?
        let addrs: [String]?
    }
    let ok: Bool
    let error: String?
    let paired: PairedBody?
}

private struct TunnelStartConfig: Encodable {
    let endpoint: String
    let relay: String?
    let custom_relay: Bool
    let addrs: [String]
    let token: String
    let proxy: String?
}

private struct RawLinkEvent: Decodable {
    let event: String
    let port: UInt16?
    let direct: Bool?
    let error: String?

    func toLinkEvent() -> LinkEvent? {
        switch event {
        case "listening":
            guard let port else { return nil }
            return .listening(port: port)
        case "connected":
            return .connected(direct: direct ?? false)
        case "path":
            return .path(direct: direct ?? false)
        case "unreachable":
            return .unreachable(error: error ?? "无法连接到中心")
        case "unauthorized":
            return .unauthorized(error: error ?? "设备凭据已失效或被吊销")
        default:
            return nil
        }
    }
}

enum LinkBridge {
    static func decodeInvite(_ code: String) -> Result<InvitePreview, LinkError> {
        let trimmed = code.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            return .failure(.message("请输入或扫描配对码"))
        }
        let rawJson = trimmed.withCString { cStr -> String in
            guard let ptr = ca_link_decode_invite(cStr) else {
                return "{\"ok\":false,\"error\":\"FFI 调用失败\"}"
            }
            defer { ca_link_string_free(ptr) }
            return String(cString: ptr)
        }
        guard let data = rawJson.data(using: .utf8),
              let decoded = try? JSONDecoder().decode(DecodeInviteResponse.self, from: data)
        else {
            return .failure(.message("配对码解析失败"))
        }
        if decoded.ok, let expires = decoded.expires {
            return .success(InvitePreview(
                host: (decoded.host?.isEmpty == false) ? decoded.host! : "CrossAgents 中心",
                expires: expires,
                hasRelay: decoded.has_relay ?? false,
                customRelay: decoded.custom_relay ?? false,
                lanAddrs: decoded.lan_addrs ?? 0
            ))
        }
        return .failure(.message(decoded.error ?? "无效的配对码"))
    }

    static func redeem(
        code: String,
        deviceName: String,
        proxy: String? = nil
    ) async throws -> PairingRecord {
        let trimmedCode = code.trimmingCharacters(in: .whitespacesAndNewlines)
        let cleanName = deviceName.trimmingCharacters(in: .whitespacesAndNewlines)
        let finalName = cleanName.isEmpty ? "iPhone" : cleanName

        return try await Task.detached(priority: .userInitiated) {
            let rawJson: String = trimmedCode.withCString { cCode in
                finalName.withCString { cName in
                    "ios".withCString { cOs in
                        if let proxy, !proxy.isEmpty {
                            return proxy.withCString { cProxy in
                                guard let ptr = ca_link_redeem(cCode, cName, cOs, cProxy) else {
                                    return "{\"ok\":false,\"error\":\"配对调用失败\"}"
                                }
                                defer { ca_link_string_free(ptr) }
                                return String(cString: ptr)
                            }
                        } else {
                            guard let ptr = ca_link_redeem(cCode, cName, cOs, nil) else {
                                return "{\"ok\":false,\"error\":\"配对调用失败\"}"
                            }
                            defer { ca_link_string_free(ptr) }
                            return String(cString: ptr)
                        }
                    }
                }
            }

            guard let data = rawJson.data(using: .utf8),
                  let decoded = try? JSONDecoder().decode(RedeemResponse.self, from: data)
            else {
                throw LinkError.message("中心返回了无法识别的配对响应")
            }
            guard decoded.ok, let paired = decoded.paired else {
                throw LinkError.message(decoded.error ?? "配对失败，请重试")
            }

            return PairingRecord(
                endpoint: paired.endpoint,
                relay: paired.relay,
                customRelay: paired.custom_relay ?? false,
                addrs: paired.addrs ?? [],
                host: paired.host.isEmpty ? "CrossAgents 中心" : paired.host,
                deviceId: paired.device.id,
                deviceName: paired.device.name,
                token: paired.token,
                level: paired.device.level,
                pairedAt: Date()
            )
        }.value
    }
}

/// Manages one in-process Rust iroh loopback tunnel (`127.0.0.1:<port>`) safely across threads.
final class TunnelRunner: @unchecked Sendable {
    private final class EventBox {
        let continuation: AsyncStream<LinkEvent>.Continuation
        init(continuation: AsyncStream<LinkEvent>.Continuation) {
            self.continuation = continuation
        }
    }

    private let lock = NSLock()
    private var handle: OpaquePointer?
    private var retainedBox: UnsafeMutableRawPointer?

    deinit {
        stop()
    }

    func start(record: PairingRecord, proxy: String? = nil) -> AsyncStream<LinkEvent> {
        stop()

        let cfg = TunnelStartConfig(
            endpoint: record.endpoint,
            relay: record.relay,
            custom_relay: record.customRelay,
            addrs: record.addrs,
            token: record.token,
            proxy: proxy
        )
        let configJson = (try? JSONEncoder().encode(cfg)).flatMap { String(data: $0, encoding: .utf8) } ?? "{}"

        return AsyncStream<LinkEvent>(bufferingPolicy: .bufferingNewest(64)) { continuation in
            let box = EventBox(continuation: continuation)
            let rawBox = Unmanaged.passRetained(box).toOpaque()

            let callback: CaEventCallback = { eventPtr, userData in
                guard let eventPtr, let userData else { return }
                let str = String(cString: eventPtr)
                guard let data = str.data(using: .utf8),
                      let raw = try? JSONDecoder().decode(RawLinkEvent.self, from: data),
                      let event = raw.toLinkEvent()
                else { return }
                let box = Unmanaged<EventBox>.fromOpaque(userData).takeUnretainedValue()
                box.continuation.yield(event)
            }

            let startedHandle: OpaquePointer? = configJson.withCString { cCfg in
                ca_link_start(cCfg, callback, rawBox)
            }

            lock.lock()
            if let startedHandle {
                self.handle = startedHandle
                self.retainedBox = rawBox
                lock.unlock()
            } else {
                lock.unlock()
                let leaked = Unmanaged<EventBox>.fromOpaque(rawBox).takeRetainedValue()
                leaked.continuation.finish()
            }
        }
    }

    func stop() {
        lock.lock()
        let currentHandle = handle
        let currentBox = retainedBox
        handle = nil
        retainedBox = nil
        lock.unlock()

        // `ca_link_stop` clears the callback sink under a Rust mutex before returning, so no
        // background thread can invoke the callback after this line completes.
        if let currentHandle {
            ca_link_stop(currentHandle)
        }
        if let currentBox {
            let box = Unmanaged<EventBox>.fromOpaque(currentBox).takeRetainedValue()
            box.continuation.finish()
        }
    }
}
