import Foundation
import UIKit

enum HubClientError: LocalizedError, Sendable {
    case http(status: Int, code: String, message: String)
    case unauthorized(String)
    case invalidURL
    case transport(String)

    var errorDescription: String? {
        switch self {
        case .http(_, _, let msg): return msg
        case .unauthorized(let msg): return msg
        case .invalidURL: return "无效的请求地址"
        case .transport(let msg): return msg
        }
    }
}

/// Pure URLSession client talking to the local iroh loopback port (`http://127.0.0.1:<port>`).
/// Reports `x-crossagents-client-os: ios` so the hub records this device with the iPhone icon.
final class HubClient: Sendable {
    let port: UInt16
    let token: String

    private let session: URLSession
    private let streamSession: URLSession

    init(port: UInt16, token: String) {
        self.port = port
        self.token = token

        let restCfg = URLSessionConfiguration.ephemeral
        restCfg.timeoutIntervalForRequest = 30
        restCfg.timeoutIntervalForResource = 120
        restCfg.waitsForConnectivity = false
        self.session = URLSession(configuration: restCfg)

        let sseCfg = URLSessionConfiguration.ephemeral
        sseCfg.timeoutIntervalForRequest = 60
        sseCfg.timeoutIntervalForResource = 60 * 60 * 24
        sseCfg.waitsForConnectivity = false
        self.streamSession = URLSession(configuration: sseCfg)
    }

    private func makeRequest(path: String, method: String = "GET", body: Data? = nil) throws -> URLRequest {
        guard let url = URL(string: "http://127.0.0.1:\(port)\(path)") else {
            throw HubClientError.invalidURL
        }
        var req = URLRequest(url: url)
        req.httpMethod = method
        req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        req.setValue("ios", forHTTPHeaderField: "x-crossagents-client-os")
        if let body {
            req.httpBody = body
            req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }
        return req
    }

