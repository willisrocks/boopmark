import BoopmarkShared
import SwiftUI

struct CaptureView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.dismiss) private var dismiss
    @State private var urlText: String
    @State private var title: String
    @State private var note: String
    @State private var tags: String
    @State private var isSaving = false
    @State private var isSuggesting = false
    @State private var autofillMessage: String?

    init(initialURL: URL? = nil) {
        _urlText = State(initialValue: initialURL?.absoluteString ?? "")
        _title = State(initialValue: "")
        _note = State(initialValue: "")
        _tags = State(initialValue: "")
    }

    var body: some View {
        NavigationStack {
            Form {
                Section("Link") {
                    TextField("https://…", text: $urlText)
                        .accessibilityIdentifier("capture.url")
                        .textInputAutocapitalization(.never)
                        .keyboardType(.URL)
                        .autocorrectionDisabled()
                    Button {
                        Task { await autofill() }
                    } label: {
                        HStack {
                            if isSuggesting { ProgressView() }
                            Label("Autofill with AI", systemImage: "sparkles")
                        }
                    }
                    .accessibilityIdentifier("capture.autofill")
                    .disabled(isSuggesting || isSaving || (try? BookmarkURLValidator.validate(urlText)) == nil)
                    if let autofillMessage {
                        Label(autofillMessage, systemImage: "checkmark.circle.fill")
                            .font(.footnote)
                            .foregroundStyle(BoopmarkTheme.muted)
                            .accessibilityIdentifier("capture.autofillResult")
                    }
                    TextField("Title (optional)", text: $title)
                        .accessibilityIdentifier("capture.title")
                    TextField("Note (optional)", text: $note, axis: .vertical)
                        .accessibilityIdentifier("capture.note")
                        .lineLimit(2...5)
                    TextField("Tags, separated by commas", text: $tags)
                        .accessibilityIdentifier("capture.tags")
                }
                Section {
                    Button {
                        Task { await save() }
                    } label: {
                        HStack {
                            Spacer()
                            if isSaving { ProgressView() }
                            else { Label("Save bookmark", systemImage: "bookmark.fill").fontWeight(.semibold) }
                            Spacer()
                        }
                    }
                    .accessibilityIdentifier("capture.save")
                    .disabled(isSaving || (try? BookmarkURLValidator.validate(urlText)) == nil)
                }
            }
            .scrollContentBackground(.hidden)
            .background(BoopmarkTheme.background)
            .navigationTitle("Save bookmark")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } } }
        }
        .tint(BoopmarkTheme.primary)
        .alert("Boopmark", isPresented: errorBinding) {
            Button("OK", role: .cancel) { model.errorMessage = nil }
        } message: { Text(model.errorMessage ?? "") }
    }

    private func save() async {
        guard let url = try? BookmarkURLValidator.validate(urlText) else {
            model.errorMessage = "Enter a valid HTTPS link (or a local HTTP link)."
            return
        }
        isSaving = true
        defer { isSaving = false }
        let saved = await model.capture(
            url: url,
            title: title.nilIfBlank,
            note: note.nilIfBlank,
            tags: tags.split(separator: ",").map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }.filter { !$0.isEmpty }
        )
        if saved { dismiss() }
    }

    private func autofill() async {
        guard let url = try? BookmarkURLValidator.validate(urlText) else {
            model.errorMessage = "Enter a valid HTTPS link (or a local HTTP link)."
            return
        }
        isSuggesting = true
        defer { isSuggesting = false }
        guard let suggestion = await model.suggest(url: url) else { return }
        // Match the web add flow: preserve anything the user already typed and
        // fill only missing fields from metadata/LLM enrichment.
        if title.nilIfBlank == nil { title = suggestion.title ?? "" }
        if note.nilIfBlank == nil { note = suggestion.description ?? "" }
        if tags.nilIfBlank == nil { tags = suggestion.tags.joined(separator: ", ") }
        autofillMessage = [title, note, tags].allSatisfy { $0.nilIfBlank != nil }
            ? "AI filled title, note, and tags."
            : "AI filled every available suggestion."
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
