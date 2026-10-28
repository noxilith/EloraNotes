import SwiftUI

/// Pick the folder a note lives in: none, one that is there already, or a new one.
struct FolderSheet: View {
    let note: NoteSummary
    @EnvironmentObject private var library: Library
    @Environment(\.dismiss) private var dismiss
    @Environment(\.palette) private var palette
    @State private var newFolder = ""

    private var folders: [String] { library.folderNames(shared: note.shared) }

    var body: some View {
        NavigationStack {
            List {
                Section {
                    choice(nil, title: "No folder", glyph: Icon.file)
                    ForEach(folders, id: \.self) { name in
                        choice(name, title: name, glyph: Icon.folder)
                    }
                } header: {
                    Text(note.shared ? "Folders in the shared library" : "Folders on this phone")
                }
                .listRowBackground(palette.elevated)

                Section("New folder") {
                    TextField("Folder name", text: $newFolder)
                        .textInputAutocapitalization(.words)
                    Button("Create and move there") {
                        move(to: newFolder.trimmingCharacters(in: .whitespaces))
                    }
                    .disabled(newFolder.trimmingCharacters(in: .whitespaces).isEmpty)
                }
                .listRowBackground(palette.elevated)
            }
            .scrollContentBackground(.hidden)
            .background(palette.canvas)
            .toolbarBackground(palette.sidebar, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
            .navigationTitle("Move “\(note.title)”")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
            }
        }
    }

    private func choice(_ name: String?, title: String, glyph: String) -> some View {
        Button {
            move(to: name)
        } label: {
            HStack(spacing: 12) {
                Glyph(glyph, size: 18, color: palette.fgSecondary).frame(width: 22)
                Text(title).foregroundStyle(palette.fgPrimary)
                Spacer()
                if note.folder == name || (name == nil && (note.folder ?? "").isEmpty) {
                    Image(systemName: "checkmark").foregroundStyle(palette.accent)
                }
            }
        }
    }

    private func move(to folder: String?) {
        Task {
            await library.setFolder(note.id, to: folder)
            dismiss()
        }
    }
}

/// Deleted notes: bring one back, or delete it for good.
struct TrashView: View {
    @EnvironmentObject private var library: Library
    @Environment(\.dismiss) private var dismiss
    @Environment(\.palette) private var palette
    @State private var confirming: NoteSummary?
    @State private var confirmEmpty = false

    var body: some View {
        NavigationStack {
            List {
                Text("Deleted notes wait here until you delete them for good. Tap Restore to bring one back.")
                    .font(AppFont.body(15))
                    .foregroundStyle(palette.fgSecondary)
                    .listRowBackground(Color.clear)
                    .listRowSeparator(.hidden)
                if library.trashed.isEmpty {
                    Text("The trash is empty.")
                        .foregroundStyle(palette.fgMuted)
                        .listRowBackground(Color.clear)
                        .listRowSeparator(.hidden)
                }
                ForEach(library.trashed) { note in
                    HStack(spacing: 12) {
                        Glyph(Icon.named(note.icon), size: 19, color: palette.fgMuted).frame(width: 22)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(note.title).font(AppFont.body(18)).foregroundStyle(palette.fgPrimary).lineLimit(1)
                            if note.shared {
                                Text("In the shared library").font(AppFont.body(13)).foregroundStyle(palette.fgMuted)
                            }
                        }
                        Spacer()
                        Button("Restore") {
                            Task { await library.restore(note.id) }
                        }
                        .buttonStyle(.bordered)
                        .tint(palette.accent)
                    }
                    .listRowBackground(Color.clear)
                    .swipeActions(edge: .trailing) {
                        Button(role: .destructive) {
                            confirming = note
                        } label: {
                            Label("Delete for good", systemImage: "trash")
                        }
                    }
                    .contextMenu {
                        Button {
                            Task { await library.restore(note.id) }
                        } label: {
                            Label("Restore", systemImage: "arrow.uturn.backward")
                        }
                        Button(role: .destructive) {
                            confirming = note
                        } label: {
                            Label("Delete for good", systemImage: "trash")
                        }
                    }
                }
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            .background(palette.sidebar)
            .toolbarBackground(palette.sidebar, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
            .navigationTitle("Trash")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
                if !library.trashed.isEmpty {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("Empty") { confirmEmpty = true }
                    }
                }
            }
            .task { await library.loadTrash() }
            .confirmationDialog(
                "Delete “\(confirming?.title ?? "")” for good?",
                isPresented: Binding(get: { confirming != nil }, set: { if !$0 { confirming = nil } }),
                titleVisibility: .visible
            ) {
                Button("Delete for good", role: .destructive) {
                    if let note = confirming { Task { await library.deleteForever(note.id) } }
                }
            } message: {
                Text(confirming?.shared == true
                    ? "It is in the shared library, so it is deleted for everybody. This cannot be undone."
                    : "This cannot be undone.")
            }
            .confirmationDialog(
                "Delete everything in the trash for good?",
                isPresented: $confirmEmpty, titleVisibility: .visible
            ) {
                Button("Empty the trash", role: .destructive) {
                    Task { for note in library.trashed { await library.deleteForever(note.id) } }
                }
            } message: {
                Text(library.trashed.contains { $0.shared }
                    ? "Notes from the shared library are deleted for everybody. This cannot be undone."
                    : "This cannot be undone.")
            }
        }
    }
}