    private func encodePathSegment(_ value: String) -> String {
        value.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed)?
            .replacingOccurrences(of: "/", with: "%2F") ?? value
    }

    private func encodeQueryValue(_ value: String) -> String {
        value.addingPercentEncoding(withAllowedCharacters: .urlQueryAllowed)?
            .replacingOccurrences(of: "&", with: "%26")
            .replacingOccurrences(of: "+", with: "%2B")
            .replacingOccurrences(of: "=", with: "%3D") ?? value
    }

    private func request<T: Decodable>(_ path: String, method: String = "GET", body: (any Encodable)? = nil) async throws -> T {
        let data = try await requestRaw(path, method: method, body: body)
        return try JSONDecoder().decode(T.self, from: data)
    }

    private func requestVoid(_ path: String, method: String = "POST", body: (any Encodable)? = nil) async throws {
        _ = try await requestRaw(path, method: method, body: body)
    }

    private func requestRaw(_ path: String, method: String, body: (any Encodable)?) async throws -> Data {
        let encodedBody = try body.map { try JSONEncoder().encode($0) }
        let req = try makeRequest(path: path, method: method, body: encodedBody)
        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await session.data(for: req)
        } catch {
            throw HubClientError.transport(error.localizedDescription)
        }
        guard let http = response as? HTTPURLResponse else {
            throw HubClientError.transport("无效的 HTTP 响应")
        }
        if http.statusCode == 401 {
            let msg = parseErrorMessage(data) ?? "设备凭据已失效或被吊销"
            throw HubClientError.unauthorized(msg)
        }
        if http.statusCode >= 400 {
            let envelope = try? JSONDecoder().decode(ApiErrorEnvelope.self, from: data)
            throw HubClientError.http(
                status: http.statusCode,
                code: envelope?.error.code ?? "http_\(http.statusCode)",
                message: envelope?.error.message ?? (String(data: data, encoding: .utf8) ?? "HTTP \(http.statusCode)")
            )
        }
        return data
    }

    private func parseErrorMessage(_ data: Data) -> String? {
        (try? JSONDecoder().decode(ApiErrorEnvelope.self, from: data))?.error.message
    }

    // MARK: - REST Endpoints

    func info() async throws -> HubInfo {
        try await request("/v1/info")
    }

    func profile() async throws -> UserProfile {
        try await request("/v1/profile")
    }

    func listAgents() async throws -> [Agent] {
        let res: ListAgentsResponse = try await request("/v1/agents")
        return res.agents
    }

    func createAgent(_ req: CreateAgentRequest) async throws -> Agent {
        try await request("/v1/agents", method: "POST", body: req)
    }

    func transcript(agentId: String) async throws -> Transcript {
        try await request("/v1/agents/\(encodePathSegment(agentId))/transcript")
    }

    func sendMessage(agentId: String, req: SendMessageRequest) async throws -> SendMessageResponse {
        try await request("/v1/agents/\(encodePathSegment(agentId))/messages", method: "POST", body: req)
    }

    func sendMessageNow(agentId: String, messageId: String) async throws -> SendMessageResponse {
        try await request(
            "/v1/agents/\(encodePathSegment(agentId))/messages/\(encodePathSegment(messageId))/send-now",
            method: "POST"
        )
    }

    func cancelAgent(agentId: String) async throws {
        try await requestVoid("/v1/agents/\(encodePathSegment(agentId))/cancel", method: "POST")
    }

    func compactAgent(agentId: String) async throws {
        try await requestVoid("/v1/agents/\(encodePathSegment(agentId))/compact", method: "POST")
    }

    func setModel(agentId: String, model: String?, effort: String?) async throws -> Agent {
        try await request(
            "/v1/agents/\(encodePathSegment(agentId))/model",
            method: "POST",
            body: SetModelRequest(model: model, effort: effort)
        )
    }

    func archiveAgent(agentId: String, archived: Bool) async throws -> Agent {
        try await request(
            "/v1/agents/\(encodePathSegment(agentId))/archive",
            method: "POST",
            body: ArchiveAgentRequest(archived: archived)
        )
    }

    func listNodes() async throws -> [RemoteNodeInfo] {
        let res: ListNodesResponse = try await request("/v1/nodes")
        return res.nodes
    }

    func nodeDirs(node: String, path: String = "") async throws -> NodeDirsResponse {
        let query = path.isEmpty ? "" : "?path=\(encodeQueryValue(path))"
        return try await request("/v1/nodes/\(encodePathSegment(node))/dirs\(query)")
    }

    func listPlugins(node: String? = nil) async throws -> [PluginInfo] {
        let query: String
        if let node, !node.isEmpty, node != "local" {
            query = "?node=\(encodeQueryValue(node))"
        } else {
            query = ""
        }
        let res: ListPluginsResponse = try await request("/v1/plugins\(query)")
        return res.plugins
    }

    func quota() async throws -> QuotaResponse {
        try await request("/v1/quota")
    }

    func questions(all: Bool = false) async throws -> [HumanItem] {
        let res: ListQuestionsResponse = try await request("/v1/questions\(all ? "?all=true" : "")")
        return res.items
    }

    func answerQuestion(id: String, answer: String) async throws -> HumanItem {
        try await request(
            "/v1/questions/\(encodePathSegment(id))/answer",
            method: "POST",
            body: AnswerRequest(answer: answer)
        )
    }

    func dismissQuestion(id: String) async throws {
        try await requestVoid("/v1/questions/\(encodePathSegment(id))/dismiss", method: "POST")
    }

    func avatar(id: String) async throws -> AttachmentContent {
        try await request("/v1/avatars/\(encodePathSegment(id))")
    }

    func attachment(agentId: String, id: String) async throws -> AttachmentContent {
        try await request("/v1/agents/\(encodePathSegment(agentId))/attachments/\(encodePathSegment(id))")
    }

    // MARK: - Live SSE Event Stream

    func streamEvents(since: UInt64?) -> AsyncThrowingStream<HubSSEEvent, Error> {
        let query = since.map { "since=\($0)" } ?? "live=true"
        let path = "/v1/events?\(query)"

        return AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    var req = try makeRequest(path: path, method: "GET")
                    req.setValue("text/event-stream", forHTTPHeaderField: "Accept")
                    req.setValue("no-cache", forHTTPHeaderField: "Cache-Control")

                    let (bytes, response) = try await streamSession.bytes(for: req)
                    guard let http = response as? HTTPURLResponse else {
                        throw HubClientError.transport("事件流连接失败")
                    }
                    if http.statusCode == 401 {
                        throw HubClientError.unauthorized("设备凭据已失效或被吊销")
                    }
                    guard (200..<300).contains(http.statusCode) else {
                        throw HubClientError.http(status: http.statusCode, code: "sse_http", message: "事件流 HTTP \(http.statusCode)")
                    }

                    let decoder = JSONDecoder()
                    for try await line in bytes.lines {
                        if Task.isCancelled { break }
                        guard line.hasPrefix("data:") else { continue }
                        let payload = line.dropFirst(5).trimmingCharacters(in: .whitespaces)
                        guard !payload.isEmpty, let jsonData = payload.data(using: .utf8) else { continue }
                        if let event = try? decoder.decode(HubSSEEvent.self, from: jsonData) {
                            continuation.yield(event)
                        }
                    }
                    continuation.finish()
                } catch {
                    if Task.isCancelled {
                        continuation.finish()
                    } else {
                        continuation.finish(throwing: error)
                    }
                }
            }
            continuation.onTermination = { _ in
                task.cancel()
            }
        }
    }
}

// MARK: - Image Base64 Helpers

enum ImageCodec {
    static func decodeBase64Image(_ raw: String) -> UIImage? {
        let cleaned: String
        if let comma = raw.firstIndex(of: ","), raw.hasPrefix("data:") {
            cleaned = String(raw[raw.index(after: comma)...])
        } else {
            cleaned = raw
        }
        guard let data = Data(base64Encoded: cleaned, options: .ignoreUnknownCharacters) else {
            return nil
        }
        return UIImage(data: data)
    }

    /// Resizes large camera photos to keep uploads responsive over mobile networks.
    static func prepareUpload(image: UIImage, name: String) -> AttachmentUpload? {
        let maxDimension: CGFloat = 1600
        let size = image.size
        let targetImage: UIImage
        if max(size.width, size.height) > maxDimension {
            let scale = maxDimension / max(size.width, size.height)
            let newSize = CGSize(width: size.width * scale, height: size.height * scale)
            let renderer = UIGraphicsImageRenderer(size: newSize)
            targetImage = renderer.image { _ in
                image.draw(in: CGRect(origin: .zero, size: newSize))
            }
        } else {
            targetImage = image
        }
        guard let jpeg = targetImage.jpegData(compressionQuality: 0.82) else { return nil }
        return AttachmentUpload(
            name: name.hasSuffix(".jpg") ? name : "\(name).jpg",
            mime: "image/jpeg",
            data: jpeg.base64EncodedString()
        )
    }
}
