import SwiftUI

/// The open-source software the app is built from, written by `scripts/make-acknowledgements.py`.
struct AcknowledgementsView: View {
    @Environment(\.palette) private var palette

    private var text: String {
        guard let url = Bundle.main.url(forResource: "Acknowledgements", withExtension: "txt"),
              let contents = try? String(contentsOf: url, encoding: .utf8)
        else { return "The list of credits is missing from this build." }
        return contents
    }

    var body: some View {
        ScrollView {
            Text(text)
                .font(.system(.footnote, design: .monospaced))
                .foregroundStyle(palette.fgSecondary)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding()
                .textSelection(.enabled)
        }
        .background(palette.canvas)
        .toolbarBackground(palette.sidebar, for: .navigationBar)
        .toolbarBackground(.visible, for: .navigationBar)
        .navigationTitle("Open-source credits")
        .navigationBarTitleDisplayMode(.inline)
    }
}
