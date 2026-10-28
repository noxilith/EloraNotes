import SwiftUI

struct NoteListView: View {
    @EnvironmentObject private var library: Library
    @State private var query = ""
    @State private var results: [NoteSummary] = []
    @State private var showSettings = false
    @State private var showNew = false
    @State private var showGraph = false
    @State private var path: [String] = []

    private var shown: [NoteSummary] {
        query.trimmingCharacters(in: .whitespaces).isEmpty ? library.notes : results
    }

    var body: some View {
        NavigationStack(path: $path) {
            List {
                if let why = library.sharedError {
                    Section {
                        Label(why, systemImage: "exclamationmark.triangle")
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                    }
                }
                ForEach(shown) { note in
                    NavigationLink(value: note.id) {
                        NoteRow(note: note)
                    }
                }
                .onDelete { offsets in
                    let ids = offsets.map { shown[$0].id }
                    Task { await library.trash(ids) }
                }
            }
            .listStyle(.plain)
            .navigationTitle("Notes")
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
                ToolbarItem(placement: .topBarLeading) {
                    Button {
                        showSettings = true
                    } label: {
                        Image(systemName: "gearshape")
                    }
                    .accessibilityLabel("Settings")
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
}

struct NoteRow: View {
    let note: NoteSummary

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 6) {
                if note.pinned {
                    Image(systemName: "pin.fill").font(.caption).foregroundStyle(.orange)
                }
                Text(note.title).font(.headline).lineLimit(1)
                Spacer(minLength: 0)
                if note.shared {
                    Image(systemName: "person.2.fill").font(.caption).foregroundStyle(.secondary)
                }
            }
            let detail = ([note.folder].compactMap { $0 } + note.tags.map { "#\($0)" })
                .joined(separator: "  ")
            if !detail.isEmpty {
                Text(detail).font(.caption).foregroundStyle(.secondary).lineLimit(1)
            }
            if !note.snippet.isEmpty {
                Text(note.snippet).font(.subheadline).foregroundStyle(.secondary).lineLimit(2)
            }
        }
        .padding(.vertical, 2)
    }
}
