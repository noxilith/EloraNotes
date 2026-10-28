import SwiftUI

/// Draws a note's Markdown. Headings, lists, tasks, quotes and code blocks are drawn as blocks;
/// bold, italics, code and links inside a line are drawn by the system. A `[[wiki link]]` is a
/// tappable link to the note it names, or plain muted text when there is no such note yet.
struct MarkdownView: View {
    let text: String
    let links: [NoteLink]
    let open: (String) -> Void
    @Environment(\.palette) private var palette

    private enum Block {
        case heading(Int, String)
        case bullet(Int, String)
        case task(Bool, String)
        case quote(String)
        case code(String)
        case rule
        case paragraph(String)
    }

    private var blocks: [Block] {
        var blocks: [Block] = []
        var code: [String]?
        for raw in text.components(separatedBy: "\n") {
            let line = raw.replacingOccurrences(of: "\r", with: "")
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("```") {
                if let lines = code {
                    blocks.append(.code(lines.joined(separator: "\n")))
                    code = nil
                } else {
                    code = []
                }
                continue
            }
            if code != nil {
                code?.append(line)
                continue
            }
            if trimmed.isEmpty { continue }
            if trimmed == "---" || trimmed == "***" {
                blocks.append(.rule)
            } else if let level = headingLevel(trimmed) {
                blocks.append(.heading(level, String(trimmed.drop { $0 == "#" }.dropFirst())))
            } else if trimmed.hasPrefix("- [ ] ") {
                blocks.append(.task(false, String(trimmed.dropFirst(6))))
            } else if trimmed.lowercased().hasPrefix("- [x] ") {
                blocks.append(.task(true, String(trimmed.dropFirst(6))))
            } else if trimmed.hasPrefix("- ") || trimmed.hasPrefix("* ") {
                let indent = line.prefix { $0 == " " || $0 == "\t" }.count / 2
                blocks.append(.bullet(indent, String(trimmed.dropFirst(2))))
            } else if trimmed.hasPrefix(">") {
                blocks.append(.quote(String(trimmed.dropFirst()).trimmingCharacters(in: .whitespaces)))
            } else {
                blocks.append(.paragraph(trimmed))
            }
        }
        if let lines = code {
            blocks.append(.code(lines.joined(separator: "\n")))
        }
        return blocks
    }

    private func headingLevel(_ line: String) -> Int? {
        let hashes = line.prefix { $0 == "#" }.count
        guard (1...6).contains(hashes), line.dropFirst(hashes).hasPrefix(" ") else { return nil }
        return hashes
    }

    /// `[[Title]]` and `[[Title|shown]]` as Markdown links the app handles itself.
    private func inline(_ source: String) -> AttributedString {
        let ids = Dictionary(
            links.compactMap { link in link.id.map { (link.text.lowercased(), $0) } },
            uniquingKeysWith: { first, _ in first })
        var result = source
        if let pattern = try? NSRegularExpression(pattern: #"\[\[([^\]|]+)(?:\|([^\]]+))?\]\]"#) {
            let whole = NSRange(source.startIndex..., in: source)
            for match in pattern.matches(in: source, range: whole).reversed() {
                guard let full = Range(match.range, in: source),
                      let target = Range(match.range(at: 1), in: source)
                else { continue }
                let name = source[target].trimmingCharacters(in: .whitespaces)
                var shown = name
                if match.range(at: 2).location != NSNotFound, let alias = Range(match.range(at: 2), in: source) {
                    shown = String(source[alias])
                }
                shown = shown.replacingOccurrences(of: "[", with: "\\[").replacingOccurrences(of: "]", with: "\\]")
                let replacement = ids[name.lowercased()].map { "[\(shown)](elora://\($0))" } ?? shown
                result.replaceSubrange(full, with: replacement)
            }
        }
        let options = AttributedString.MarkdownParsingOptions(
            interpretedSyntax: .inlineOnlyPreservingWhitespace)
        return (try? AttributedString(markdown: result, options: options)) ?? AttributedString(source)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in
                switch block {
                case .heading(let level, let value):
                    Text(inline(value))
                        .font(AppFont.bold(level == 1 ? 24 : level == 2 ? 20 : 17))
                        .padding(.top, level <= 2 ? 6 : 2)
                case .bullet(let depth, let value):
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text("•").foregroundStyle(palette.accent)
                        Text(inline(value))
                    }
                    .padding(.leading, CGFloat(depth) * 16)
                case .task(let done, let value):
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Image(systemName: done ? "checkmark.square.fill" : "square")
                            .foregroundStyle(done ? palette.accent : palette.fgMuted)
                        Text(inline(value)).strikethrough(done)
                    }
                case .quote(let value):
                    HStack(spacing: 10) {
                        Rectangle().fill(palette.accent).frame(width: 3)
                        Text(inline(value)).foregroundStyle(palette.fgSecondary)
                    }
                case .code(let value):
                    Text(value)
                        .font(AppFont.mono(14))
                        .padding(10)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(palette.elevated, in: RoundedRectangle(cornerRadius: 8))
                case .rule:
                    Rectangle().fill(palette.border).frame(height: 1)
                case .paragraph(let value):
                    Text(inline(value))
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .font(AppFont.body(18))
        .lineSpacing(5)
        .foregroundStyle(palette.fgPrimary)
        .textSelection(.enabled)
        .environment(
            \.openURL,
            OpenURLAction { url in
                guard url.scheme == "elora", let id = url.host else { return .systemAction }
                open(id)
                return .handled
            })
    }
}
