import SwiftUI

@main
struct EloraNotesApp: App {
    @StateObject private var library = Library()

    var body: some Scene {
        WindowGroup {
            NoteListView()
                .environmentObject(library)
                .task { await library.start() }
        }
    }
}
