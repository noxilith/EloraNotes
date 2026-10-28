import SwiftUI

/// Ko-fi's own symbol, the same artwork the desktop app uses: the cup in the text colour and the
/// heart in the accent.
struct KofiMark: View {
    @Environment(\.palette) private var palette

    var body: some View {
        ZStack {
            Image("KofiCup").renderingMode(.template).resizable().scaledToFit()
                .foregroundStyle(palette.fgSecondary)
            Image("KofiHeart").renderingMode(.template).resizable().scaledToFit()
                .foregroundStyle(palette.accent)
        }
        .frame(width: 27, height: 22)
    }
}
