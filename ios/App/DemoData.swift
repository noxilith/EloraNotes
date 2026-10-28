import Foundation

/// A made-up vault for trying the app out: `-EloraDemo YES` on the command line starts the app
/// with it instead of the real notes folder. The CI job uses it to take screenshots in the
/// simulator.
enum DemoData {
    static var enabled: Bool { UserDefaults.standard.bool(forKey: "EloraDemo") }
    static var screen: String? { UserDefaults.standard.string(forKey: "EloraDemoScreen") }

    static func folder() -> String {
        FileManager.default.temporaryDirectory
            .appendingPathComponent("EloraDemo-\(UUID().uuidString)", isDirectory: true).path
    }

    private static let topics: [(String, String, [String])] = [
        ("Slime Combat", "Games", ["Green Slime", "Boss Slime", "Knockback", "Hit Sparks", "Combo Timing", "Dodge Roll", "Damage Numbers", "Slime Splits"]),
        ("World Map", "Games", ["Swamp", "Cave Entrance", "Crystal Lake", "Market Town", "Old Bridge", "Hidden Grove", "Fast Travel"]),
        ("Crafting", "Games", ["Gel Armor", "Bone Sword", "Potion Recipes", "Workbench", "Rare Drops", "Upgrade Tiers", "Salvage"]),
        ("Music", "Audio", ["Swamp Theme", "Boss Theme", "Menu Loop", "Footstep Sounds", "Mixing Notes", "Instruments"]),
        ("Reading List", "Life", ["Pattern Languages", "Game Feel", "Deep Work", "Rust Book", "Notes on Notes"]),
    ]

    private static let loose = [
        "Groceries", "Gift ideas", "Trip to Lisbon", "Passwords hint", "Bike repair", "Recipe: curry",
        "Movie list", "Wifi setup", "Book quotes", "Budget 2026", "Workout plan", "Dentist", "Birthday list", "Packing list",
    ]

    static func seed(_ vault: Vault) {
        func make(_ title: String, _ folder: String?, _ body: String) {
            _ = try? vault.createNote(title: title, folder: folder, body: body, shared: false)
        }
        for (index, topic) in topics.enumerated() {
            let (hub, folder, spokes) = topic
            let other = topics[(index + 1) % topics.count].0
            var body = "# \(hub)\n\nWhere the ideas for \(hub.lowercased()) live. Also see [[\(other)]].\n\n## Parts\n\n"
            body += spokes.map { "- [[\($0)]]" }.joined(separator: "\n")
            if hub == "Slime Combat" {
                body += """


                ## To do

                - [x] Add hit sparks
                - [ ] Tune the knockback
                - [ ] Boss fight **second phase**

                > Keep every fight under a minute.

                ```
                damage = base * (1 + combo / 10)
                ```
                """
            }
            make(hub, folder, body)
            for (n, spoke) in spokes.enumerated() {
                var text = "Notes about \(spoke.lowercased()). Part of [[\(hub)]]."
                if n % 3 == 0 { text += " Related: [[\(spokes[(n + 1) % spokes.count])]]." }
                if n == 2 { text += " Ties into [[\(other)]]." }
                make(spoke, folder, text)
            }
        }
        for title in loose { make(title, nil, "A note nothing links to yet.") }
    }
}
