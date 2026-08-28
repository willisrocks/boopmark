import BoopmarkShared
import Combine
import SwiftUI
import UIKit
import UniformTypeIdentifiers

final class ShareViewController: UIViewController {
    private let viewModel = ShareCaptureViewModel()
    private var hostingController: UIViewController?

    override func viewDidLoad() {
        super.viewDidLoad()
        let host = UIHostingController(rootView: ShareCaptureView().environmentObject(viewModel))
        hostingController = host
        addChild(host)
        view.addSubview(host.view)
        host.view.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            host.view.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            host.view.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            host.view.topAnchor.constraint(equalTo: view.topAnchor),
            host.view.bottomAnchor.constraint(equalTo: view.bottomAnchor)
        ])
        host.didMove(toParent: self)
        Task { await viewModel.readSharedContent(from: extensionContext) }
    }

    override func viewDidDisappear(_ animated: Bool) {
        super.viewDidDisappear(animated)
        if isBeingDismissed { extensionContext?.cancelRequest(withError: CancellationError()) }
    }
}

@MainActor
final class ShareCaptureViewModel: ObservableObject {
    @Published var url: URL?
    @Published var title = ""
    @Published var note = ""
    @Published var tags = ""
    @Published var isLoading = true
    @Published var isSuggesting = false
    @Published var isSaving = false
    @Published var suggestionMessage: String?
    @Published var errorMessage: String?
    @Published var successMessage: String?

    private let settingsStore = UserDefaultsSettingsStore()
    private lazy var queue = CaptureQueue(store: Self.queueStore())
    private weak var context: NSExtensionContext?

    func readSharedContent(from context: NSExtensionContext?) async {
        self.context = context
        guard let context else {
            errorMessage = "Boopmark could not read the shared item."
            isLoading = false
            return
        }
        let items = (context.inputItems as? [NSExtensionItem]) ?? []
        for item in items {
            for provider in item.attachments ?? [] {
                if provider.hasItemConformingToTypeIdentifier(UTType.url.identifier),
                   let sharedURL = await loadURL(provider),
                   let normalizedURL = try? BookmarkURLValidator.validate(sharedURL.absoluteString) {
                    url = normalizedURL
                    isLoading = false
                    await autofill()
                    return
                }
                if provider.hasItemConformingToTypeIdentifier(UTType.plainText.identifier),
                   let sharedText = await loadText(provider),
                   let normalizedURL = try? BookmarkURLValidator.validate(sharedText) {
                    url = normalizedURL
                    isLoading = false
                    await autofill()
                    return
                }
            }
        }
        errorMessage = "Share a web link to save it in Boopmark."
        isLoading = false
    }

    func autofill() async {
        guard !isSuggesting, let url else { return }
        isSuggesting = true
        suggestionMessage = nil
        defer { isSuggesting = false }
        do {
            let settings = try settingsStore.load()
            guard let serverURL = settings.serverURL,
                  let apiKey = settings.apiKey,
                  !apiKey.isEmpty else {
                suggestionMessage = "Connect Boopmark in the app to enable AI autofill."
                return
            }
            let suggestion = try await BoopmarkAPI(baseURL: serverURL, token: apiKey).suggest(url: url)

            // The request runs while the form remains editable. Only fill a
            // field if the user has not typed something before it completes.
            if title.nilIfBlank == nil { title = suggestion.title ?? "" }
            if note.nilIfBlank == nil { note = suggestion.description ?? "" }
            if tags.nilIfBlank == nil { tags = suggestion.tags.joined(separator: ", ") }

            suggestionMessage = [title, note, tags].allSatisfy { $0.nilIfBlank != nil }
                ? "AI filled title, note, and tags."
                : "AI filled every available suggestion."
        } catch {
            // Enrichment is optional. Keep Save available and let the user
            // retry without turning a metadata failure into a blocking alert.
            suggestionMessage = "AI autofill is unavailable. Retry, or save without it."
        }
    }

