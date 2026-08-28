import BoopmarkShared
import SwiftUI

struct BookmarkDetailView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.openURL) private var openURL
    @Environment(\.dismiss) private var dismiss
    let bookmark: Bookmark
    @State private var title: String
    @State private var note: String
    @State private var tags: String
    @State private var isEditing = false
    @State private var isSuggesting = false
    @State private var autofillMessage: String?
    @State private var showingDeleteConfirmation = false

    init(bookmark: Bookmark) {
        self.bookmark = bookmark
        _title = State(initialValue: bookmark.title ?? "")
        _note = State(initialValue: bookmark.description ?? "")
        _tags = State(initialValue: bookmark.tags.joined(separator: ", "))
    }

    private var currentBookmark: Bookmark {
        model.bookmarks.first(where: { $0.id == bookmark.id }) ?? bookmark
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                if let imageURL = currentBookmark.effectiveImageURL {
                    AsyncImage(url: imageURL) { phase in
                        if let image = phase.image { image.resizable().scaledToFill() }
                        else { Color.clear }
                    }
                    .frame(maxWidth: .infinity)
                    .frame(height: 190)
                    .clipped()
                    .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
                }
                VStack(alignment: .leading, spacing: 8) {
                    if isEditing {
                        TextField("Title", text: $title, axis: .vertical)
                            .font(.title2.weight(.bold))
                            .accessibilityIdentifier("bookmark.edit.title")
                        TextField("Note", text: $note, axis: .vertical)
                            .lineLimit(3...8)
                            .accessibilityIdentifier("bookmark.edit.note")
                        TextField("Tags", text: $tags)
                            .accessibilityIdentifier("bookmark.edit.tags")
                        Button {
                            Task { await autofill() }
                        } label: {
                            HStack {
                                if isSuggesting { ProgressView() }
                                Label("Autofill with AI", systemImage: "sparkles")
                            }
                        }
                        .accessibilityIdentifier("bookmark.edit.autofill")
                        .disabled(isSuggesting)
                        if let autofillMessage {
                            Label(autofillMessage, systemImage: "checkmark.circle.fill")
                                .font(.footnote)
                                .foregroundStyle(BoopmarkTheme.muted)
                                .accessibilityIdentifier("bookmark.edit.autofillResult")
                        }
                    } else {
                        Text(currentBookmark.displayTitle).font(.title2.weight(.bold))
                        if let description = currentBookmark.description, !description.isEmpty {
                            Text(description).foregroundStyle(BoopmarkTheme.muted)
                        }
                        if !currentBookmark.tags.isEmpty {
                            ScrollView(.horizontal, showsIndicators: false) {
                                HStack { ForEach(currentBookmark.tags, id: \.self) { TagPill(tag: $0) } }
                            }
                        }
                    }
                    Text(currentBookmark.url.absoluteString)
                        .font(.footnote)
                        .foregroundStyle(BoopmarkTheme.primary)
                        .lineLimit(3)
                }
                .boopmarkCard()

                Button { openURL(currentBookmark.url) } label: {
                    Label("Open original", systemImage: "arrow.up.right.square")
                        .frame(maxWidth: .infinity)
                }
                .buttonStyle(.borderedProminent)
                .tint(BoopmarkTheme.primary)
            }
            .padding(16)
        }
        .background(BoopmarkTheme.background)
        .navigationTitle("Bookmark")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    Button(isEditing ? "Cancel editing" : "Edit") { isEditing.toggle() }
                    if isEditing {
                        Button("Save changes") { Task { await save() } }
                        Button {
                            Task { await autofill() }
                        } label: {
                            Label("Autofill with AI", systemImage: "sparkles")
                        }
                        .disabled(isSuggesting)
                    }
                    Button("Delete", role: .destructive) {
                        showingDeleteConfirmation = true
                    }
                } label: { Image(systemName: "ellipsis.circle") }
                .accessibilityIdentifier("bookmark.detailMenu")
            }
        }
        .confirmationDialog(
            "Delete this bookmark?",
            isPresented: $showingDeleteConfirmation,
            titleVisibility: .visible
        ) {
            Button("Delete bookmark", role: .destructive) {
                Task {
                    if await model.delete(bookmark: currentBookmark) { dismiss() }
                }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("This permanently removes it from Boopmark.")
        }
    }

    private func save() async {
        let saved = await model.update(
            bookmark: currentBookmark,
            title: title,
            note: note,
            tags: tags.split(separator: ",").map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }.filter { !$0.isEmpty }
        )
        if saved { isEditing = false }
    }

    private func autofill() async {
        isSuggesting = true
        defer { isSuggesting = false }
        guard let suggestion = await model.suggest(url: currentBookmark.url) else { return }
        // Match the web edit flow: an explicit suggestion refresh replaces
        // editable fields, falling back to current values when unavailable.
        title = suggestion.title ?? title
        note = suggestion.description ?? note
        if !suggestion.tags.isEmpty { tags = suggestion.tags.joined(separator: ", ") }
        autofillMessage = [title, note, tags].allSatisfy {
            !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        } ? "AI filled title, note, and tags." : "AI filled every available suggestion."
    }
}
