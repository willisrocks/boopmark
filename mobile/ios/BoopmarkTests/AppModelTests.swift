@testable import Boopmark
import BoopmarkShared
import XCTest

@MainActor
final class AppModelTests: XCTestCase {
    func testConfigureLoadsBookmarksBeforeReturning() async throws {
        let expected = Bookmark(
            id: UUID(uuidString: "10000000-0000-0000-0000-000000000001")!,
            userID: UUID(uuidString: "20000000-0000-0000-0000-000000000001")!,
            url: URL(string: "https://example.com/saved")!,
            title: "Already saved",
            description: nil,
            domain: "example.com",
            tags: ["test"],
            createdAt: Date(timeIntervalSince1970: 1_700_000_000),
            updatedAt: Date(timeIntervalSince1970: 1_700_000_000)
        )
        let api = StubAPI(bookmarks: [expected])
        let model = AppModel(
            settingsStore: MemorySettingsStore(),
            queue: CaptureQueue(store: MemoryQueueStore()),
            apiFactory: { _, _ in api }
        )

        try await model.configure(serverURLText: "https://boopmark.com", apiKey: "test-key")

        XCTAssertEqual(model.bookmarks, [expected])
        let listCallCount = await api.listCallCount
        XCTAssertEqual(listCallCount, 1)
        XCTAssertNil(model.errorMessage)
    }

    func testSyncFromServerLoadsBookmarkCreatedByAnotherProcess() async throws {
        let initial = makeBookmark(
            id: "10000000-0000-0000-0000-000000000001",
            url: "https://example.com/initial",
            title: "Initial bookmark"
        )
        let shared = makeBookmark(
            id: "10000000-0000-0000-0000-000000000002",
            url: "https://example.com/shared",
            title: "Created from Share Extension"
        )
        let api = StubAPI(bookmarks: [initial])
        let model = AppModel(
            settingsStore: MemorySettingsStore(),
            queue: CaptureQueue(store: MemoryQueueStore()),
            apiFactory: { _, _ in api }
        )
        try await model.configure(serverURLText: "https://boopmark.com", apiKey: "test-key")

        await api.setBookmarks([shared, initial])
        await model.syncFromServer()

        XCTAssertEqual(model.bookmarks, [shared, initial])
        let listCallCount = await api.listCallCount
        XCTAssertEqual(listCallCount, 2)
        XCTAssertNil(model.errorMessage)
    }

    func testOldConnectionResponseCannotOverwriteNewConnection() async throws {
        let oldBookmark = makeBookmark(
            id: "10000000-0000-0000-0000-000000000003",
            url: "https://old.example/bookmark",
            title: "Old server bookmark"
        )
        let newBookmark = makeBookmark(
            id: "10000000-0000-0000-0000-000000000004",
            url: "https://new.example/bookmark",
            title: "New server bookmark"
        )
        let oldAPI = StubAPI(bookmarks: [oldBookmark], delayAfterFirstList: 200_000_000)
        let newAPI = StubAPI(bookmarks: [newBookmark])
        let model = AppModel(
            settingsStore: MemorySettingsStore(),
            queue: CaptureQueue(store: MemoryQueueStore()),
            apiFactory: { url, _ in url.host == "old.example" ? oldAPI : newAPI }
        )
        try await model.configure(serverURLText: "https://old.example", apiKey: "old-key")

        let staleRefresh = Task { await model.refresh() }
        try await Task.sleep(nanoseconds: 20_000_000)
        try await model.configure(serverURLText: "https://new.example", apiKey: "new-key")
        await staleRefresh.value

        XCTAssertEqual(model.bookmarks, [newBookmark])
        XCTAssertEqual(model.settings.serverURL, URL(string: "https://new.example"))
        XCTAssertNil(model.errorMessage)
    }

    private func makeBookmark(id: String, url: String, title: String) -> Bookmark {
        Bookmark(
            id: UUID(uuidString: id)!,
            userID: UUID(uuidString: "20000000-0000-0000-0000-000000000001")!,
            url: URL(string: url)!,
            title: title,
            description: nil,
            domain: "example.com",
            tags: ["test"],
            createdAt: Date(timeIntervalSince1970: 1_700_000_000),
            updatedAt: Date(timeIntervalSince1970: 1_700_000_000)
        )
    }
}

private final class MemorySettingsStore: @unchecked Sendable, SettingsStore {
    private var serverURL: URL?
    private var apiKey: String?

    func load() throws -> BoopmarkSettings {
        BoopmarkSettings(serverURL: serverURL, apiKey: apiKey)
    }

    func saveServerURL(_ url: URL?) throws { serverURL = url }
    func saveAPIKey(_ key: String?) throws { apiKey = key }
}

private actor MemoryQueueStore: CaptureQueueStore {
    private var captures: [PendingCapture] = []

    func load() async throws -> [PendingCapture] { captures }
    func save(_ captures: [PendingCapture]) async throws { self.captures = captures }
    func mutate(
        _ transform: @escaping @Sendable ([PendingCapture]) -> [PendingCapture]
    ) async throws -> [PendingCapture] {
        captures = transform(captures)
        return captures
    }
}

private actor StubAPI: BoopmarkAPIProtocol {
    private(set) var listCallCount = 0
    private var bookmarks: [Bookmark]
    private let delayAfterFirstList: UInt64

    init(bookmarks: [Bookmark], delayAfterFirstList: UInt64 = 0) {
        self.bookmarks = bookmarks
        self.delayAfterFirstList = delayAfterFirstList
    }

    func list(_ query: BookmarkQuery) async throws -> [Bookmark] {
        listCallCount += 1
        if listCallCount > 1, delayAfterFirstList > 0 {
            try await Task.sleep(nanoseconds: delayAfterFirstList)
        }
        return bookmarks
    }

    func setBookmarks(_ bookmarks: [Bookmark]) {
        self.bookmarks = bookmarks
    }

    func create(_ input: CreateBookmark, suggest: Bool) async throws -> Bookmark {
        throw StubError.notImplemented
    }

    func suggest(url: URL) async throws -> SuggestionResult {
        throw StubError.notImplemented
    }

    func update(id: UUID, input: UpdateBookmark, suggest: Bool) async throws -> Bookmark {
        throw StubError.notImplemented
    }

    func delete(id: UUID) async throws {
        throw StubError.notImplemented
    }
}

private enum StubError: Error {
    case notImplemented
}
