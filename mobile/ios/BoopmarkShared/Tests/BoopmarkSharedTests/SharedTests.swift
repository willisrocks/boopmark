import Foundation
import XCTest
@testable import BoopmarkShared

final class SharedTests: XCTestCase {
    func testBookmarkDecodesServerSnakeCaseAndFractionalTimestamp() throws {
        let json = #"""
        {
          "id": "11111111-1111-1111-1111-111111111111",
          "user_id": "22222222-2222-2222-2222-222222222222",
          "url": "https://boopmark.example/articles/swift",
          "title": "Swift on the go",
          "description": "A useful article",
          "image_url": "https://boopmark.example/image.png",
          "override_image_url": null,
          "domain": "boopmark.example",
          "tags": ["swift", "mobile"],
          "created_at": "2026-08-22T12:00:00.123Z",
          "updated_at": "2026-08-22T12:00:01Z"
        }
        """#.data(using: .utf8)!

        let bookmark = try JSONDecoder.boopmark.decode(Bookmark.self, from: json)
        XCTAssertEqual(bookmark.title, "Swift on the go")
        XCTAssertEqual(bookmark.tags, ["swift", "mobile"])
        XCTAssertEqual(bookmark.domain, "boopmark.example")
        XCTAssertEqual(bookmark.url.absoluteString, "https://boopmark.example/articles/swift")
        XCTAssertEqual(bookmark.createdAt.timeIntervalSince1970, 1787400000.123, accuracy: 0.001)
    }

    func testServerURLValidationNormalizesAndProtectsCredentials() throws {
        XCTAssertEqual(
            try ServerURLValidator.normalize(" boopmark.example/ ").absoluteString,
            "https://boopmark.example"
        )
        XCTAssertEqual(
            try ServerURLValidator.normalize("http://localhost:4000/boopmark/").absoluteString,
            "http://localhost:4000/boopmark"
        )
        XCTAssertThrowsError(try ServerURLValidator.normalize("http://boopmark.example")) { error in
            XCTAssertEqual(error as? ServerURLValidationError, .unsupportedScheme)
        }
        XCTAssertThrowsError(try ServerURLValidator.normalize("https://user:pass@boopmark.example")) { error in
            XCTAssertEqual(error as? ServerURLValidationError, .credentialsNotAllowed)
        }
    }

    func testBookmarkURLValidationPreservesQueryAndFragment() throws {
        let original = "https://example.com/article?id=42#comments"
        XCTAssertEqual(try BookmarkURLValidator.validate(original).absoluteString, original)
        XCTAssertThrowsError(try BookmarkURLValidator.validate("file:///tmp/private"))
        XCTAssertThrowsError(try BookmarkURLValidator.validate("https://user:pass@example.com/private"))
    }

    func testCaptureQueueFlushesOnlyAfterSuccessfulCreate() async throws {
        let store = MemoryStore()
        let queue = CaptureQueue(store: store)
        let first = PendingCapture(url: URL(string: "https://example.com/one")!)
        let second = PendingCapture(url: URL(string: "https://example.com/two")!)
        try await queue.enqueue(first)
        try await queue.enqueue(second)

        let api = FakeAPI(failuresRemaining: 1)
        let failedSendCount = try await queue.flush(using: api)
        XCTAssertEqual(failedSendCount, 0)
        let pendingAfterFailure = try await queue.pending()
        XCTAssertEqual(pendingAfterFailure.map(\.id), [first.id, second.id])
        XCTAssertNotNil(pendingAfterFailure.first?.lastError)

        let sentCount = try await queue.flush(using: api)
        XCTAssertEqual(sentCount, 2)
        let pendingAfterSuccess = try await queue.pending()
        XCTAssertTrue(pendingAfterSuccess.isEmpty)
        let createdURLs = await api.createdURLs
        XCTAssertEqual(createdURLs, [first.url, second.url])
    }

    func testTwoQueueInstancesDoNotOverwriteEachOthersCaptures() async throws {
        let store = MemoryStore()
        let firstQueue = CaptureQueue(store: store)
        let secondQueue = CaptureQueue(store: store)
        let first = PendingCapture(url: URL(string: "https://example.com/first")!)
        let second = PendingCapture(url: URL(string: "https://example.com/second")!)

        try await firstQueue.enqueue(first)
        try await secondQueue.enqueue(second)

        let pending = try await firstQueue.pending()
        XCTAssertEqual(pending.map(\.url), [first.url, second.url])
    }

    func testFileQueueStoreMergesTwoInstances() async throws {
        let fileURL = FileManager.default.temporaryDirectory
            .appendingPathComponent("boopmark-queue-\(UUID().uuidString).json")
        defer {
            try? FileManager.default.removeItem(at: fileURL)
            try? FileManager.default.removeItem(at: fileURL.appendingPathExtension("lock"))
        }
        let firstQueue = CaptureQueue(store: FileCaptureQueueStore(fileURL: fileURL))
        let secondQueue = CaptureQueue(store: FileCaptureQueueStore(fileURL: fileURL))
        try await firstQueue.enqueue(PendingCapture(url: URL(string: "https://example.com/first")!))
        try await secondQueue.enqueue(PendingCapture(url: URL(string: "https://example.com/second")!))

        let pending = try await firstQueue.pending()
        XCTAssertEqual(pending.count, 2)
    }
}

private actor MemoryStore: CaptureQueueStore {
    var value: [PendingCapture] = []
    func load() async throws -> [PendingCapture] { value }
    func save(_ captures: [PendingCapture]) async throws { value = captures }
    func mutate(_ transform: @escaping @Sendable ([PendingCapture]) -> [PendingCapture]) async throws -> [PendingCapture] {
        value = transform(value)
        return value
    }
}

private actor FakeAPI: BoopmarkAPIProtocol {
    var failuresRemaining: Int
    var createdURLs: [URL] = []

    init(failuresRemaining: Int) { self.failuresRemaining = failuresRemaining }

    func list(_ query: BookmarkQuery) async throws -> [Bookmark] { [] }

    func create(_ input: CreateBookmark, suggest: Bool) async throws -> Bookmark {
        if failuresRemaining > 0 {
            failuresRemaining -= 1
            throw BoopmarkAPIError.transport("offline")
        }
        createdURLs.append(input.url)
        return Bookmark(
            id: UUID(), userID: UUID(), url: input.url,
            createdAt: Date(), updatedAt: Date()
        )
    }

    func suggest(url: URL) async throws -> SuggestionResult {
        fatalError("not used")
    }

    func update(id: UUID, input: UpdateBookmark, suggest: Bool) async throws -> Bookmark {
        fatalError("not used")
    }

    func delete(id: UUID) async throws {}
}
