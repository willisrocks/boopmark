import BoopmarkShared
import SwiftUI

struct RootView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.scenePhase) private var scenePhase
    @State private var search = ""
    @State private var tags = ""
    @State private var sort: BookmarkSort = .newest
    @State private var showingCapture = false
    @State private var showingSettings = false
    @State private var showingFilters = false

    var body: some View {
        NavigationStack {
            ZStack {
                BoopmarkTheme.background.ignoresSafeArea()
                content
            }
            .navigationTitle("Boopmark")
            .toolbar {
                ToolbarItem(placement: .topBarLeading) { BrandMark(size: 28) }
                ToolbarItem(placement: .topBarTrailing) {
                    HStack(spacing: 16) {
                        Menu {
                            Picker("Sort bookmarks", selection: $sort) {
                                ForEach(BookmarkSort.allCases, id: \.self) { option in
                                    Text(option.title).tag(option)
                                }
                            }
                            Button {
                                showingFilters = true
                            } label: {
                                Label("Filter by tags", systemImage: "tag")
                            }
                            if !selectedTags.isEmpty {
                                Button("Clear tag filters", role: .destructive) {
                                    tags = ""
                                    Task { await model.refresh(query: query) }
                                }
                            }
                        } label: {
                            Image(systemName: selectedTags.isEmpty && sort == .newest
                                  ? "line.3.horizontal.decrease.circle"
                                  : "line.3.horizontal.decrease.circle.fill")
                        }
                        .accessibilityLabel("Sort and filter")
                        .accessibilityIdentifier("bookmarks.filters")
                        Button { showingSettings = true } label: { Image(systemName: "gearshape") }
                            .accessibilityLabel("Settings")
                            .accessibilityIdentifier("settings.button")
                        Button { showingCapture = true } label: { Image(systemName: "plus") }
                            .accessibilityLabel("Add bookmark")
                            .accessibilityIdentifier("capture.toolbarButton")
                    }
                }
            }
            .searchable(text: $search, prompt: "Search bookmarks")
            .onSubmit(of: .search) { Task { await model.refresh(query: query) } }
            .task { await model.syncFromServer(query: query) }
            .onChange(of: scenePhase) { phase in
                guard phase == .active else { return }
                Task { await model.syncFromServer(query: query) }
            }
            .onChange(of: sort) { _ in Task { await model.refresh(query: query) } }
            .refreshable { await model.refresh(query: query) }
            .sheet(isPresented: $showingCapture, onDismiss: refreshAfterSheetDismissal) { CaptureView() }
            .sheet(isPresented: $showingSettings, onDismiss: refreshAfterSheetDismissal) { SettingsView() }
            .sheet(isPresented: $showingFilters) {
                BookmarkFiltersView(tags: $tags, sort: $sort) {
                    Task { await model.refresh(query: query) }
                }
            }
            .alert("Boopmark", isPresented: alertBinding) {
                Button("OK", role: .cancel) { model.errorMessage = nil }
            } message: { Text(model.errorMessage ?? "") }
            .overlay(alignment: .top) {
                if let notice = model.noticeMessage {
                    Button {
                        model.noticeMessage = nil
                    } label: {
                        Label(notice, systemImage: "checkmark.circle.fill")
                            .font(.subheadline.weight(.medium))
                            .foregroundStyle(.white)
                            .padding(.horizontal, 14)
                            .padding(.vertical, 10)
                            .background(BoopmarkTheme.surfaceRaised, in: Capsule())
                            .shadow(radius: 8)
                    }
                    .buttonStyle(.plain)
                    .padding(.top, 8)
                    .accessibilityHint("Dismiss notification")
                }
            }
        }
        .tint(BoopmarkTheme.primary)
    }

    private var selectedTags: [String] {
        tags.split(separator: ",")
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
    }

    private var query: BookmarkQuery {
        BookmarkQuery(search: search.nilIfBlank, tags: selectedTags, sort: sort)
    }

    private func refreshAfterSheetDismissal() {
        Task { await model.syncFromServer(query: query) }
    }

    @ViewBuilder private var content: some View {
        if model.isLoading && model.bookmarks.isEmpty {
            ProgressView("Loading bookmarks…")
        } else if model.bookmarks.isEmpty {
            VStack(spacing: 14) {
                Image(systemName: "bookmark")
                    .font(.system(size: 36))
                    .foregroundStyle(BoopmarkTheme.primary)
                Text("No bookmarks yet").font(.headline)
                Text(model.isConfigured ? "Save something from the Share Sheet to get started." : "Connect your server in Settings to get started.")
                    .font(.subheadline)
                    .foregroundStyle(BoopmarkTheme.muted)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
                    .padding(.horizontal, 8)
                Button("Add bookmark") { showingCapture = true }
                    .buttonStyle(.borderedProminent)
                    .tint(BoopmarkTheme.primary)
                    .accessibilityIdentifier("capture.emptyStateButton")
            }
            .padding(32)
        } else {
            ScrollView {
                LazyVStack(spacing: 12) {
                    ForEach(model.bookmarks) { bookmark in
                        NavigationLink(value: bookmark) { BookmarkRow(bookmark: bookmark) }
                            .buttonStyle(.plain)
                            .accessibilityIdentifier("bookmark.row.\(bookmark.id.uuidString)")
                    }
                }
                .padding(.horizontal, 16)
                .padding(.vertical, 12)
            }
            .navigationDestination(for: Bookmark.self) { BookmarkDetailView(bookmark: $0) }
        }
    }

    private var alertBinding: Binding<Bool> {
        Binding(
            get: { model.errorMessage != nil },
            set: { if !$0 { model.errorMessage = nil } }
        )
    }
}

