import BoopmarkShared
import SwiftUI

struct SettingsView: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.dismiss) private var dismiss
    @State private var serverURL: String
    @State private var apiKey: String
    @State private var isSaving = false

    init() {
        _serverURL = State(initialValue: "")
        _apiKey = State(initialValue: "")
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Text("Connect the app with an API key from Boopmark → Settings → API keys.")
                        .font(.footnote)
                        .foregroundStyle(BoopmarkTheme.muted)
                    if let notice = model.noticeMessage {
                        Label(notice, systemImage: "checkmark.circle.fill")
                            .font(.footnote)
                            .foregroundStyle(BoopmarkTheme.primary)
                    }
                }
                Section("Server") {
                    TextField("https://boopmark.example", text: $serverURL)
                        .accessibilityIdentifier("settings.serverURL")
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .keyboardType(.URL)
                    SecureField("API key", text: $apiKey)
                        .accessibilityIdentifier("settings.apiKey")
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                }
                Section {
                    Button {
                        save()
                    } label: {
                        HStack { Spacer(); if isSaving { ProgressView() } else { Text("Save connection").fontWeight(.semibold) }; Spacer() }
                    }
                    .disabled(isSaving || serverURL.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || apiKey.isEmpty)
                }
                Section("Offline captures") {
                    HStack {
                        Label("Waiting to send", systemImage: "tray.full")
                        Spacer()
                        Text("\(model.pendingCaptures.count)")
                            .foregroundStyle(BoopmarkTheme.muted)
                            .accessibilityIdentifier("settings.pendingCount")
                    }
                    if !model.pendingCaptures.isEmpty {
                        Button {
                            Task { await model.sendQueuedCaptures() }
                        } label: {
                            Label("Send queued captures", systemImage: "arrow.up.circle")
                        }
                        ForEach(model.pendingCaptures) { capture in
                            VStack(alignment: .leading, spacing: 4) {
                                Text(capture.title ?? capture.url.host ?? capture.url.absoluteString)
                                    .font(.subheadline)
                                    .lineLimit(1)
                                if let error = capture.lastError {
                                    Text(error)
                                        .font(.caption)
                                        .foregroundStyle(.red)
                                }
                                Button("Remove from queue", role: .destructive) {
                                    Task { await model.removeQueuedCapture(id: capture.id) }
                                }
                                .font(.caption)
                            }
                            .accessibilityIdentifier("settings.pending.\(capture.url.absoluteString)")
                        }
                    }
                    Text("Queued captures are sent only when you tap this button, so an offline retry can never silently create duplicates.")
                        .font(.caption)
                        .foregroundStyle(BoopmarkTheme.muted)
                }
            }
            .scrollContentBackground(.hidden)
            .background(BoopmarkTheme.background)
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Done") { dismiss() } } }
            .task {
                serverURL = model.settings.serverURL?.absoluteString ?? ""
                apiKey = model.settings.apiKey ?? ""
                await model.refreshPendingCaptures()
            }
        }
        .tint(BoopmarkTheme.primary)
        .alert("Boopmark", isPresented: errorBinding) {
            Button("OK", role: .cancel) { model.errorMessage = nil }
        } message: { Text(model.errorMessage ?? "") }
    }

    private func save() {
        isSaving = true
        Task { @MainActor in
            defer { isSaving = false }
            do {
                try await model.configure(serverURLText: serverURL, apiKey: apiKey)
                dismiss()
            } catch { model.errorMessage = error.localizedDescription }
        }
    }

    private var errorBinding: Binding<Bool> {
        Binding(
            get: { model.errorMessage != nil },
            set: { if !$0 { model.errorMessage = nil } }
        )
    }
}
