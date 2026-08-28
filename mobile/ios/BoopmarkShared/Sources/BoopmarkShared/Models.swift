import Foundation

/// The API's bookmark representation. Keep this type in lockstep with
/// `server/src/domain/bookmark.rs` so both the app and Share Extension use
/// exactly the same wire format.
public struct Bookmark: Codable, Identifiable, Equatable, Hashable, Sendable {
    public let id: UUID
    public let userID: UUID
    public let url: URL
    public let title: String?
    public let description: String?
    public let imageURL: URL?
    public let overrideImageURL: URL?
    public let domain: String?
    public let tags: [String]
    public let createdAt: Date
    public let updatedAt: Date

    private enum CodingKeys: String, CodingKey {
        case id, url, title, description, domain, tags, createdAt, updatedAt
        case userID = "userId"
        case imageURL = "imageUrl"
        case overrideImageURL = "overrideImageUrl"
    }

    public init(
        id: UUID,
        userID: UUID,
        url: URL,
        title: String? = nil,
        description: String? = nil,
        imageURL: URL? = nil,
        overrideImageURL: URL? = nil,
        domain: String? = nil,
        tags: [String] = [],
        createdAt: Date,
        updatedAt: Date
    ) {
        self.id = id
        self.userID = userID
        self.url = url
        self.title = title
        self.description = description
        self.imageURL = imageURL
        self.overrideImageURL = overrideImageURL
        self.domain = domain
        self.tags = tags
        self.createdAt = createdAt
        self.updatedAt = updatedAt
    }

    public var displayTitle: String {
        let cleaned = title?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return cleaned.isEmpty ? (domain ?? url.host ?? url.absoluteString) : cleaned
    }

    public var effectiveImageURL: URL? { overrideImageURL ?? imageURL }
}

public struct CreateBookmark: Codable, Equatable, Sendable {
    public var url: URL
    public var title: String?
    public var description: String?
    public var imageURL: URL?
    public var domain: String?
    public var tags: [String]?

    private enum CodingKeys: String, CodingKey {
        case url, title, description, domain, tags
        case imageURL = "imageUrl"
    }

    public init(
        url: URL,
        title: String? = nil,
        description: String? = nil,
        imageURL: URL? = nil,
        domain: String? = nil,
        tags: [String]? = nil
    ) {
        self.url = url
        self.title = title
        self.description = description
        self.imageURL = imageURL
        self.domain = domain
        self.tags = tags
    }
}

public struct UpdateBookmark: Codable, Equatable, Sendable {
    public var title: String?
    public var description: String?
    public var tags: [String]?

    public init(title: String? = nil, description: String? = nil, tags: [String]? = nil) {
        self.title = title
        self.description = description
        self.tags = tags
    }
}

public struct SuggestionResult: Codable, Equatable, Sendable {
    public let title: String?
    public let description: String?
    public let imageURL: URL?
    public let domain: String?
    public let tags: [String]

    private enum CodingKeys: String, CodingKey {
        case title, description, domain, tags
        case imageURL = "imageUrl"
    }
}

public struct URLMetadata: Codable, Equatable, Sendable {
    public let title: String?
    public let description: String?
    public let imageURL: URL?
    public let domain: String?

    private enum CodingKeys: String, CodingKey {
        case title, description, domain
        case imageURL = "imageUrl"
    }
}

public struct APIErrorBody: Codable, Sendable {
    public let error: String
}

/// A capture is deliberately independent of a server bookmark ID. It can be
/// persisted by the Share Extension while the phone is offline and retried by
/// the containing app later.
public struct PendingCapture: Codable, Identifiable, Equatable, Sendable {
    public let id: UUID
    public let url: URL
    public var title: String?
    public var note: String?
    public var tags: [String]
    public let createdAt: Date
    public var lastError: String?

    public init(
        id: UUID = UUID(),
        url: URL,
        title: String? = nil,
        note: String? = nil,
        tags: [String] = [],
        createdAt: Date = Date(),
        lastError: String? = nil
    ) {
        self.id = id
        self.url = url
        self.title = title
        self.note = note
        self.tags = tags
        self.createdAt = createdAt
        self.lastError = lastError
    }

    public var request: CreateBookmark {
        CreateBookmark(url: url, title: title, description: note, tags: tags.isEmpty ? nil : tags)
    }
}

public struct BookmarkQuery: Sendable {
    public var search: String?
    public var tags: [String]
    public var sort: BookmarkSort
    public var limit: Int
    public var offset: Int

    public init(
        search: String? = nil,
        tags: [String] = [],
        sort: BookmarkSort = .newest,
        limit: Int = 50,
        offset: Int = 0
    ) {
        self.search = search
        self.tags = tags
        self.sort = sort
        self.limit = limit
        self.offset = offset
    }
}

public enum BookmarkSort: String, Codable, CaseIterable, Sendable {
    case newest
    case oldest
    case title
    case domain
}

extension JSONDecoder {
    /// API timestamps are RFC3339 UTC strings (for example, `2026-08-22T12:00:00Z`).
    public static var boopmark: JSONDecoder {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        decoder.dateDecodingStrategy = .custom { value in
            let container = try value.singleValueContainer()
            let string = try container.decode(String.self)
            guard let date = ISO8601DateParser.date(from: string) else {
                throw DecodingError.dataCorruptedError(
                    in: container,
                    debugDescription: "Expected an RFC3339 timestamp, got \(string)"
                )
            }
            return date
        }
        return decoder
    }
}

extension JSONEncoder {
    public static var boopmark: JSONEncoder {
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        encoder.dateEncodingStrategy = .iso8601
        return encoder
    }
}

enum ISO8601DateParser {
    static func date(from string: String) -> Date? {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withDashSeparatorInDate, .withColonSeparatorInTime]
        if let date = formatter.date(from: string) { return date }
        formatter.formatOptions.insert(.withFractionalSeconds)
        return formatter.date(from: string)
    }
}
