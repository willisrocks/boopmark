import Foundation

#if canImport(Darwin)
import Darwin
#elseif canImport(Glibc)
import Glibc
#endif

public protocol CaptureQueueStore: Sendable {
    func load() async throws -> [PendingCapture]
    func save(_ captures: [PendingCapture]) async throws
    /// Performs a read/modify/write while holding the store's cross-process
    /// lock. App and Share Extension instances must use this for mutations so
    /// one process cannot overwrite a capture added by the other.
    func mutate(_ transform: @escaping @Sendable ([PendingCapture]) -> [PendingCapture]) async throws -> [PendingCapture]
}

public enum CaptureQueueStoreError: Error, LocalizedError, Sendable {
    case unreadable
    case unwritable

    public var errorDescription: String? {
        switch self {
        case .unreadable: return "The offline capture queue could not be read."
        case .unwritable: return "The offline capture queue could not be saved."
        }
    }
}

/// Fails visibly when the signed app/extension cannot access its App Group.
public struct UnavailableCaptureQueueStore: CaptureQueueStore, Sendable {
    public init() {}
    public func load() async throws -> [PendingCapture] { throw CaptureQueueStoreError.unreadable }
    public func save(_ captures: [PendingCapture]) async throws { throw CaptureQueueStoreError.unwritable }
    public func mutate(
        _ transform: @escaping @Sendable ([PendingCapture]) -> [PendingCapture]
    ) async throws -> [PendingCapture] { throw CaptureQueueStoreError.unwritable }
}

/// JSON on disk keeps the Share Extension dependency-free and makes queued
/// captures inspectable during development. App Group URLs are supplied by
/// the app/extension, so this type is also straightforward to unit test.
public struct FileCaptureQueueStore: CaptureQueueStore, Sendable {
    public let fileURL: URL

    public init(fileURL: URL) { self.fileURL = fileURL }

    public func load() async throws -> [PendingCapture] {
        do {
            return try withExclusiveLock { try loadUnlocked() }
        } catch let error as CaptureQueueStoreError { throw error }
        catch { throw CaptureQueueStoreError.unreadable }
    }

    public func save(_ captures: [PendingCapture]) async throws {
        do {
            try withExclusiveLock { try saveUnlocked(captures) }
        } catch let error as CaptureQueueStoreError { throw error }
        catch { throw CaptureQueueStoreError.unwritable }
    }

    public func mutate(
        _ transform: @escaping @Sendable ([PendingCapture]) -> [PendingCapture]
    ) async throws -> [PendingCapture] {
        do {
            return try withExclusiveLock {
                let current = try loadUnlocked()
                let updated = transform(current)
                try saveUnlocked(updated)
                return updated
            }
        } catch let error as CaptureQueueStoreError { throw error }
        catch { throw CaptureQueueStoreError.unwritable }
    }

    private func loadUnlocked() throws -> [PendingCapture] {
        guard FileManager.default.fileExists(atPath: fileURL.path) else { return [] }
        let data = try Data(contentsOf: fileURL)
        return try JSONDecoder.boopmark.decode([PendingCapture].self, from: data)
    }

    private func saveUnlocked(_ captures: [PendingCapture]) throws {
        try FileManager.default.createDirectory(
            at: fileURL.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        let data = try JSONEncoder.boopmark.encode(captures)
        try data.write(to: fileURL, options: [.atomic])
    }

    private func withExclusiveLock<T>(_ body: () throws -> T) throws -> T {
        let lockURL = fileURL.appendingPathExtension("lock")
        try FileManager.default.createDirectory(
            at: lockURL.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        let descriptor = open(lockURL.path, O_CREAT | O_RDWR, S_IRUSR | S_IWUSR)
        guard descriptor >= 0 else { throw CaptureQueueStoreError.unwritable }
        guard flock(descriptor, LOCK_EX) == 0 else {
            close(descriptor)
            throw CaptureQueueStoreError.unwritable
        }
        defer {
            _ = flock(descriptor, LOCK_UN)
            close(descriptor)
        }
        return try body()
    }
}

public actor CaptureQueue {
    private let store: any CaptureQueueStore
    private var captures: [PendingCapture]

    public init(store: any CaptureQueueStore) {
        self.store = store
        self.captures = []
    }

    public func pending() async throws -> [PendingCapture] {
        captures = try await store.load()
        return captures
    }

    @discardableResult
    public func enqueue(_ capture: PendingCapture) async throws -> PendingCapture {
        captures = try await store.mutate { current in current + [capture] }
        return capture
    }

    /// Sends captures in FIFO order. A failed item stays queued with a useful
    /// error for the app's Settings/Inbox UI; later items are not lost.
    public func flush(using api: any BoopmarkAPIProtocol) async throws -> Int {
        captures = try await store.load()
        var sent = 0
        while let capture = captures.first {
            do {
                _ = try await api.create(capture.request, suggest: true)
                captures = try await store.mutate { current in
                    current.filter { $0.id != capture.id }
                }
                sent += 1
            } catch {
                let message = error.localizedDescription
                captures = try await store.mutate { current in
                    current.map { item in
                        guard item.id == capture.id else { return item }
                        var failed = item
                        failed.lastError = message
                        return failed
                    }
                }
                break
            }
        }
        return sent
    }

    public func remove(id: UUID) async throws {
        captures = try await store.mutate { current in current.filter { $0.id != id } }
    }
}
