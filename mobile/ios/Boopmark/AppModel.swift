import BoopmarkShared
import Combine
import Foundation

@MainActor
final class AppModel: ObservableObject {
    @Published var settings: BoopmarkSettings
    @Published private(set) var bookmarks: [Bookmark] = []
    @Published private(set) var pendingCaptures: [PendingCapture] = []
    @Published private(set) var isLoading = false
    @Published var errorMessage: String?
    @Published var noticeMessage: String?

    private let settingsStore: SettingsStore
    private let queue: CaptureQueue
    private let apiFactory: (URL, String) -> any BoopmarkAPIProtocol
    private var api: (any BoopmarkAPIProtocol)?
    private var activeQuery = BookmarkQuery()
    private var bookmarkLoadGeneration = 0

    init(
        settingsStore: SettingsStore = UserDefaultsSettingsStore(),
        queue: CaptureQueue? = nil,
        apiFactory: @escaping (URL, String) -> any BoopmarkAPIProtocol = { url, key in
            BoopmarkAPI(baseURL: url, token: key)
        }
    ) {
        self.settingsStore = settingsStore
        self.apiFactory = apiFactory
        let loadedSettings: BoopmarkSettings
        let settingsError: String?
        do {
            loadedSettings = try settingsStore.load()
            settingsError = nil
        } catch {
            loadedSettings = BoopmarkSettings()
            settingsError = error.localizedDescription
        }
        self.settings = loadedSettings
        self.queue = queue ?? CaptureQueue(store: Self.queueStore())
        self.errorMessage = settingsError
        rebuildAPI()
        Task { await refreshPendingCaptures() }
    }

    var isConfigured: Bool { settings.isConfigured && api != nil }

    func configure(serverURLText: String, apiKey: String) async throws {
        let serverURL = try ServerURLValidator.normalize(serverURLText)
        let cleanKey = apiKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !cleanKey.isEmpty else { throw BoopmarkAPIError.unauthorized }
        let verified: BoopmarkSettings
        do {
            // Clear the credential before changing its origin. A partial
            // failure can leave the app disconnected, but can never pair an
            // old bearer token with a newly entered server.
            try settingsStore.saveAPIKey(nil)
            try settingsStore.saveServerURL(serverURL)
            try settingsStore.saveAPIKey(cleanKey)
            verified = try settingsStore.load()
            guard verified.serverURL == serverURL, verified.apiKey == cleanKey else {
                throw SettingsStoreError.verificationFailed
            }
        } catch {
            try? settingsStore.saveAPIKey(nil)
            try? settingsStore.saveServerURL(nil)
            settings = BoopmarkSettings()
            rebuildAPI()
            throw error
        }
        settings = verified
        rebuildAPI()
        try await loadBookmarks(query: activeQuery)
        noticeMessage = "Settings saved."
    }

    func refresh(query: BookmarkQuery = BookmarkQuery()) async {
        activeQuery = query
        // An unconfigured app is a valid first-launch state. RootView already
        // presents the connection prompt, so do not cover it with an alert.
        guard api != nil else { return }
        try? await loadBookmarks(query: query)
    }

    /// Reconcile process-local state with the server whenever the app becomes
    /// active. The Share Extension runs in a separate process, so its successful
    /// create cannot directly update this model's in-memory bookmark array.
    func syncFromServer(query: BookmarkQuery = BookmarkQuery()) async {
        await refresh(query: query)
        await refreshPendingCaptures()
    }

