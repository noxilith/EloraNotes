import SwiftUI

struct SettingsView: View {
    @EnvironmentObject private var library: Library
    @Environment(\.dismiss) private var dismiss
    @State private var host = ""
    @State private var port = "7331"
    @State private var password = ""
    @State private var name = ""
    @State private var identifier = ""
    @State private var busy = false
    @State private var message: String?
    @State private var confirming: String?

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("Server address", text: $host)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .keyboardType(.URL)
                    TextField("Port", text: $port).keyboardType(.numberPad)
                    SecureField("Server password", text: $password)
                    TextField("Your name", text: $name)
                    SecureField("Your identifier", text: $identifier)
                } header: {
                    Text("Shared library")
                } footer: {
                    Text("The same address, password, name and identifier as in EloraNotes on your computer, under Settings → Shared library.")
                }
                Section {
                    Button(busy ? "Connecting…" : "Connect") { Task { await connect() } }
                        .disabled(!ready || busy)
                    if library.serverConfigured {
                        Button("Disconnect", role: .destructive) { disconnect() }
                    }
                    if let message {
                        Text(message).font(.footnote).foregroundStyle(.secondary)
                    }
                }
                Section("About") {
                    LabeledContent("App", value: Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "")
                    LabeledContent("Core", value: coreVersion())
                    Text("Notes on this phone are kept in the EloraNotes folder of the Files app.")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
            .onAppear(perform: fill)
            .alert(
                "Trust this server?",
                isPresented: Binding(get: { confirming != nil }, set: { if !$0 { confirming = nil } })
            ) {
                Button("Trust and connect") {
                    if let fingerprint = confirming { Task { await save(fingerprint) } }
                }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("Its certificate code is:\n\n\(grouped(confirming ?? ""))\n\nOnly continue if it matches the code EloraNotes shows on your computer.")
            }
        }
    }

    private var ready: Bool {
        !host.trimmingCharacters(in: .whitespaces).isEmpty && !password.isEmpty
            && !name.trimmingCharacters(in: .whitespaces).isEmpty && identifier.count >= 8
            && UInt16(port) != nil
    }

    private func fill() {
        guard let saved = library.store.load() else { return }
        host = saved.host
        port = String(saved.port)
        password = saved.password
        name = saved.displayName
        identifier = saved.identifier
    }

    private func grouped(_ fingerprint: String) -> String {
        stride(from: 0, to: fingerprint.count, by: 4).map { start in
            let from = fingerprint.index(fingerprint.startIndex, offsetBy: start)
            let to = fingerprint.index(from, offsetBy: min(4, fingerprint.count - start))
            return String(fingerprint[from..<to])
        }.joined(separator: " ")
    }

    private func connect() async {
        busy = true
        defer { busy = false }
        message = nil
        let (h, p, pw, n, i) = (
            host.trimmingCharacters(in: .whitespaces), UInt16(port) ?? 7331, password,
            name.trimmingCharacters(in: .whitespaces), identifier
        )
        do {
            let found = try await library.run {
                try $0.probeServer(host: h, port: p, password: pw, displayName: n, identifier: i)
            }
            if library.store.load()?.fingerprint == found {
                await save(found)
            } else {
                confirming = found
            }
        } catch {
            message = library.describe(error)
        }
    }

    private func save(_ fingerprint: String) async {
        library.store.save(
            ServerSettings(
                host: host.trimmingCharacters(in: .whitespaces),
                port: UInt16(port) ?? 7331,
                password: password,
                fingerprint: fingerprint,
                displayName: name.trimmingCharacters(in: .whitespaces),
                identifier: identifier))
        library.applyServer()
        await library.refresh()
        message = library.sharedError ?? "Connected."
    }

    private func disconnect() {
        library.store.clear()
        library.applyServer()
        Task { await library.refresh() }
        message = "Disconnected. Shared notes are no longer shown."
    }
}
