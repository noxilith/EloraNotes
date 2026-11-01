import SwiftUI

@main
struct EloraNotesApp: App {
    @StateObject private var library = Library()
    @Environment(\.scenePhase) private var scenePhase
    @State private var backgroundTask = UIBackgroundTaskIdentifier.invalid

    var body: some Scene {
        WindowGroup {
            NoteListView()
                .environmentObject(library)
                .themed()
                .task { await library.start() }
                .onChange(of: scenePhase) { _, phase in
                    // Leaving the app to send a link: iOS lets it run a little longer, which keeps
                    // a note being shared reachable for those first moments.
                    if phase == .background, !library.shares.isEmpty, backgroundTask == .invalid {
                        backgroundTask = UIApplication.shared.beginBackgroundTask {
                            UIApplication.shared.endBackgroundTask(backgroundTask)
                            backgroundTask = .invalid
                        }
                    } else if phase == .active, backgroundTask != .invalid {
                        UIApplication.shared.endBackgroundTask(backgroundTask)
                        backgroundTask = .invalid
                    }
                }
                .onOpenURL { url in
                    // elora://share/… from a tap on a link, or from the join page.
                    library.pendingLink = url.absoluteString
                }
        }
    }
}
