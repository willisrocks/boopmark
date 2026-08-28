import BoopmarkShared
import SwiftUI

@main
struct BoopmarkApp: App {
    @StateObject private var model: AppModel

    init() {
#if DEBUG
        if ProcessInfo.processInfo.arguments.contains("--appstore-screenshots") {
            let screenshotModel = AppModel(
                settingsStore: UITestSettingsStore(),
                queue: CaptureQueue(store: UITestCaptureQueueStore())
            )
            screenshotModel.loadAppStoreScreenshotFixtures()
            _model = StateObject(wrappedValue: screenshotModel)
            return
        }
#endif
        if ProcessInfo.processInfo.arguments.contains("--uitesting") {
            let queue: CaptureQueue
            if ProcessInfo.processInfo.arguments.contains("--share-e2e"),
               let appGroup = FileManager.default.containerURL(
                   forSecurityApplicationGroupIdentifier: "group.com.boopmark.shared"
               ) {
                queue = CaptureQueue(
                    store: FileCaptureQueueStore(
                        fileURL: appGroup.appendingPathComponent("pending-captures.json")
                    )
                )
            } else {
                queue = CaptureQueue(store: UITestCaptureQueueStore())
            }
            _model = StateObject(wrappedValue: AppModel(settingsStore: UITestSettingsStore(), queue: queue))
        } else {
            _model = StateObject(wrappedValue: AppModel())
        }
    }

    var body: some Scene {
        WindowGroup {
            RootView()
                .environmentObject(model)
                .preferredColorScheme(.dark)
        }
    }
}

private final class UITestSettingsStore: @unchecked Sendable, SettingsStore {
    private var settings = BoopmarkSettings()
    func load() throws -> BoopmarkSettings { settings }
    func saveServerURL(_ url: URL?) throws { settings.serverURL = url }
    func saveAPIKey(_ key: String?) throws { settings.apiKey = key }
}

private actor UITestCaptureQueueStore: CaptureQueueStore {
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
