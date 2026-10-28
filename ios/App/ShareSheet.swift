import SwiftUI

/// Share one note by link with nobody running a server: pick whether people can edit or only
/// watch, give out the link, stop whenever. The note stays on this phone.
struct ShareSheet: View {
    let noteID: String
    let shareName: String
    let onStarted: (ShareSession) -> Void
    let onStopped: () -> Void
    @EnvironmentObject private var library: Library
    @Environment(\.palette) private var palette
    @Environment(\.dismiss) private var dismiss
    @State private var viewOnly = false
    @State private var starting = false
    @State private var error: String?
    @State private var copied = false

    private var session: ShareSession? { library.shares[noteID] }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Picker("People can", selection: $viewOnly) {
                        Text("Edit").tag(false)
                        Text("Only watch").tag(true)
                    }
                    .pickerStyle(.segmented)
                } footer: {
                    Text("They see your typing live and you see theirs, each in their own colour. Nobody needs an account or a server.")
                }
                if let session {
                    let link = session.link(viewOnly: viewOnly, web: true)
                    Section {
                        Text(link)
                            .font(.footnote.monospaced())
                            .lineLimit(3)
                            .textSelection(.enabled)
                        ShareLink(item: link) { Label("Send link", systemImage: "square.and.arrow.up") }
                        Button {
                            UIPasteboard.general.string = link
                            copied = true
                        } label: {
                            Label(copied ? "Copied" : "Copy link", systemImage: "doc.on.doc")
                        }
                        Button {
                            UIPasteboard.general.string = session.link(viewOnly: viewOnly, web: false)
                            copied = true
                        } label: {
                            Label("Copy app link", systemImage: "link")
                        }
                    } footer: {
                        Text("The link works while EloraNotes is open on this phone. Treat it like a password: anyone who has it can get in.")
                    }
                    Section {
                        Button("Stop sharing", role: .destructive) {
                            session.stop()
                            library.shares[noteID] = nil
                            onStopped()
                            dismiss()
                        }
                    }
                } else {
                    Section {
                        Button {
                            Task { await start() }
                        } label: {
                            if starting {
                                HStack { ProgressView(); Text("Making a link…") }
                            } else {
                                Text("Make a link")
                            }
                        }
                        .disabled(starting)
                    }
                }
                if let error {
                    Section { Text(error).foregroundStyle(.red).font(.footnote) }
                }
            }
            .onChange(of: viewOnly) { _, _ in copied = false }
            .navigationTitle("Share")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
        }
        .presentationDetents([.medium, .large])
    }

    private func start() async {
        starting = true
        error = nil
        let (id, name) = (noteID, shareName)
        do {
            let session = try await library.run { try $0.startShare(id: id, name: name) }
            library.shares[noteID] = session
            onStarted(session)
        } catch {
            self.error = library.describe(error)
        }
        starting = false
    }
}