private struct BookmarkFiltersView: View {
    @Environment(\.dismiss) private var dismiss
    @Binding var tags: String
    @Binding var sort: BookmarkSort
    let apply: () -> Void

    var body: some View {
        NavigationStack {
            Form {
                Section("Tags") {
                    TextField("Tags, separated by commas", text: $tags)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .accessibilityIdentifier("bookmarks.filterTags")
                    Text("Multiple tags match bookmarks containing every selected tag.")
                        .font(.footnote)
                        .foregroundStyle(BoopmarkTheme.muted)
                }
                Section("Sort") {
                    Picker("Order", selection: $sort) {
                        ForEach(BookmarkSort.allCases, id: \.self) { option in
                            Text(option.title).tag(option)
                        }
                    }
                    .pickerStyle(.inline)
                    .accessibilityIdentifier("bookmarks.sort")
                }
            }
            .navigationTitle("Sort and filter")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Clear") { tags = ""; sort = .newest }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Apply") { apply(); dismiss() }
                        .accessibilityIdentifier("bookmarks.applyFilters")
                }
            }
        }
        .tint(BoopmarkTheme.primary)
    }
}

private extension BookmarkSort {
    var title: String {
        switch self {
        case .newest: "Newest first"
        case .oldest: "Oldest first"
        case .title: "Title"
        case .domain: "Domain"
        }
    }
}

private struct BookmarkRow: View {
    let bookmark: Bookmark

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            image
            VStack(alignment: .leading, spacing: 6) {
                Text(bookmark.displayTitle)
                    .font(.headline)
                    .foregroundStyle(.primary)
                    .lineLimit(2)
                Text(bookmark.domain ?? bookmark.url.host ?? bookmark.url.absoluteString)
                    .font(.caption)
                    .foregroundStyle(BoopmarkTheme.muted)
                    .lineLimit(1)
                if !bookmark.tags.isEmpty {
                    ScrollView(.horizontal, showsIndicators: false) {
                        HStack(spacing: 5) { ForEach(bookmark.tags.prefix(3), id: \.self) { TagPill(tag: $0) } }
                    }
                }
            }
            Spacer(minLength: 0)
        }
        .boopmarkCard()
    }

    @ViewBuilder private var image: some View {
        if let imageURL = bookmark.effectiveImageURL {
            AsyncImage(url: imageURL) { phase in
                if let image = phase.image { image.resizable().scaledToFill() }
                else { placeholder }
            }
            .frame(width: 72, height: 72)
            .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
        } else { placeholder.frame(width: 72, height: 72) }
    }

    private var placeholder: some View {
        RoundedRectangle(cornerRadius: 10, style: .continuous)
            .fill(BoopmarkTheme.imagePlaceholder)
            .overlay(Image(systemName: "bookmark.fill").foregroundStyle(BoopmarkTheme.primary))
    }
}

private extension String {
    var nilIfBlank: String? {
        let trimmed = trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}