    func capture(
        url: URL,
        title: String? = nil,
        note: String? = nil,
        tags: [String] = []
    ) async -> Bool {
        guard let safeURL = try? BookmarkURLValidator.validate(url.absoluteString) else {
            errorMessage = "Enter a valid HTTPS link (or a local HTTP link)."
            return false
        }
        let pending = PendingCapture(url: safeURL, title: title, note: note, tags: tags)
        guard let api else {
            return await saveOffline(pending, message: "Saved offline. Connect Boopmark to send it.")
        }
        do {
            let created = try await api.create(pending.request, suggest: true)
            upsert(created)
            await refresh(query: activeQuery)
            noticeMessage = "Saved to Boopmark."
            return true
        } catch let error as BoopmarkAPIError where error.isRetryableOffline {
            return await saveOffline(pending, message: "Saved offline. Send it when you’re connected.")
        } catch {
            errorMessage = error.localizedDescription
            return false
        }
    }

    func suggest(url: URL) async -> SuggestionResult? {
        guard let safeURL = try? BookmarkURLValidator.validate(url.absoluteString) else {
            errorMessage = "Enter a valid HTTPS link (or a local HTTP link)."
            return nil
        }
        guard let api else {
            errorMessage = "Connect your Boopmark server before requesting AI suggestions."
            return nil
        }
        do {
            let suggestion = try await api.suggest(url: safeURL)
            errorMessage = nil
            return suggestion
        } catch {
            errorMessage = error.localizedDescription
            return nil
        }
    }

    func update(bookmark: Bookmark, title: String, note: String, tags: [String]) async -> Bool {
        guard let api else {
            errorMessage = "Connect your Boopmark server before editing."
            return false
        }
        do {
            let updated = try await api.update(
                id: bookmark.id,
                input: UpdateBookmark(
                    title: title.trimmingCharacters(in: .whitespacesAndNewlines),
                    description: note.trimmingCharacters(in: .whitespacesAndNewlines),
                    tags: tags
                ),
                suggest: false
            )
            if let index = bookmarks.firstIndex(where: { $0.id == updated.id }) { bookmarks[index] = updated }
            await refresh(query: activeQuery)
            noticeMessage = "Bookmark updated."
            return true
        } catch { errorMessage = error.localizedDescription }
        return false
    }

    func delete(bookmark: Bookmark) async -> Bool {
        guard let api else {
            errorMessage = "Connect your Boopmark server before deleting."
            return false
        }
        do {
            try await api.delete(id: bookmark.id)
            bookmarks.removeAll { $0.id == bookmark.id }
            await refresh(query: activeQuery)
            noticeMessage = "Bookmark deleted."
            return true
        } catch {
            errorMessage = error.localizedDescription
            return false
        }
    }

    /// Sending queued captures is deliberately user initiated. A POST create
    /// is not idempotent, so silently retrying it when the app wakes could
    /// create duplicates.
    func sendQueuedCaptures() async {
        guard let api else {
            errorMessage = "Connect your Boopmark server before sending the queue."
            return
        }
        do {
            let sent = try await queue.flush(using: api)
            await refreshPendingCaptures()
            if sent > 0 { await refresh() }
            if sent == 0, let failure = pendingCaptures.first?.lastError {
                errorMessage = "The first queued capture could not be sent: \(failure)"
            } else {
                noticeMessage = sent == 0 ? "Nothing was sent." : "Sent \(sent) queued capture\(sent == 1 ? "" : "s")."
            }
        } catch { errorMessage = error.localizedDescription }
    }

    func removeQueuedCapture(id: UUID) async {
        do {
            try await queue.remove(id: id)
            await refreshPendingCaptures()
        } catch { errorMessage = error.localizedDescription }
    }

