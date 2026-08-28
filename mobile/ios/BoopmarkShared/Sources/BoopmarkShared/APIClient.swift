import Foundation

public enum BoopmarkAPIError: Error, LocalizedError, Equatable, Sendable {
    case missingBaseURL
    case invalidURL
    case unauthorized
    case conflict
    case invalidResponse(Int, String)
    case transport(String)
    case encoding(String)

    public var errorDescription: String? {
        switch self {
        case .missingBaseURL: return "Add your Boopmark server URL in Settings."
        case .invalidURL: return "The server URL is not valid."
        case .unauthorized: return "Your Boopmark API key is invalid or expired."
        case .conflict: return "That bookmark already exists."
        case let .invalidResponse(status, message): return "Boopmark returned HTTP \(status): \(message)"
        case let .transport(message): return message
        case let .encoding(message): return message
        }
    }

    /// Only transport failures are safe to retry from the offline queue.
    /// HTTP errors are actionable responses and must stay visible to the user.
    public var isRetryableOffline: Bool {
        if case .transport = self { return true }
        return false
    }
}

public protocol BoopmarkAPIProtocol: Sendable {
    func list(_ query: BookmarkQuery) async throws -> [Bookmark]
    func create(_ input: CreateBookmark, suggest: Bool) async throws -> Bookmark
    func suggest(url: URL) async throws -> SuggestionResult
    func update(id: UUID, input: UpdateBookmark, suggest: Bool) async throws -> Bookmark
    func delete(id: UUID) async throws
}

/// A small actor-based HTTP client shared by the app and Share Extension.
/// Bearer API keys are never placed in URLs or logs.
public actor BoopmarkAPI: BoopmarkAPIProtocol {
    public let baseURL: URL
    private let token: String
    private let session: URLSession
    private let decoder: JSONDecoder
    private let encoder: JSONEncoder

    public init(baseURL: URL, token: String, session: URLSession = .shared) {
        self.baseURL = baseURL
        self.token = token
        self.session = session
        self.decoder = .boopmark
        self.encoder = .boopmark
    }

    public func list(_ query: BookmarkQuery = BookmarkQuery()) async throws -> [Bookmark] {
        var components = try endpoint("/api/v1/bookmarks")
        var items = [URLQueryItem(name: "sort", value: query.sort.rawValue)]
        items.append(URLQueryItem(name: "limit", value: String(max(1, min(query.limit, 100)))))
        items.append(URLQueryItem(name: "offset", value: String(max(0, query.offset))))
        if let search = query.search?.trimmingCharacters(in: .whitespacesAndNewlines), !search.isEmpty {
            items.append(URLQueryItem(name: "search", value: search))
        }
        if !query.tags.isEmpty { items.append(URLQueryItem(name: "tags", value: query.tags.joined(separator: ","))) }
        components.queryItems = items
        return try await send(
            components.url!,
            method: "GET",
            body: Optional<String>.none,
            response: [Bookmark].self
        )
    }

    public func create(_ input: CreateBookmark, suggest: Bool = true) async throws -> Bookmark {
        var components = try endpoint("/api/v1/bookmarks")
        if suggest { components.queryItems = [URLQueryItem(name: "suggest", value: "true")] }
        return try await send(components.url!, method: "POST", body: input, response: Bookmark.self)
    }

    public func suggest(url: URL) async throws -> SuggestionResult {
        let endpoint = try endpoint("/api/v1/bookmarks/suggest").url!
        return try await send(endpoint, method: "POST", body: ["url": url.absoluteString], response: SuggestionResult.self)
    }

    public func update(id: UUID, input: UpdateBookmark, suggest: Bool = false) async throws -> Bookmark {
        var components = try endpoint("/api/v1/bookmarks/\(id.uuidString)")
        if suggest { components.queryItems = [URLQueryItem(name: "suggest", value: "true")] }
        return try await send(components.url!, method: "PUT", body: input, response: Bookmark.self)
    }

    public func delete(id: UUID) async throws {
        let url = try endpoint("/api/v1/bookmarks/\(id.uuidString)").url!
        _ = try await send(url, method: "DELETE", body: Optional<String>.none, response: EmptyResponse.self)
    }

    private func endpoint(_ path: String) throws -> URLComponents {
        guard let safeBaseURL = try? ServerURLValidator.normalize(baseURL.absoluteString),
              var components = URLComponents(url: safeBaseURL, resolvingAgainstBaseURL: false) else {
            throw BoopmarkAPIError.invalidURL
        }
        let basePath = components.path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        components.path = "/" + ([basePath, path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))]
            .filter { !$0.isEmpty }
            .joined(separator: "/"))
        components.query = nil
        components.fragment = nil
        guard components.url != nil else { throw BoopmarkAPIError.invalidURL }
        return components
    }

    private func send<Body: Encodable, Response: Decodable>(
        _ url: URL,
        method: String,
        body: Body?,
        response: Response.Type
    ) async throws -> Response {
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        if let body {
            do { request.httpBody = try encoder.encode(body) }
            catch { throw BoopmarkAPIError.encoding(error.localizedDescription) }
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        }

        let data: Data
        let http: HTTPURLResponse
        do {
            let result = try await session.data(for: request)
            guard let response = result.1 as? HTTPURLResponse else {
                throw BoopmarkAPIError.transport("Boopmark returned an invalid response.")
            }
            data = result.0
            http = response
        } catch let error as BoopmarkAPIError {
            throw error
        } catch {
            throw BoopmarkAPIError.transport(error.localizedDescription)
        }

        guard (200..<300).contains(http.statusCode) else {
            let message = (try? decoder.decode(APIErrorBody.self, from: data).error)
                ?? HTTPURLResponse.localizedString(forStatusCode: http.statusCode)
            switch http.statusCode {
            case 401, 403: throw BoopmarkAPIError.unauthorized
            case 409: throw BoopmarkAPIError.conflict
            default: throw BoopmarkAPIError.invalidResponse(http.statusCode, message)
            }
        }
        if Response.self == EmptyResponse.self { return EmptyResponse() as! Response }
        do { return try decoder.decode(Response.self, from: data) }
        catch { throw BoopmarkAPIError.invalidResponse(http.statusCode, error.localizedDescription) }
    }
}

public struct EmptyResponse: Decodable, Sendable {
    public init() {}
}
