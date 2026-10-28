import SwiftUI

struct NewNoteView: View {
    @EnvironmentObject private var library: Library
    @Environment(\.dismiss) private var dismiss
    @Environment(\.palette) private var palette
    let created: (String) -> Void
    @State private var title = ""
    @State private var folder = ""
    @State private var shared = false
    @State private var busy = false
    @State private var message: String?

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Title", text: $title)
                    TextField("Folder (optional)", text: $folder)
                        .textInputAutocapitalization(.words)
                }
                if library.serverConfigured {
                    Section {
                        Toggle("Put it in the shared library", isOn: $shared)
                    }
                }
                if let message {
                    Section { Text(message).foregroundStyle(.red).font(.footnote) }
                }
            }
            .scrollContentBackground(.hidden)
            .background(palette.canvas)
            .toolbarBackground(palette.sidebar, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
            .navigationTitle("New note")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Create") { Task { await create() } }
                        .disabled(title.trimmingCharacters(in: .whitespaces).isEmpty || busy)
                }
            }
        }
    }

    private func create() async {
        busy = true
        defer { busy = false }
        let name = folder.trimmingCharacters(in: .whitespaces)
        do {
            let made = try await library.create(
                title: title, folder: name.isEmpty ? nil : name, shared: shared)
            dismiss()
            created(made.id)
        } catch {
            message = library.describe(error)
        }
    }
}