    func refreshPendingCaptures() async {
        do { pendingCaptures = try await queue.pending() }
        catch { errorMessage = error.localizedDescription }
    }

#if DEBUG
    /// Deterministic, fictional content used only to create App Store media.
    /// The flag that invokes this method is compiled out of Release builds.
    func loadAppStoreScreenshotFixtures() {
        let userID = UUID(uuidString: "00000000-0000-0000-0000-000000000001")!
        let now = Date()
        bookmarks = [
            Bookmark(
                id: UUID(uuidString: "10000000-0000-0000-0000-000000000001")!,
                userID: userID,
                url: URL(string: "https://developer.apple.com/swift/")!,
                title: "Swift: A powerful language for every platform",
                description: "Explore the language, tools, and community behind modern app development.",
                domain: "developer.apple.com",
                tags: ["swift", "ios", "development"],
                createdAt: now,
                updatedAt: now
            ),
            Bookmark(
                id: UUID(uuidString: "10000000-0000-0000-0000-000000000002")!,
                userID: userID,
                url: URL(string: "https://developer.mozilla.org/en-US/docs/Web")!,
                title: "Resources for developers, by developers",
                description: "A practical reference for building an open and accessible web.",
                domain: "developer.mozilla.org",
                tags: ["web", "reference"],
                createdAt: now.addingTimeInterval(-3600),
                updatedAt: now.addingTimeInterval(-3600)
            ),
            Bookmark(
                id: UUID(uuidString: "10000000-0000-0000-0000-000000000003")!,
                userID: userID,
                url: URL(string: "https://www.rfc-editor.org/rfc/rfc9110.html")!,
                title: "HTTP Semantics",
                description: "The core semantics and extensibility model of the HTTP protocol.",
                domain: "rfc-editor.org",
                tags: ["protocols", "reading"],
                createdAt: now.addingTimeInterval(-7200),
                updatedAt: now.addingTimeInterval(-7200)
            ),
            Bookmark(
                id: UUID(uuidString: "10000000-0000-0000-0000-000000000004")!,
                userID: userID,
                url: URL(string: "https://www.nngroup.com/articles/ten-usability-heuristics/")!,
                title: "10 usability heuristics for interface design",
                description: "Ten enduring principles for clear, useful, human-centered interfaces.",
                domain: "nngroup.com",
                tags: ["design", "ux"],
                createdAt: now.addingTimeInterval(-10800),
                updatedAt: now.addingTimeInterval(-10800)
            )
        ]
    }
#endif

    private func saveOffline(_ capture: PendingCapture, message: String) async -> Bool {
        do {
            try await queue.enqueue(capture)
            await refreshPendingCaptures()
            noticeMessage = message
            return true
        } catch { errorMessage = error.localizedDescription }
        return false
    }

    private func upsert(_ bookmark: Bookmark) {
        if let index = bookmarks.firstIndex(where: { $0.id == bookmark.id }) {
            bookmarks[index] = bookmark
        } else {
            bookmarks.insert(bookmark, at: 0)
        }
    }

    private func rebuildAPI() {
        // Invalidate any request started for the previous connection. Without
        // this guard, a slow response from an old server can overwrite the
        // freshly loaded list after the user saves new connection details.
        bookmarkLoadGeneration &+= 1
        isLoading = false
        bookmarks = []
        guard let url = settings.serverURL, let key = settings.apiKey, !key.isEmpty else {
            api = nil
            return
        }
        api = apiFactory(url, key)
    }

    private func loadBookmarks(query: BookmarkQuery) async throws {
        guard let api else { throw BoopmarkAPIError.missingBaseURL }
        activeQuery = query
        bookmarkLoadGeneration &+= 1
        let generation = bookmarkLoadGeneration
        isLoading = true
        do {
            let loaded = try await api.list(query)
            guard generation == bookmarkLoadGeneration else { return }
            bookmarks = loaded
            errorMessage = nil
            isLoading = false
        } catch {
            guard generation == bookmarkLoadGeneration else { throw error }
            errorMessage = error.localizedDescription
            isLoading = false
            throw error
        }
    }

    private static func queueStore() -> any CaptureQueueStore {
        if let appGroup = FileManager.default.containerURL(
            forSecurityApplicationGroupIdentifier: "group.com.boopmark.shared"
        ) {
            return FileCaptureQueueStore(fileURL: appGroup.appendingPathComponent("pending-captures.json"))
        }
        return UnavailableCaptureQueueStore()
    }
}

private extension String {
    var nilIfBlank: String? {
        let trimmed = trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}