    func save() async {
        guard let url else { errorMessage = "No web link was shared."; return }
        isSaving = true
        defer { isSaving = false }
        let capture = PendingCapture(
            url: url,
            title: title.nilIfBlank,
            note: note.nilIfBlank,
            tags: tags.split(separator: ",").map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }.filter { !$0.isEmpty }
        )
        do {
            let settings = try settingsStore.load()
            if let serverURL = settings.serverURL, let apiKey = settings.apiKey, !apiKey.isEmpty {
                let api = BoopmarkAPI(baseURL: serverURL, token: apiKey)
                _ = try await api.create(capture.request, suggest: true)
                successMessage = "Saved to Boopmark"
            } else {
                try await queue.enqueue(capture)
                successMessage = "Saved on this device"
            }
            finishAfterSuccess()
        } catch let error as BoopmarkAPIError where error.isRetryableOffline {
            do {
                try await queue.enqueue(capture)
                successMessage = "Saved offline — send it from the Boopmark app"
                finishAfterSuccess()
            } catch {
                errorMessage = error.localizedDescription
            }
        } catch { errorMessage = error.localizedDescription }
    }

    func cancel() { context?.cancelRequest(withError: CancellationError()) }

    private func finishAfterSuccess() {
        Task { @MainActor in
            try? await Task.sleep(nanoseconds: 500_000_000)
            context?.completeRequest(returningItems: nil)
        }
    }

    private func loadURL(_ provider: NSItemProvider) async -> URL? {
        await withCheckedContinuation { continuation in
            provider.loadItem(forTypeIdentifier: UTType.url.identifier, options: nil) { item, _ in
                if let item = item as? URL { continuation.resume(returning: item); return }
                if let string = item as? String { continuation.resume(returning: URL(string: string)); return }
                continuation.resume(returning: nil)
            }
        }
    }

    private func loadText(_ provider: NSItemProvider) async -> String? {
        await withCheckedContinuation { continuation in
            provider.loadItem(forTypeIdentifier: UTType.plainText.identifier, options: nil) { item, _ in
                if let item = item as? String { continuation.resume(returning: item); return }
                if let data = item as? Data { continuation.resume(returning: String(data: data, encoding: .utf8)); return }
                continuation.resume(returning: nil)
            }
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

struct ShareCaptureView: View {
    @EnvironmentObject private var model: ShareCaptureViewModel

    var body: some View {
        NavigationStack {
            ZStack {
                BoopmarkTheme.background.ignoresSafeArea()
                if model.isLoading {
                    ProgressView("Reading link…")
                } else if let successMessage = model.successMessage {
                    VStack(spacing: 14) {
                        Image(systemName: "checkmark.circle.fill")
                            .font(.system(size: 48))
                            .foregroundStyle(BoopmarkTheme.mustard)
                        Text(successMessage).font(.headline)
                    }
                } else {
                    form
                }
            }
            .navigationTitle("Save to Boopmark")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { model.cancel() }
                }
            }
            .alert("Boopmark", isPresented: errorBinding) {
                Button("OK", role: .cancel) { model.errorMessage = nil }
            } message: { Text(model.errorMessage ?? "") }
        }
        .tint(BoopmarkTheme.primary)
        .preferredColorScheme(.dark)
    }

    private var form: some View {
        Form {
            Section("Link") {
                Text(model.url?.absoluteString ?? "Unknown link")
                    .font(.footnote)
                    .foregroundStyle(BoopmarkTheme.muted)
                    .lineLimit(3)
                Button {
                    Task { await model.autofill() }
                } label: {
                    HStack {
                        if model.isSuggesting { ProgressView() }
                        Label(
                            model.isSuggesting ? "Generating title, note, and tags…" : "Autofill with AI",
                            systemImage: "sparkles"
                        )
                    }
                }
                .accessibilityIdentifier("share.autofill")
                .disabled(model.isSuggesting || model.isSaving)
                if let suggestionMessage = model.suggestionMessage {
                    Label(suggestionMessage, systemImage: "sparkles")
                        .font(.footnote)
                        .foregroundStyle(BoopmarkTheme.muted)
                        .accessibilityIdentifier("share.autofillStatus")
                }
                TextField(
                    "",
                    text: $model.title,
                    prompt: Text("Title (optional)").foregroundColor(BoopmarkTheme.muted)
                )
                .foregroundStyle(.white)
                .accessibilityIdentifier("share.title")
                TextField(
                    "",
                    text: $model.note,
                    prompt: Text("Note (optional)").foregroundColor(BoopmarkTheme.muted),
                    axis: .vertical
                )
                .foregroundStyle(.white)
                .lineLimit(2...4)
                .accessibilityIdentifier("share.note")
                TextField(
                    "",
                    text: $model.tags,
                    prompt: Text("Tags, separated by commas").foregroundColor(BoopmarkTheme.muted)
                )
                .foregroundStyle(.white)
                .accessibilityIdentifier("share.tags")
            }
            .listRowBackground(BoopmarkTheme.surface)
            .foregroundStyle(.white)
            Section {
                Button {
                    Task { await model.save() }
                } label: {
                    HStack {
                        Spacer()
                        if model.isSaving { ProgressView() }
                        else { Label("Save bookmark", systemImage: "bookmark.fill").fontWeight(.semibold) }
                        Spacer()
                    }
                }
                .accessibilityIdentifier("share.save")
                .disabled(model.isSaving || model.url == nil)
            }
            .listRowBackground(BoopmarkTheme.surface)
        }
        .scrollContentBackground(.hidden)
        .background(BoopmarkTheme.background)
    }

    private var errorBinding: Binding<Bool> {
        Binding(
            get: { model.errorMessage != nil },
            set: { if !$0 { model.errorMessage = nil } }
        )
    }
}

private extension String {
    var nilIfBlank: String? {
        let trimmed = trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}
