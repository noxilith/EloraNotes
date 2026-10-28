import SwiftUI

/// One note: read it with its links and backlinks, or edit its text. A note on this phone saves
/// a moment after typing stops. A shared note is edited live, the way the desktop does it: your
/// typing reaches everybody at once, theirs reaches you without moving your caret, and everyone
/// else with the note open has a caret in their colour with their name on it.
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
    @State private var editorText = EditorText(id: 0, text: "")
    @State private var live: LiveSession?
    @State private var liveReady = false
    @State private var liveError: String?
    @State private var people: [LivePerson] = []
    private let tick = Timer.publish(every: 0.06, on: .main, in: .common).autoconnect()

    private var shared: Bool { detail?.summary.shared ?? false }
    /// A shared note can be edited once its live connection has its text.
    private var canEdit: Bool { detail != nil && (!shared || liveReady) }

    var body: some View {
        VStack(spacing: 0) {
            if !people.isEmpty { whoIsHere }
            if shared, !liveReady, editing {
                Text(liveError ?? "Connecting to the shared library…")
                    .font(AppFont.body(14)).foregroundStyle(palette.fgSecondary).padding(8)
            }
            if editing { pageHeader }
            content
        }
        .background(palette.canvas)
        .toolbarBackground(palette.sidebar, for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
        .navigationTitle("")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button(editing ? "Done" : "Edit") {
                    Task { await toggle() }
                }
                .disabled(!canEdit)
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
        .onReceive(tick) { _ in look() }
        .onChange(of: draft) { _, _ in
            if live == nil { scheduleSave() }
        }
        .onDisappear {
            live?.close()
            live = nil
            Task { await save() }
        }
    }

    @ViewBuilder
    private var content: some View {
        if detail == nil && message == nil {
            ProgressView().frame(maxHeight: .infinity)
        } else if editing {
            NoteEditor(
                incoming: editorText, people: people, palette: palette, editable: canEdit,
                onEdit: { text in
                    draft = text
                    live?.setText(text: text)
                },
                onCursor: { byte in live?.setCursor(byte: UInt32(byte)) })
        } else {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    pageHeader.padding(.horizontal, -16)
                    MarkdownView(text: draft, links: detail?.links ?? []) { id in
                        path.append(id)
                    }
                    if let backlinks = detail?.backlinks, !backlinks.isEmpty {
                        Rectangle().fill(palette.border).frame(height: 1)
                        Text("Linked from").font(AppFont.bold(15)).foregroundStyle(palette.fgPrimary)
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

    /// The title, tags and date, as the desktop shows them above the text.
    private var pageHeader: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(detail?.summary.title ?? "")
                .font(AppFont.body(31))
                .foregroundStyle(palette.fgPrimary)
            if let tags = detail?.summary.tags, !tags.isEmpty {
                Text(tags.map { "#\($0)" }.joined(separator: "  "))
                    .font(AppFont.body(16))
                    .foregroundStyle(palette.accent)
            }
            Text(metaLine)
                .font(AppFont.body(14))
                .foregroundStyle(palette.fgMuted)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 16)
        .padding(.top, 14)
        .padding(.bottom, 8)
    }

    /// "Today, 6:43 PM  ·  105 words"
    private var metaLine: String {
        guard let summary = detail?.summary else { return "" }
        let date = Date(timeIntervalSince1970: TimeInterval(summary.updated))
        let format = DateFormatter()
        if Calendar.current.isDateInToday(date) {
            format.dateFormat = "'Today,' h:mm a"
        } else if Calendar.current.isDateInYesterday(date) {
            format.dateFormat = "'Yesterday,' h:mm a"
        } else {
            format.dateStyle = .medium
            format.timeStyle = .short
        }
        let words = draft.split(whereSeparator: { $0.isWhitespace }).count
        return "\(format.string(from: date))  ·  \(words) word\(words == 1 ? "" : "s")"
    }

    /// The people with this note open, each in their own colour.
    private var whoIsHere: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(Array(people.enumerated()), id: \.offset) { _, person in
                    HStack(spacing: 5) {
                        Circle()
                            .fill(Color(red: Double(person.red) / 255, green: Double(person.green) / 255, blue: Double(person.blue) / 255))
                            .frame(width: 9, height: 9)
                        Text(person.name).font(AppFont.body(13)).foregroundStyle(palette.fgSecondary)
                    }
                    .padding(.horizontal, 9).padding(.vertical, 4)
                    .background(palette.elevated, in: Capsule())
                }
            }
            .padding(.horizontal, 12).padding(.vertical, 6)
        }
        .background(palette.sidebar)
    }

    private func load() async {
        do {
            let found = try await library.run { try $0.readNote(id: noteID) }
            detail = found
            draft = found.body
            saved = found.body
            editorText = EditorText(id: editorText.id + 1, text: found.body)
            message = nil
            if library.launchRoute == "edit" {
                // Screenshots only: open straight into the editor, with the keyboard up.
                editing = true
            }
            if found.summary.shared, live == nil {
                // Opening the note live: its text arrives through `look`.
                live = try? await library.run { try $0.openLive(id: noteID) }
            }
        } catch {
            message = library.describe(error)
        }
    }

    /// Asked every few hundredths of a second: what has somebody else done to the note?
    private func look() {
        guard let live else { return }
        let update = live.poll()
        people = update.people
        liveError = update.error
        if update.connected, update.text != nil { liveReady = true }
        if let text = update.text, text != draft {
            draft = text
            saved = text
            editorText = EditorText(id: editorText.id + 1, text: text)
        }
    }

    private func toggle() async {
        if editing {
            await save()
            editing = false
            if live == nil { await load() }
        } else {
            editorText = EditorText(id: editorText.id + 1, text: draft)
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
        // A shared note is kept up to date by its live connection as it is typed.
        guard live == nil, draft != saved else { return }
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
