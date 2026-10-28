import SwiftUI

/// One note: read it with its links and backlinks, or edit its text. Edits are saved a moment
/// after typing stops, and when the screen is left.
struct NoteView: View {
    let noteID: String
    @Binding var path: [String]
    @EnvironmentObject private var library: Library
    @Environment(\.palette) private var palette
    @State private var detail: NoteDetail?
    @State private var draft = ""
    @State private var saved = ""
    @State private var editing = false
    @State private var message: String?
    @State private var pending: Task<Void, Never>?

    var body: some View {
        Group {
            if detail == nil && message == nil {
                ProgressView()
            } else if editing {
                TextEditor(text: $draft)
                    .font(.body)
                    .foregroundStyle(palette.fgPrimary)
                    .scrollContentBackground(.hidden)
                    .padding(.horizontal, 12)
            } else {
                ScrollView {
                    VStack(alignment: .leading, spacing: 16) {
                        MarkdownView(text: draft, links: detail?.links ?? []) { id in
                            path.append(id)
                        }
                        if let backlinks = detail?.backlinks, !backlinks.isEmpty {
                            Divider()
                            Text("Linked from").font(.headline).foregroundStyle(palette.fgPrimary)
                            ForEach(backlinks) { note in
                                Button(note.title) { path.append(note.id) }
                            }
                        }
                    }
                    .padding()
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
        }
        .background(palette.canvas)
        .toolbarBackground(palette.sidebar, for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
        .navigationTitle(detail?.summary.title ?? "Note")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button(editing ? "Done" : "Edit") {
                    Task { await toggle() }
                }
                .disabled(detail == nil)
            }
        }
        .overlay(alignment: .bottom) {
            if let message {
                Text(message)
                    .font(.footnote)
                    .padding(10)
                    .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 10))
                    .padding()
            }
        }
        .task { await load() }
        .onChange(of: draft) { _, _ in scheduleSave() }
        .onDisappear { Task { await save() } }
    }

    private func load() async {
        do {
            let found = try await library.run { try $0.readNote(id: noteID) }
            detail = found
            draft = found.body
            saved = found.body
            message = nil
        } catch {
            message = library.describe(error)
        }
    }

    private func toggle() async {
        if editing {
            await save()
            editing = false
            await load()
        } else {
            editing = true
        }
    }

    private func scheduleSave() {
        pending?.cancel()
        guard draft != saved else { return }
        pending = Task {
            try? await Task.sleep(nanoseconds: 1_500_000_000)
            if Task.isCancelled { return }
            await save()
        }
    }

    private func save() async {
        pending?.cancel()
        guard draft != saved else { return }
        let body = draft
        do {
            let summary = try await library.run { try $0.saveNote(id: noteID, body: body) }
            saved = body
            library.replace(summary)
            message = nil
        } catch {
            message = library.describe(error)
        }
    }
}
