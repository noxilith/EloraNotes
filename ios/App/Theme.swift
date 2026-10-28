import SwiftUI

/// The colours of one theme: the same names and values as the desktop app's palettes.
struct Palette {
    let canvas: Color
    let sidebar: Color
    let elevated: Color
    let selected: Color
    let fgPrimary: Color
    let fgSecondary: Color
    let fgMuted: Color
    let accent: Color
    let pin: Color
    let danger: Color
    let border: Color
    let isDark: Bool
}

private func rgb(_ r: Double, _ g: Double, _ b: Double) -> Color {
    Color(.sRGB, red: r / 255, green: g / 255, blue: b / 255)
}

/// The desktop app's themes, minus Matugen (which follows the desktop's wallpaper).
enum AppTheme: String, CaseIterable, Identifiable {
    case obsidian = "Obsidian"
    case paper = "Paper"
    case nordic = "Nordic"
    case matcha = "Matcha"
    case sepia = "Sepia"
    case midnight = "Midnight"

    var id: String { rawValue }

    var palette: Palette {
        switch self {
        case .obsidian:
            return Palette(
                canvas: rgb(20, 21, 24), sidebar: rgb(25, 27, 31), elevated: rgb(32, 35, 40),
                selected: rgb(41, 46, 54), fgPrimary: rgb(232, 230, 225),
                fgSecondary: rgb(168, 166, 160), fgMuted: rgb(112, 112, 108),
                accent: rgb(134, 168, 145), pin: rgb(214, 173, 108), danger: rgb(209, 109, 106),
                border: rgb(40, 43, 49), isDark: true)
        case .paper:
            return Palette(
                canvas: rgb(250, 248, 244), sidebar: rgb(242, 239, 233), elevated: rgb(234, 230, 222),
                selected: rgb(225, 220, 210), fgPrimary: rgb(38, 37, 35),
                fgSecondary: rgb(100, 98, 93), fgMuted: rgb(148, 144, 137),
                accent: rgb(76, 114, 91), pin: rgb(184, 128, 48), danger: rgb(186, 74, 72),
                border: rgb(224, 219, 210), isDark: false)
        case .nordic:
            return Palette(
                canvas: rgb(24, 28, 36), sidebar: rgb(29, 34, 44), elevated: rgb(37, 43, 55),
                selected: rgb(47, 56, 72), fgPrimary: rgb(229, 233, 240),
                fgSecondary: rgb(165, 176, 194), fgMuted: rgb(112, 124, 145),
                accent: rgb(136, 192, 208), pin: rgb(235, 203, 139), danger: rgb(191, 97, 106),
                border: rgb(46, 52, 64), isDark: true)
        case .matcha:
            return Palette(
                canvas: rgb(244, 247, 242), sidebar: rgb(234, 240, 232), elevated: rgb(224, 232, 221),
                selected: rgb(211, 223, 207), fgPrimary: rgb(34, 43, 36),
                fgSecondary: rgb(86, 102, 89), fgMuted: rgb(134, 148, 136),
                accent: rgb(65, 118, 84), pin: rgb(176, 124, 46), danger: rgb(184, 76, 74),
                border: rgb(212, 221, 209), isDark: false)
        case .sepia:
            return Palette(
                canvas: rgb(246, 238, 222), sidebar: rgb(237, 227, 207), elevated: rgb(252, 247, 236),
                selected: rgb(230, 216, 190), fgPrimary: rgb(58, 45, 30),
                fgSecondary: rgb(118, 96, 70), fgMuted: rgb(160, 142, 118),
                accent: rgb(150, 92, 40), pin: rgb(170, 110, 30), danger: rgb(168, 66, 56),
                border: rgb(222, 206, 180), isDark: false)
        case .midnight:
            return Palette(
                canvas: rgb(12, 16, 28), sidebar: rgb(16, 22, 38), elevated: rgb(24, 32, 52),
                selected: rgb(33, 45, 72), fgPrimary: rgb(220, 228, 245),
                fgSecondary: rgb(150, 164, 196), fgMuted: rgb(98, 112, 145),
                accent: rgb(110, 170, 255), pin: rgb(240, 190, 100), danger: rgb(238, 120, 130),
                border: rgb(34, 44, 68), isDark: true)
        }
    }
}

private struct PaletteKey: EnvironmentKey {
    static let defaultValue = AppTheme.obsidian.palette
}

extension EnvironmentValues {
    var palette: Palette {
        get { self[PaletteKey.self] }
        set { self[PaletteKey.self] = newValue }
    }
}

/// Puts the chosen theme on everything below it: the colours, the accent, and light or dark for
/// the system's own controls.
struct Themed: ViewModifier {
    @AppStorage("EloraTheme") private var name = AppTheme.obsidian.rawValue

    private var theme: AppTheme { AppTheme(rawValue: name) ?? .obsidian }

    func body(content: Content) -> some View {
        content
            .environment(\.palette, theme.palette)
            .font(AppFont.body(16))
            .tint(theme.palette.accent)
            .preferredColorScheme(theme.palette.isDark ? .dark : .light)
    }
}

extension View {
    func themed() -> some View { modifier(Themed()) }
}

/// The fonts the desktop app uses: Ubuntu Light for text, DejaVu Sans Bold for anything bold,
/// Hack for code, and the Tabler icon font for every icon.
enum AppFont {
    static func body(_ size: CGFloat) -> Font { .custom("Ubuntu-Light", size: size, relativeTo: .body) }
    static func bold(_ size: CGFloat) -> Font { .custom("DejaVuSans-Bold", size: size, relativeTo: .body) }
    static func mono(_ size: CGFloat) -> Font { .custom("Hack-Regular", size: size, relativeTo: .body) }
    static func icon(_ size: CGFloat) -> Font { .custom("tabler-icons", size: size) }
}

/// The icons the app draws itself, as Tabler glyphs (the same ones the desktop app uses).
enum Icon {
    static let plus = "\u{eb0b}"
    static let search = "\u{eb1c}"
    static let pinned = "\u{ed60}"
    static let folder = "\u{eaad}"
    static let file = "\u{eaa2}"
    static let chevronRight = "\u{ea61}"
    static let chevronDown = "\u{ea5f}"
    static let feather = "\u{ee8b}"
    static let settings = "\u{eb20}"
    static let cloud = "\u{ea76}"
    static let graph = "\u{f288}"
    static let edit = "\u{ea98}"
    static let eye = "\u{ea9a}"

    private static var known: [String: String] = [:]

    /// A note's own icon by name, or the plain page the desktop shows when it has none.
    static func named(_ name: String?) -> String {
        guard let name, !name.isEmpty else { return file }
        if let found = known[name] { return found }
        let glyph = iconGlyph(name: name) ?? file
        known[name] = glyph
        return glyph
    }
}

/// One icon from the icon font.
struct Glyph: View {
    let glyph: String
    var size: CGFloat = 16
    var color: Color

    init(_ glyph: String, size: CGFloat = 16, color: Color) {
        self.glyph = glyph
        self.size = size
        self.color = color
    }

    var body: some View {
        Text(glyph).font(AppFont.icon(size)).foregroundStyle(color)
    }
}
