import Foundation
import SwiftUI

extension NoteSummary: Identifiable {}

enum LibraryError: Error {
    case notReady
}

/// What every screen shares: the open vault and the list of notes. The Rust core does the
/// work; calls that may wait on the network run off the main thread.
@MainActor
final class Library: ObservableObject {
    @Published var notes: [NoteSummary] = []
    @Published var sharedError: String?
    @Published var loading = false
    @Published var problem: String?
    @Published var serverConfigured = false
    /// The shared library's name, once connected: the title of its folder in the list.
    @Published var serverName: String?
    /// Folders the person has collapsed, kept between launches.
    @Published var collapsed: Set<String> = Set(
        UserDefaults.standard.stringArray(forKey: "collapsedFolders") ?? [])
    /// Set when the app was started in demo mode to open on a particular screen.
    @Published var launchRoute: String?

    private var vault: Vault?
    let store = ServerStore()

    /// Opens the phone's notes folder, which is also visible in the Files app.
    func start() async {
        guard vault == nil else { return }
        do {
            if DemoData.enabled {
                UserDefaults.standard.set(DemoData.previews, forKey: "showPreviews")
                let demo = try Vault.open(directory: DemoData.folder())
                DemoData.seed(demo)
                vault = demo
                await refresh()
                launchRoute = DemoData.screen
                return
            }
            let documents = try FileManager.default.url(
                for: .documentDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
            let folder = documents.appendingPathComponent("EloraNotes", isDirectory: true)
            vault = try Vault.open(directory: folder.path)
            applyServer()
            await refresh()
        } catch {
            problem = describe(error)
        }
    }

    func toggleFolder(_ key: String) {
        if collapsed.contains(key) { collapsed.remove(key) } else { collapsed.insert(key) }
        UserDefaults.standard.set(Array(collapsed), forKey: "collapsedFolders")
    }

    func applyServer() {
        let settings = store.load()
        vault?.setServer(settings: settings)
        serverConfigured = settings != nil
    }

    func describe(_ error: Error) -> String {
        if let core = error as? CoreError, case .Failed(let message) = core {
            return message
        }
        return error.localizedDescription
    }

    /// Runs `work` against the vault off the main thread.
    func run<T>(_ work: @escaping (Vault) throws -> T) async throws -> T {
        guard let vault else { throw LibraryError.notReady }
        return try await Task.detached(priority: .userInitiated) { try work(vault) }.value
    }

    func refresh() async {
        guard let vault else { return }
        loading = true
        let list = await Task.detached(priority: .userInitiated) {
            try? vault.reload()
            return vault.listNotes()
        }.value
        notes = list.notes
        sharedError = list.sharedError
        serverName = list.serverName
        if DemoData.sharedPreview {
            // For screenshots only: some notes shown as if they came from a shared library.
            notes = notes.map { note in
                var note = note
                if note.folder == "Audio" || note.folder == "Life" { note.shared = true }
                return note
            }
            serverName = "Home server"
        }
        loading = false
    }

    /// Puts a note that was just saved or made into the list without asking the server again.
    func replace(_ summary: NoteSummary) {
        if let index = notes.firstIndex(where: { $0.id == summary.id }) {
            notes[index] = summary
        } else {
            notes.append(summary)
        }
        notes.sort {
            if $0.pinned != $1.pinned { return $0.pinned }
            if $0.updated != $1.updated { return $0.updated > $1.updated }
            return $0.title < $1.title
        }
    }

    func create(title: String, folder: String?, shared: Bool) async throws -> NoteSummary {
        let made = try await run {
            try $0.createNote(title: title, folder: folder, body: "", shared: shared)
        }
        replace(made)
        return made
    }

    func trash(_ ids: [String]) async {
        for id in ids {
            do {
                try await run { try $0.trashNote(id: id) }
                notes.removeAll { $0.id == id }
            } catch {
                problem = describe(error)
            }
        }
    }

    func search(_ query: String) async -> [NoteSummary] {
        (try? await run { $0.search(query: query) }) ?? []
    }
}
