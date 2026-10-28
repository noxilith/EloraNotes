import SwiftUI

/// A folder and the notes in it.
struct FolderGroup: Identifiable {
    let name: String
    let notes: [NoteSummary]
    var id: String { name }
}

struct NoteListView: View {
    @EnvironmentObject private var library: Library
    @Environment(\.palette) private var palette
    @AppStorage("showPreviews") private var showPreviews = false
    @State private var query = ""
    @State private var results: [NoteSummary] = []
    @State private var showSettings = false
    @State private var showNew = false
    @State private var showGraph = false
    @State private var path: [String] = []

    private var searching: Bool {
        !query.trimmingCharacters(in: .whitespaces).isEmpty
    }

    /// Notes with no folder, and the folders with their notes, A to Z.
    private func split(_ notes: [NoteSummary]) -> (unfiled: [NoteSummary], folders: [FolderGroup]) {
        var unfiled: [NoteSummary] = []
        var byFolder: [String: [NoteSummary]] = [:]
        for note in notes {
            if let folder = note.folder, !folder.isEmpty {
                byFolder[folder, default: []].append(note)
            } else {
                unfiled.append(note)
            }
        }
        let names = byFolder.keys.sorted { $0.localizedCaseInsensitiveCompare($1) == .orderedAscending }
        return (unfiled, names.map { FolderGroup(name: $0, notes: byFolder[$0] ?? []) })
    }

