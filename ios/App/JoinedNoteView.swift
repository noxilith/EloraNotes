import SwiftUI

/// A note somebody shared by link: its text as it is typed, by them and by you unless the link
/// only lets you watch. Nothing is saved on this phone. If the person sharing cannot be reached
/// for a while, the note closes and what it said last is kept as a note of your own.
struct JoinedNoteView: View {
    let id: String
    @Binding var path: [String]
    @EnvironmentObject private var library: Library
    @Environment(\.palette) private var palette
    @State private var text = ""
    @State private var editorText = EditorText(id: 0, text: "")
    @State private var ready = false
    @State private var people: [LivePerson] = []
    @State private var error: String?
    @State private var lostSince: Date?
    @State private var closed = false
    private let tick = Timer.publish(every: 0.06, on: .main, in: .common).autoconnect()
    /// How long the person sharing may be out of reach before the note is given up.
    private let lostAfter: TimeInterval = 8

    private var share: JoinedShare? { library.joined[id] }

    var body: some View {
        VStack(spacing: 0) {
            if !people.isEmpty { whoIsHere }
            if !ready {
                Text(error ?? "Connecting…")
                    .font(AppFont.body(14)).foregroundStyle(palette.fgSecondary).padding(8)
            }
            if let share {
                if share.viewOnly() {
                    ScrollView {
                        MarkdownView(text: text, links: []) { _ in }
                            .padding()
                            .frame(maxWidth: .infinity, alignment: .leading)
                    }
                } else {
                    NoteEditor(
                        incoming: editorText, people: people, palette: palette, editable: ready,
                        onEdit: { value in
                            text = value
                            share.live().setText(text: value)
                        },
                        onCursor: { byte in share.live().setCursor(byte: UInt32(byte)) })
                }
            } else {
                Spacer()
            }
        }
        .background(palette.canvas)
        .toolbarBackground(palette.sidebar, for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
        .navigationTitle(share?.title() ?? "")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Button("Leave") { close() }
            }
        }
        .onReceive(tick) { _ in look() }
    }

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

    private func look() {
        guard !closed, let share else { return }
        let update = share.live().poll()
        people = update.people
        error = update.error
        if let incoming = update.text {
            ready = true
            if incoming != text {
                text = incoming
                editorText = EditorText(id: editorText.id + 1, text: incoming)
            }
        }
        guard ready else { return }
        if update.connected {
            lostSince = nil
        } else if let since = lostSince {
            if Date().timeIntervalSince(since) > lostAfter { Task { await lost(share) } }
        } else {
            lostSince = Date()
        }
    }

    /// Leaves the note, keeping nothing.
    private func close() {
        closed = true
        library.leave(id)
        path.removeAll { $0 == id }
    }

    /// The person sharing is gone: the note closes and its last text stays as a copy.
    private func lost(_ share: JoinedShare) async {
        guard !closed else { return }
        closed = true
        let (title, body) = (share.title(), text)
        library.leave(id)
        path.removeAll { $0 == id }
        await library.keepCopy(title: title, body: body)
        library.showNotice("Lost the connection to \(title). It is closed, and a copy is saved on this phone.")
    }
}
