import SwiftUI

/// Draws a note's Markdown the way the desktop app does: headings, lists (bulleted, numbered and
/// nested), task lists with boxes that can be ticked, quotes, tables and code blocks as blocks;
/// bold, italics, strikethrough, underline, highlights, colours, code, tags and links inside a
/// line. A `[[wiki link]]` is a tappable link to the note it names, or muted text when there is no
/// such note yet.
///
/// The inline marks are read by the same rules as the desktop's (`parse_inline_spans` in
/// `src/markdown.rs`) and drawn with explicit fonts: the system's own Markdown would ask the
/// note font for a bold or an italic face it does not have, so those marks would show as nothing.
struct MarkdownView: View {
    let text: String
    let links: [NoteLink]
    /// Called with the line (counting from 0) of a task whose box was tapped. Without it the
    /// boxes only show their state, as in a note that can only be watched.
    var onToggle: ((Int) -> Void)?
    let open: (String) -> Void
    @Environment(\.palette) private var palette

    private enum Block {
        case heading(Int, String)
        case bullet(Int, String)
        case numbered(Int, String, String)
        case task(Int, Bool, String, Int)
        case quote(String)
        case code(String)
        case rule
        case table([[String]])
        case paragraph(String)
    }

    // MARK: Blocks

    private func depth(of line: String) -> Int {
        var width = 0
        for ch in line {
            if ch == " " { width += 1 } else if ch == "\t" { width += 2 } else { break }
        }
        return width / 2
    }

