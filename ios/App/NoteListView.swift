import SwiftUI

/// A folder and the notes in it.
struct FolderGroup: Identifiable {
    let name: String
    let notes: [NoteSummary]
    var id: String { name }
}

/// The list of notes, laid out like the desktop sidebar: the wordmark and a New button, the
/// search field, then pinned notes, the shared library (a folder with a cloud), this phone's
/// folders and the notes with no folder, and the graph and settings buttons along the bottom.
struct NoteListView: View {
    @EnvironmentObject private var library: Library
    @Environment(\.palette) private var palette
    @AppStorage("showPreviews") private var showPreviews = false
    @State private var query = ""
    @State private var results: [NoteSummary] = []
    @State private var showSettings = false
    @State private var showNew = false
    @State private var showGraph = false
    @State private var showTrash = false
    @State private var showJoin = false
    @State private var moving: NoteSummary?
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
            VStack(spacing: 0) {
                wordmark
                searchField
                List {
                    if let why = library.sharedError {
                        Label(why, systemImage: "exclamationmark.triangle")
                            .font(AppFont.body(13))
                            .foregroundStyle(palette.fgSecondary)
                            .listRowBackground(Color.clear)
                            .listRowSeparator(.hidden)
                    }
                    if searching {
                        rows(results, indent: 0)
                    } else {
                        groups
                    }
                }
                .listStyle(.plain)
                .scrollContentBackground(.hidden)
                .refreshable { await library.refresh() }
                footer
            }
            .background(palette.sidebar)
            .toolbar(.hidden, for: .navigationBar)
            .navigationDestination(for: String.self) { id in
                if id.hasPrefix("joined:") {
                    JoinedNoteView(id: id, path: $path)
                } else {
                    NoteView(noteID: id, path: $path)
                }
            }
            .onChange(of: library.pendingLink) { _, link in
                // A share link the app was opened with joins straight away.
                guard let link else { return }
                library.pendingLink = nil
                Task {
                    if let id = await library.join(link) { path.append(id) }
                }
            }
            .sheet(isPresented: $showJoin) {
                JoinSheet { id in path.append(id) }
            }
            .navigationDestination(isPresented: $showGraph) {
                GraphScreen(path: $path)
            }
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
            .onChange(of: library.launchRoute) { _, route in
                switch route {
                case "graph": showGraph = true
                case "settings": showSettings = true
                case "search": query = "slime"
                case "trash":
                    // Screenshots only: delete a note, then open the trash.
                    Task {
                        if let note = library.notes.first(where: { $0.title == "Slime Combat" }) {
                            await library.trash([note.id])
                        }
                        showTrash = true
                    }
                case "folder":
                    moving = library.notes.first(where: { $0.title == "Slime Combat" })
                case "note", "edit":
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
            .sheet(isPresented: $showTrash) {
                TrashView()
            }
            .sheet(item: $moving) { note in
                FolderSheet(note: note)
            }
            .overlay(alignment: .bottom) {
                if let notice = library.notice {
                    Text(notice)
                        .font(AppFont.body(15))
                        .foregroundStyle(palette.fgPrimary)
                        .multilineTextAlignment(.center)
                        .padding(.horizontal, 16)
                        .padding(.vertical, 10)
                        .background(palette.elevated, in: RoundedRectangle(cornerRadius: 12))
                        .overlay(RoundedRectangle(cornerRadius: 12).stroke(palette.border, lineWidth: 1))
                        .padding(.horizontal, 24)
                        .padding(.bottom, 74)
                        .transition(.opacity)
                }
            }
            .animation(.easeInOut(duration: 0.2), value: library.notice)
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

    // MARK: Header, search and footer

    private var wordmark: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Glyph(Icon.feather, size: 26, color: palette.accent)
                .alignmentGuide(.firstTextBaseline) { $0[VerticalAlignment.center] + 6 }
            Text("elora").font(AppFont.body(32)).foregroundStyle(palette.fgPrimary)
            Text("notes").font(AppFont.body(21)).foregroundStyle(palette.fgMuted)
            Spacer()
            Button {
                showJoin = true
            } label: {
                Image(systemName: "link")
                    .font(.system(size: 18))
                    .foregroundStyle(palette.fgPrimary)
                    .padding(10)
                    .background(palette.elevated, in: RoundedRectangle(cornerRadius: 9))
            }
            .accessibilityLabel("Join a shared note")
            Button {
                showNew = true
            } label: {
                HStack(spacing: 6) {
                    Glyph(Icon.plus, size: 16, color: palette.fgPrimary)
                    Text("New").font(AppFont.body(17)).foregroundStyle(palette.fgPrimary)
                }
                .padding(.horizontal, 14)
                .padding(.vertical, 9)
                .background(palette.accent.opacity(0.55), in: RoundedRectangle(cornerRadius: 9))
            }
            .accessibilityLabel("New note")
        }
        .padding(.horizontal, 18)
        .padding(.top, 10)
        .padding(.bottom, 12)
    }

