import SwiftUI

struct SettingsView: View {
    @EnvironmentObject private var library: Library
    @Environment(\.dismiss) private var dismiss
    @Environment(\.palette) private var palette
    @AppStorage("EloraTheme") private var themeName = AppTheme.obsidian.rawValue
    @AppStorage("showPreviews") private var showPreviews = false
    @State private var host = ""
    @State private var port = "7331"
    @State private var password = ""
    @State private var name = ""
    @State private var identifier = ""
    @State private var confirmNewIdentifier = false
    @State private var busy = false
    @State private var message: String?
    @State private var confirming: String?

    var body: some View {
        NavigationStack {
            Form {
                Section("Appearance") {
                    ForEach(AppTheme.allCases) { theme in
                        Button {
                            themeName = theme.rawValue
                        } label: {
                            HStack(spacing: 12) {
                                swatch(theme)
                                Text(theme.rawValue).foregroundStyle(palette.fgPrimary)
                                Spacer()
                                if theme.rawValue == themeName {
                                    Image(systemName: "checkmark").foregroundStyle(palette.accent)
                                }
                            }
                        }
                    }
                }
                Section {
                    Toggle("Show a preview line", isOn: $showPreviews)
                } header: {
                    Text("Note list")
                } footer: {
                    Text("Adds the first lines of each note under its name. Off keeps the list short.")
                }
                Section {
                    TextField("Server address", text: $host)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .keyboardType(.URL)
                    TextField("Port", text: $port).keyboardType(.numberPad)
                    SecureField("Server password", text: $password)
                    TextField("Your name", text: $name)
                    HStack {
                        SecureField("Your identifier", text: $identifier)
                            .textInputAutocapitalization(.never)
                            .autocorrectionDisabled()
                        Button("New") {
                            // With none yet, one is made at once; replacing one asks first.
                            if identifier.trimmingCharacters(in: .whitespaces).isEmpty {
                                identifier = Self.freshIdentifier()
                            } else {
                                confirmNewIdentifier = true
                            }
                        }
                        .buttonStyle(.borderless)
                        Button("Copy") { UIPasteboard.general.string = identifier }
                            .buttonStyle(.borderless)
                            .disabled(identifier.isEmpty)
                    }
                    if !identifier.isEmpty, !identifierLongEnough {
                        Text("The identifier needs at least 3 characters. Press New for a fresh one, or paste the one from your other device.")
                            .font(.footnote)
                            .foregroundStyle(.red)
                    }
                } header: {
                    Text("Shared library")
                } footer: {
                    Text("The same address, password, name and identifier as in EloraNotes on your computer, under Settings → Shared library. The identifier needs at least 3 characters.")
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
                    if let policy = URL(string: "https://noxilith.github.io/docs/privacy/") {
                        Link("Privacy policy", destination: policy)
                    }
                    if let support = URL(string: "https://github.com/noxilith/EloraNotes/issues") {
                        Link("Support and feedback", destination: support)
                    }
                    NavigationLink("Open-source credits") { AcknowledgementsView() }
                    Text("Notes on this phone are kept in the EloraNotes folder of the Files app.")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
            }
            .scrollContentBackground(.hidden)
            .background(palette.canvas)
            .toolbarBackground(palette.sidebar, for: .navigationBar)
            .toolbarBackground(.visible, for: .navigationBar)
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
            .onAppear(perform: fill)
            .confirmationDialog("Make a new identifier?", isPresented: $confirmNewIdentifier, titleVisibility: .visible) {
                Button("Make a new one", role: .destructive) { identifier = Self.freshIdentifier() }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("The server will see you as a different person: the name and notes tied to the old identifier stay with it. Copy the old one first if you want it back.")
            }
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

    /// Four dots in the theme's own colours.
    private func swatch(_ theme: AppTheme) -> some View {
        let colors = theme.palette
        return HStack(spacing: -6) {
            ForEach(Array([colors.canvas, colors.elevated, colors.accent, colors.fgPrimary].enumerated()), id: \.offset) { _, color in
                Circle().fill(color).frame(width: 18, height: 18)
                    .overlay(Circle().stroke(colors.border, lineWidth: 1))
            }
        }
        .frame(width: 64, alignment: .leading)
    }

    /// A new identifier: 32 random hexadecimal characters, the same kind the desktop app makes.
    private static func freshIdentifier() -> String {
        (0..<16).map { _ in String(format: "%02x", UInt8.random(in: 0...255)) }.joined()
    }

    private var identifierLongEnough: Bool {
        identifier.trimmingCharacters(in: .whitespacesAndNewlines).count >= 3
    }

    private var ready: Bool {
        !host.trimmingCharacters(in: .whitespaces).isEmpty && !password.isEmpty
            && !name.trimmingCharacters(in: .whitespaces).isEmpty && identifierLongEnough
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
            name.trimmingCharacters(in: .whitespaces),
            identifier.trimmingCharacters(in: .whitespacesAndNewlines)
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
                identifier: identifier.trimmingCharacters(in: .whitespacesAndNewlines)))
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