    var body: some View {
        NavigationStack(path: $path) {
            List {
                if let why = library.sharedError {
                    Label(why, systemImage: "exclamationmark.triangle")
                        .font(.footnote)
                        .foregroundStyle(palette.fgSecondary)
                        .listRowBackground(palette.canvas)
                }
                if searching {
                    rows(results, indent: 0)
                } else {
                    groups
                }
            }
            .listStyle(.plain)
            .scrollContentBackground(.hidden)
            .background(palette.canvas)
            .listRowSeparatorTint(palette.border)
            .navigationTitle("EloraNotes")
            .toolbarBackground(palette.sidebar, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
            .navigationDestination(for: String.self) { id in
                NoteView(noteID: id, path: $path)
            }
            .searchable(text: $query, prompt: "Search notes")
            .task(id: query) {
                let text = query.trimmingCharacters(in: .whitespaces)
                guard !text.isEmpty else {
                    results = []
                    return
                }
                try? await Task.sleep(nanoseconds: 250_000_000)
                if Task.isCancelled { return }
                results = await library.search(text)
            }
            .refreshable { await library.refresh() }
            .toolbar {
                ToolbarItemGroup(placement: .topBarLeading) {
                    Button {
                        showSettings = true
                    } label: {
                        Image(systemName: "gearshape")
                    }
                    .accessibilityLabel("Settings")
                    if let kofi = URL(string: "https://ko-fi.com/noxilith") {
                        Link(destination: kofi) {
                            KofiMark()
                        }
                        .accessibilityLabel("Support EloraNotes on Ko-fi")
                    }
                }
                ToolbarItemGroup(placement: .topBarTrailing) {
                    Button {
                        showGraph = true
                    } label: {
                        Image(systemName: "point.3.connected.trianglepath.dotted")
                    }
                    .accessibilityLabel("Graph")
                    Button {
                        showNew = true
                    } label: {
                        Image(systemName: "square.and.pencil")
                    }
                    .accessibilityLabel("New note")
                }
            }
            .navigationDestination(isPresented: $showGraph) {
                GraphScreen(path: $path)
            }
            .onChange(of: library.launchRoute) { _, route in
                switch route {
                case "graph": showGraph = true
                case "settings": showSettings = true
                case "search": query = "slime"
                case "note":
                    if let note = library.notes.first(where: { $0.title == "Slime Combat" }) {
                        path.append(note.id)
                    }
                default: break
                }
            }
            .sheet(isPresented: $showSettings) {
                SettingsView()
            }
            .sheet(isPresented: $showNew) {
                NewNoteView { id in path.append(id) }
            }
            .overlay {
                if library.loading && library.notes.isEmpty {
                    ProgressView()
                }
            }
            .alert(
                "Something went wrong",
                isPresented: Binding(
                    get: { library.problem != nil },
                    set: { if !$0 { library.problem = nil } })
            ) {
                Button("OK", role: .cancel) {}
            } message: {
                Text(library.problem ?? "")
            }
        }
    }

    /// Pinned notes, then the shared library (a folder with a cloud), then this phone's folders
    /// and the notes with no folder: the same order as the desktop sidebar.
    @ViewBuilder
    private var groups: some View {
        let pinned = library.notes.filter { $0.pinned }
        let remote = split(library.notes.filter { $0.shared && !$0.pinned })
        let local = split(library.notes.filter { !$0.shared && !$0.pinned })

        if !pinned.isEmpty {
            header(key: nil, icon: "pin.fill", title: "Pinned", count: pinned.count, indent: 0, tint: palette.pin)
            rows(pinned, indent: 0)
        }

        if !(remote.unfiled.isEmpty && remote.folders.isEmpty) {
            let count = remote.unfiled.count + remote.folders.reduce(0) { $0 + $1.notes.count }
            let key = "server"
            header(key: key, icon: "cloud", title: library.serverName ?? "Shared library", count: count, indent: 0, tint: palette.accent)
            if !library.collapsed.contains(key) {
                rows(remote.unfiled, indent: 1)
                ForEach(remote.folders) { folder in
                    let folderKey = "server/\(folder.name)"
                    header(key: folderKey, icon: "folder", title: folder.name, count: folder.notes.count, indent: 1, tint: palette.fgSecondary)
                    if !library.collapsed.contains(folderKey) {
                        rows(folder.notes, indent: 2)
                    }
                }
            }
        }

        ForEach(local.folders) { folder in
            let key = "local/\(folder.name)"
            header(key: key, icon: "folder", title: folder.name, count: folder.notes.count, indent: 0, tint: palette.fgSecondary)
            if !library.collapsed.contains(key) {
                rows(folder.notes, indent: 1)
            }
        }
        rows(local.unfiled, indent: 0)
    }

    private func insets(_ indent: Int) -> EdgeInsets {
        EdgeInsets(top: 6, leading: 16 + CGFloat(indent) * 18, bottom: 6, trailing: 16)
    }

    private func header(key: String?, icon: String, title: String, count: Int, indent: Int, tint: Color) -> some View {
        Button {
            if let key { library.toggleFolder(key) }
        } label: {
            HStack(spacing: 8) {
                if let key {
                    Image(systemName: library.collapsed.contains(key) ? "chevron.right" : "chevron.down")
                        .font(.caption2.weight(.bold))
                        .foregroundStyle(palette.fgMuted)
                        .frame(width: 12)
                }
                Image(systemName: icon).foregroundStyle(tint)
                Text(title).font(.subheadline.weight(.semibold)).foregroundStyle(palette.fgPrimary)
                Spacer()
                Text("\(count)").font(.footnote).foregroundStyle(palette.fgMuted)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .listRowInsets(EdgeInsets(top: 8, leading: 16 + CGFloat(indent) * 18, bottom: 8, trailing: 16))
        .listRowBackground(palette.sidebar)
    }

    @ViewBuilder
    private func rows(_ notes: [NoteSummary], indent: Int) -> some View {
        ForEach(notes) { note in
            NavigationLink(value: note.id) {
                NoteRow(note: note, showPreview: showPreviews)
            }
            .listRowInsets(insets(indent))
            .listRowBackground(palette.canvas)
        }
        .onDelete { offsets in
            let ids = offsets.map { notes[$0].id }
            Task { await library.trash(ids) }
        }
    }
}

extension NoteSummary {
    /// The first lines of the note with the Markdown marks taken off.
    var preview: String {
        snippet
            .replacingOccurrences(of: "[[", with: "")
            .replacingOccurrences(of: "]]", with: "")
            .replacingOccurrences(of: "`", with: "")
            .replacingOccurrences(of: "**", with: "")
            .replacingOccurrences(of: "#", with: "")
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }
}

/// A note in the list: its name and tags, and the preview line when that is turned on.
struct NoteRow: View {
    let note: NoteSummary
    let showPreview: Bool
    @Environment(\.palette) private var palette

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(note.title)
                .font(.body.weight(.medium))
                .foregroundStyle(palette.fgPrimary)
                .lineLimit(1)
            if !note.tags.isEmpty {
                Text(note.tags.map { "#\($0)" }.joined(separator: "  "))
                    .font(.caption)
                    .foregroundStyle(palette.fgMuted)
                    .lineLimit(1)
            }
            if showPreview, !note.preview.isEmpty {
                Text(note.preview)
                    .font(.footnote)
                    .foregroundStyle(palette.fgSecondary)
                    .lineLimit(2)
            }
        }
    }
}