    private var searchField: some View {
        HStack(spacing: 10) {
            Glyph(Icon.search, size: 17, color: palette.fgMuted)
            TextField(text: $query, prompt: Text("Search notes…").foregroundStyle(palette.fgMuted)) {
                Text("Search")
            }
            .font(AppFont.body(18))
            .foregroundStyle(palette.fgPrimary)
            .autocorrectionDisabled()
            .textInputAutocapitalization(.never)
            if searching {
                Button {
                    query = ""
                } label: {
                    Image(systemName: "xmark.circle.fill").foregroundStyle(palette.fgMuted)
                }
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 12)
        .background(palette.canvas.opacity(0.55), in: RoundedRectangle(cornerRadius: 9))
        .overlay(RoundedRectangle(cornerRadius: 9).stroke(palette.border, lineWidth: 1))
        .padding(.horizontal, 16)
        .padding(.bottom, 8)
    }

    private var footer: some View {
        HStack(spacing: 26) {
            Spacer()
            Button {
                showGraph = true
            } label: {
                Glyph(Icon.graph, size: 22, color: palette.fgSecondary)
            }
            .accessibilityLabel("Graph")
            Button {
                showTrash = true
            } label: {
                Glyph(Icon.named("trash"), size: 22, color: palette.fgSecondary)
            }
            .accessibilityLabel("Trash")
            Button {
                showSettings = true
            } label: {
                Glyph(Icon.settings, size: 22, color: palette.fgSecondary)
            }
            .accessibilityLabel("Settings")
        }
        .padding(.horizontal, 22)
        .padding(.vertical, 12)
        .overlay(alignment: .top) { Rectangle().fill(palette.border).frame(height: 1) }
    }

    // MARK: The notes

    /// Pinned notes, then the shared library (a folder with a cloud), then this phone's folders
    /// and the notes with no folder: the same order as the desktop sidebar.
    @ViewBuilder
    private var groups: some View {
        let pinned = library.notes.filter { $0.pinned }
        let remote = split(library.notes.filter { $0.shared && !$0.pinned })
        let local = split(library.notes.filter { !$0.shared && !$0.pinned })

        if !library.joined.isEmpty {
            sectionHeading(Icon.cloud, "SHARED WITH YOU", tint: palette.accent)
            ForEach(library.joined.keys.sorted(), id: \.self) { id in
                Button {
                    path.append(id)
                } label: {
                    Text(library.joined[id]?.title() ?? "Shared note")
                        .font(AppFont.body(17)).foregroundStyle(palette.fgPrimary)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .buttonStyle(.plain)
                .listRowInsets(inset(0))
                .listRowBackground(Color.clear)
                .listRowSeparator(.hidden)
            }
        }

        if !pinned.isEmpty {
            sectionHeading(Icon.pinned, "PINNED", tint: palette.pin)
            rows(pinned, indent: 0)
        }

        if !(remote.unfiled.isEmpty && remote.folders.isEmpty) {
            let count = remote.unfiled.count + remote.folders.reduce(0) { $0 + $1.notes.count }
            let key = "server"
            header(key: key, glyph: Icon.cloud, title: library.serverName ?? "Shared library", count: count, indent: 0)
            if !library.collapsed.contains(key) {
                rows(remote.unfiled, indent: 1)
                ForEach(remote.folders) { folder in
                    let folderKey = "server/\(folder.name)"
                    header(key: folderKey, glyph: Icon.folder, title: folder.name, count: folder.notes.count, indent: 1)
                    if !library.collapsed.contains(folderKey) {
                        rows(folder.notes, indent: 2)
                    }
                }
            }
        }

        ForEach(local.folders) { folder in
            let key = "local/\(folder.name)"
            header(key: key, glyph: Icon.folder, title: folder.name, count: folder.notes.count, indent: 0)
            if !library.collapsed.contains(key) {
                rows(folder.notes, indent: 1)
            }
        }
        rows(local.unfiled, indent: 0)
    }

    private func inset(_ indent: Int, top: CGFloat = 5, bottom: CGFloat = 5) -> EdgeInsets {
        EdgeInsets(top: top, leading: 18 + CGFloat(indent) * 22, bottom: bottom, trailing: 14)
    }

    private func sectionHeading(_ glyph: String, _ title: String, tint: Color) -> some View {
        HStack(spacing: 8) {
            Glyph(glyph, size: 15, color: tint)
            Text(title).font(AppFont.body(14)).tracking(0.6).foregroundStyle(tint)
            Spacer()
        }
        .padding(.top, 8)
        .listRowInsets(inset(0, top: 6, bottom: 4))
        .listRowBackground(Color.clear)
        .listRowSeparator(.hidden)
    }

    private func header(key: String, glyph: String, title: String, count: Int, indent: Int) -> some View {
        Button {
            library.toggleFolder(key)
        } label: {
            HStack(spacing: 8) {
                Glyph(library.collapsed.contains(key) ? Icon.chevronRight : Icon.chevronDown, size: 14, color: palette.fgMuted)
                    .frame(width: 14)
                Glyph(glyph, size: 18, color: palette.fgSecondary)
                Text(title).font(AppFont.body(17)).foregroundStyle(palette.fgSecondary).lineLimit(1)
                Text("\(count)").font(AppFont.body(14)).foregroundStyle(palette.fgMuted)
                Spacer()
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .listRowInsets(inset(indent - 0, top: 7, bottom: 7))
        .listRowBackground(Color.clear)
        .listRowSeparator(.hidden)
    }

    @ViewBuilder
    private func rows(_ notes: [NoteSummary], indent: Int) -> some View {
        ForEach(notes) { note in
            Button {
                path.append(note.id)
            } label: {
                NoteRow(note: note, showPreview: showPreviews)
            }
            .buttonStyle(.plain)
            .contextMenu {
                Button {
                    moving = note
                } label: {
                    Label(note.folder == nil ? "Add to a folder…" : "Change folder…", systemImage: "folder")
                }
                Button {
                    Task { await library.copy(note) }
                } label: {
                    Label(note.shared ? "Copy to this phone" : "Make a copy", systemImage: "doc.on.doc")
                }
                Button(role: .destructive) {
                    Task { await library.trash([note.id]) }
                } label: {
                    Label("Delete", systemImage: "trash")
                }
            }
            .listRowInsets(inset(indent))
            .listRowBackground(Color.clear)
            .listRowSeparator(.hidden)
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

/// A note in the list: its icon, its name and tags, and the preview line when that is turned on.
struct NoteRow: View {
    let note: NoteSummary
    let showPreview: Bool
    @Environment(\.palette) private var palette

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Glyph(Icon.named(note.icon), size: 19, color: palette.fgSecondary)
                .frame(width: 22)
                .padding(.top, 2)
            VStack(alignment: .leading, spacing: 2) {
                Text(note.title)
                    .font(AppFont.body(19))
                    .foregroundStyle(palette.fgPrimary)
                    .lineLimit(1)
                if !note.tags.isEmpty {
                    Text(note.tags.map { "#\($0)" }.joined(separator: "  "))
                        .font(AppFont.body(14))
                        .foregroundStyle(palette.accent)
                        .lineLimit(1)
                }
                if showPreview, !note.preview.isEmpty {
                    Text(note.preview)
                        .font(AppFont.body(15))
                        .foregroundStyle(palette.fgSecondary)
                        .lineLimit(2)
                }
            }
            Spacer(minLength: 0)
        }
        .contentShape(Rectangle())
    }
}
