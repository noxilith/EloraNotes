import SwiftUI

/// Join a note somebody shared: paste the link, or it is picked up from the clipboard.
struct JoinSheet: View {
    let onJoined: (String) -> Void
    @EnvironmentObject private var library: Library
    @Environment(\.dismiss) private var dismiss
    @State private var link = ""

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Paste the link", text: $link, axis: .vertical)
                        .lineLimit(1...4)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                } footer: {
                    Text("Anybody can share a note by link, with nobody running a server. You see their typing live, and they see yours unless the link only lets you watch.")
                }
                Section {
                    Button {
                        Task { await join() }
                    } label: {
                        if library.joining {
                            HStack { ProgressView(); Text("Joining…") }
                        } else {
                            Text("Join")
                        }
                    }
                    .disabled(library.joining || link.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
            }
            .navigationTitle("Join a shared note")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
            }
            .onAppear {
                if link.isEmpty, let pasted = UIPasteboard.general.string,
                    findShareLink(text: pasted) != nil
                {
                    link = pasted
                }
            }
        }
        .presentationDetents([.medium])
    }

    private func join() async {
        if let id = await library.join(link) {
            dismiss()
            onJoined(id)
        }
    }
}
