import SwiftUI

@main
struct EloraNotesApp: App {
    @StateObject private var library = Library()

    var body: some Scene {
        WindowGroup {
            NoteListView()
                .environmentObject(library)
                .themed()
                .task { await library.start() }
                .onOpenURL { url in
                    // elora://share/… from a tap on a link, or from the join page.
                    library.pendingLink = url.absoluteString
                }
        }
    }
}