    private var blocks: [Block] {
        var blocks: [Block] = []
        var code: [String]?
        var table: [String] = []
        func flushTable() {
            guard !table.isEmpty else { return }
            let rows = table.filter { !Self.isTableDivider($0) }.map(Self.cells)
            if rows.isEmpty { blocks.append(.paragraph(table.joined(separator: " "))) } else { blocks.append(.table(rows)) }
            table = []
        }
        for (number, raw) in text.components(separatedBy: "\n").enumerated() {
            let line = raw.replacingOccurrences(of: "\r", with: "")
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("```") {
                flushTable()
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
            if trimmed.hasPrefix("|") {
                table.append(trimmed)
                continue
            }
            flushTable()
            if trimmed.isEmpty { continue }
            let indent = depth(of: line)
            if trimmed == "---" || trimmed == "***" || trimmed == "___" {
                blocks.append(.rule)
            } else if let level = headingLevel(trimmed) {
                blocks.append(.heading(level, String(trimmed.drop { $0 == "#" }.dropFirst())))
            } else if let item = task(trimmed) {
                blocks.append(.task(indent, item.done, item.text, number))
            } else if trimmed.hasPrefix("- ") || trimmed.hasPrefix("* ") || trimmed.hasPrefix("+ ") {
                blocks.append(.bullet(indent, String(trimmed.dropFirst(2))))
            } else if let item = numberedItem(trimmed) {
                blocks.append(.numbered(indent, item.marker, item.text))
            } else if trimmed.hasPrefix(">") {
                blocks.append(.quote(String(trimmed.dropFirst()).trimmingCharacters(in: .whitespaces)))
            } else {
                blocks.append(.paragraph(trimmed))
            }
        }
        flushTable()
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

    /// `- [ ] text` or `- [x] text` (also with `*` or `+`).
    private func task(_ line: String) -> (done: Bool, text: String)? {
        guard let first = line.first, "-*+".contains(first) else { return nil }
        let rest = line.dropFirst()
        for (mark, done) in [(" [ ] ", false), (" [x] ", true), (" [X] ", true)] where rest.hasPrefix(mark) {
            return (done, String(rest.dropFirst(mark.count)))
        }
        for (mark, done) in [(" [ ]", false), (" [x]", true), (" [X]", true)] where rest == Substring(mark) {
            return (done, "")
        }
        return nil
    }

    /// `1. text` or `1) text`.
    private func numberedItem(_ line: String) -> (marker: String, text: String)? {
        let digits = line.prefix { $0.isNumber }
        guard !digits.isEmpty, digits.count <= 9 else { return nil }
        let rest = line.dropFirst(digits.count)
        guard let end = rest.first, end == "." || end == ")", rest.dropFirst().hasPrefix(" ") else { return nil }
        return ("\(digits)\(end)", String(rest.dropFirst(2)))
    }

    private static func cells(_ row: String) -> [String] {
        var parts = row.split(separator: "|", omittingEmptySubsequences: false).map {
            $0.trimmingCharacters(in: .whitespaces)
        }
        if parts.first == "" { parts.removeFirst() }
        if parts.last == "" { parts.removeLast() }
        return parts
    }

    private static func isTableDivider(_ row: String) -> Bool {
        let inner = row.filter { $0 != "|" && $0 != " " }
        return !inner.isEmpty && inner.contains("-") && inner.allSatisfy { $0 == "-" || $0 == ":" }
    }

    // MARK: Inline marks

    private struct Style {
        var bold = false
        var italic = false
        var strike = false
        var underline = false
        var color: Color?
        var highlight: Color?
    }

    private static let defaultHighlight = Color(red: 255 / 255, green: 196 / 255, blue: 77 / 255)

    private static func hexColor(_ hex: Substring) -> Color? {
        guard hex.count == 6, let value = UInt32(hex, radix: 16) else { return nil }
        return Color(
            red: Double((value >> 16) & 255) / 255, green: Double((value >> 8) & 255) / 255,
            blue: Double(value & 255) / 255)
    }

    private static func find(_ marker: String, in chars: [Character], from start: Int) -> Int? {
        let needle = Array(marker)
        guard start >= 0, chars.count - start >= needle.count else { return nil }
        var i = start
        while i + needle.count <= chars.count {
            if Array(chars[i..<(i + needle.count)]) == needle { return i }
            i += 1
        }
        return nil
    }

    private static func has(_ marker: String, in chars: [Character], at index: Int) -> Bool {
        let needle = Array(marker)
        return index + needle.count <= chars.count && Array(chars[index..<(index + needle.count)]) == needle
    }

    /// The links in `source`, as `elora://open/<id>` addresses for the notes they name.
    private func noteIDs() -> [String: String] {
        Dictionary(
            links.compactMap { link in link.id.map { (link.text.lowercased(), $0) } },
            uniquingKeysWith: { first, _ in first })
    }

    private func inline(_ source: String, size: CGFloat, bold: Bool = false) -> AttributedString {
        var out = AttributedString()
        var base = Style()
        base.bold = bold
        parse(Array(source), style: base, size: size, ids: noteIDs(), into: &out)
        return out
    }

    private func piece(_ text: String, _ style: Style, _ size: CGFloat) -> AttributedString {
        var run = AttributedString(text)
        var font = style.bold ? AppFont.bold(size) : AppFont.body(size)
        if style.italic { font = font.italic() }
        run.font = font
        if style.strike { run.strikethroughStyle = .single }
        if style.underline { run.underlineStyle = .single }
        if let color = style.color { run.foregroundColor = color }
        if let glow = style.highlight { run.backgroundColor = glow.opacity(0.4) }
        return run
    }

    private func parse(_ chars: [Character], style: Style, size: CGFloat, ids: [String: String], into out: inout AttributedString) {
        var buffer = ""
        func flush() {
            if !buffer.isEmpty {
                out.append(piece(buffer, style, size))
                buffer = ""
            }
        }
        /// Text between `marker` and its closing twin, parsed again with one more style.
        func nested(_ marker: String, at i: Int, _ change: (inout Style) -> Void) -> Int? {
            let width = marker.count
            guard let end = Self.find(marker, in: chars, from: i + width), end > i + width else { return nil }
            flush()
            var inner = style
            change(&inner)
            parse(Array(chars[(i + width)..<end]), style: inner, size: size, ids: ids, into: &out)
            return end + width
        }

        var i = 0
        while i < chars.count {
            let c = chars[i]
            // Inline code.
            if c == "`", let end = Self.find("`", in: chars, from: i + 1), end > i + 1 {
                flush()
                var run = AttributedString(String(chars[(i + 1)..<end]))
                run.font = AppFont.mono(size - 2)
                run.backgroundColor = palette.elevated
                out.append(run)
                i = end + 1
                continue
            }
            // Wiki link: [[Target]] or [[Target|Shown]].
            if Self.has("[[", in: chars, at: i), let end = Self.find("]]", in: chars, from: i + 2) {
                let inner = String(chars[(i + 2)..<end]).trimmingCharacters(in: .whitespaces)
                if !inner.isEmpty {
                    flush()
                    let parts = inner.split(separator: "|", maxSplits: 1).map { $0.trimmingCharacters(in: .whitespaces) }
                    let target = parts[0]
                    let shown = parts.count > 1 ? parts[1] : target
                    var run = piece(shown, style, size)
                    if let id = ids[target.lowercased()],
                       let encoded = id.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed),
                       let url = URL(string: "elora://open/\(encoded)")
                    {
                        run.link = url
                        run.foregroundColor = palette.accent
                    } else {
                        run.foregroundColor = palette.fgMuted
                    }
                    out.append(run)
                    i = end + 2
                    continue
                }
            }
            // A link: [text](address).
            if c == "[", let close = Self.find("](", in: chars, from: i + 1), let end = Self.find(")", in: chars, from: close + 2),
               close > i + 1, end > close + 2
            {
                let address = String(chars[(close + 2)..<end])
                if let url = URL(string: address), url.scheme != nil {
                    flush()
                    var run = piece(String(chars[(i + 1)..<close]), style, size)
                    run.link = url
                    run.foregroundColor = palette.accent
                    run.underlineStyle = .single
                    out.append(run)
                    i = end + 1
                    continue
                }
            }
            // Text colour: {color:#RRGGBB}text{/color}.
            if Self.has("{color:#", in: chars, at: i), i + 15 <= chars.count, chars[i + 14] == "}",
               let color = Self.hexColor(Substring(String(chars[(i + 8)..<(i + 14)]))),
               let end = Self.find("{/color}", in: chars, from: i + 15), end > i + 15
            {
                flush()
                var inner = style
                inner.color = color
                parse(Array(chars[(i + 15)..<end]), style: inner, size: size, ids: ids, into: &out)
                i = end + 8
                continue
            }
            // Highlight: ==text== or =={#RRGGBB}text==.
            if Self.has("==", in: chars, at: i), !Self.has("===", in: chars, at: i),
               let end = Self.find("==", in: chars, from: i + 2), end > i + 2
            {
                flush()
                var content = Array(chars[(i + 2)..<end])
                var glow = Self.defaultHighlight
                if content.count >= 9, content[0] == "{", content[1] == "#", content[8] == "}",
                   let color = Self.hexColor(Substring(String(content[2..<8])))
                {
                    glow = color
                    content = Array(content[9...])
                }
                var inner = style
                inner.highlight = glow
                parse(content, style: inner, size: size, ids: ids, into: &out)
                i = end + 2
                continue
            }
            if Self.has("***", in: chars, at: i), let next = nested("***", at: i, { $0.bold = true; $0.italic = true }) {
                i = next
                continue
            }
            if Self.has("**", in: chars, at: i), !Self.has("***", in: chars, at: i),
               let next = nested("**", at: i, { $0.bold = true })
            {
                i = next
                continue
            }
            if Self.has("~~", in: chars, at: i), let next = nested("~~", at: i, { $0.strike = true }) {
                i = next
                continue
            }
            if Self.has("__", in: chars, at: i), !Self.has("___", in: chars, at: i),
               let next = nested("__", at: i, { $0.underline = true })
            {
                i = next
                continue
            }
            // Italic: *text*, not touching spaces.
            if c == "*", i + 1 < chars.count, chars[i + 1] != "*", chars[i + 1] != " ",
               let end = Self.find("*", in: chars, from: i + 1), end > i + 1, chars[end - 1] != " ",
               let next = nested("*", at: i, { $0.italic = true })
            {
                i = next
                continue
            }
            // A #tag after a space or the start of the line.
            if c == "#", i + 1 < chars.count, chars[i + 1].isLetter,
               i == 0 || chars[i - 1].isWhitespace || chars[i - 1] == "(" || chars[i - 1] == "["
            {
                var end = i + 1
                while end < chars.count, chars[end].isLetter || chars[end].isNumber || "-_/".contains(chars[end]) { end += 1 }
                flush()
                var run = piece(String(chars[i..<end]), style, size)
                run.foregroundColor = palette.accent
                out.append(run)
                i = end
                continue
            }
            buffer.append(c)
            i += 1
        }
        flush()
    }

    // MARK: Drawing

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(Array(blocks.enumerated()), id: \.offset) { _, block in
                switch block {
                case .heading(let level, let value):
                    let size: CGFloat = level == 1 ? 26 : level == 2 ? 22 : level == 3 ? 19 : 17
                    Text(inline(value, size: size, bold: true))
                        .foregroundStyle(level <= 2 ? palette.accent : palette.fgPrimary)
                        .padding(.top, level <= 2 ? 6 : 2)
                case .bullet(let depth, let value):
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text("•").foregroundStyle(palette.accent)
                        Text(inline(value, size: 18))
                    }
                    .padding(.leading, CGFloat(depth) * 18)
                case .numbered(let depth, let marker, let value):
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text(marker).foregroundStyle(palette.accent)
                        Text(inline(value, size: 18))
                    }
                    .padding(.leading, CGFloat(depth) * 18)
                case .task(let depth, let done, let value, let line):
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Button {
                            onToggle?(line)
                        } label: {
                            Image(systemName: done ? "checkmark.square.fill" : "square")
                                .font(.system(size: 20))
                                .foregroundStyle(done ? palette.accent : palette.fgMuted)
                                .frame(minWidth: 30, minHeight: 28)
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .disabled(onToggle == nil)
                        .accessibilityLabel(done ? "Checked" : "Not checked")
                        // A finished item is greyed out and struck through, as on the desktop.
                        Text(inline(value, size: 18))
                            .strikethrough(done)
                            .foregroundStyle(done ? palette.fgMuted : palette.fgPrimary)
                    }
                    .padding(.leading, CGFloat(depth) * 18)
                case .quote(let value):
                    HStack(spacing: 10) {
                        Rectangle().fill(palette.accent).frame(width: 3)
                        Text(inline(value, size: 18)).foregroundStyle(palette.fgSecondary)
                    }
                case .code(let value):
                    Text(value)
                        .font(AppFont.mono(14))
                        .padding(10)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .background(palette.elevated, in: RoundedRectangle(cornerRadius: 8))
                case .rule:
                    Rectangle().fill(palette.border).frame(height: 1)
                case .table(let rows):
                    table(rows)
                case .paragraph(let value):
                    Text(inline(value, size: 18))
                }
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .font(AppFont.body(18))
        .lineSpacing(5)
        .foregroundStyle(palette.fgPrimary)
        .environment(
            \.openURL,
            OpenURLAction { url in
                guard url.scheme == "elora", url.host == "open" else { return .systemAction }
                open(url.lastPathComponent)
                return .handled
            })
    }

    /// The first row is the header, as in the desktop's tables.
    private func table(_ rows: [[String]]) -> some View {
        let columns = rows.map(\.count).max() ?? 0
        return ScrollView(.horizontal, showsIndicators: false) {
            Grid(alignment: .leading, horizontalSpacing: 0, verticalSpacing: 0) {
                ForEach(Array(rows.enumerated()), id: \.offset) { index, row in
                    GridRow {
                        ForEach(0..<columns, id: \.self) { column in
                            Text(inline(column < row.count ? row[column] : "", size: 16, bold: index == 0))
                                .padding(.horizontal, 10)
                                .padding(.vertical, 6)
                                .frame(minWidth: 70, alignment: .leading)
                                .background(index == 0 ? palette.elevated : Color.clear)
                                .overlay(Rectangle().stroke(palette.border, lineWidth: 0.5))
                        }
                    }
                }
            }
        }
    }
}
