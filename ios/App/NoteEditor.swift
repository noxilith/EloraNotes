import SwiftUI
import UIKit

/// Text the editor is told to show: when `id` changes, `text` replaces what is there, keeping the
/// caret with the words around it. Used for a note loading, and for somebody else's typing.
struct EditorText: Equatable {
    var id: Int
    var text: String
}

/// The Markdown editor. The text is styled as it is typed the way the desktop editor styles it,
/// somebody else's typing is folded in without moving your caret, and other people's carets are
/// drawn where they are, in their colour, with their name.
struct NoteEditor: UIViewRepresentable {
    let incoming: EditorText
    let people: [LivePerson]
    let palette: Palette
    let editable: Bool
    let onEdit: (String) -> Void
    let onCursor: (Int) -> Void

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeUIView(context: Context) -> CaretTextView {
        let view = CaretTextView()
        view.delegate = context.coordinator
        view.backgroundColor = .clear
        view.textContainerInset = UIEdgeInsets(top: 12, left: 12, bottom: 28, right: 12)
        view.alwaysBounceVertical = true
        view.keyboardDismissMode = .interactive
        view.autocapitalizationType = .sentences
        // Markdown is full of marks that autocorrect and smart punctuation would "fix" (two dashes,
        // straight quotes, double brackets). With them off the keyboard's empty suggestions strip
        // goes too, so the formatting bar sits directly on the keys. Misspelt words stay underlined.
        view.autocorrectionType = .no
        view.smartQuotesType = .no
        view.smartDashesType = .no
        view.spellCheckingType = .yes
        view.onLayout = { [weak coordinator = context.coordinator] in coordinator?.placeCarets() }
        context.coordinator.view = view
        if DemoData.screen == "edit" {
            DispatchQueue.main.async { view.becomeFirstResponder() }
        }
        view.keyboardAppearance = palette.isDark ? .dark : .light
        let bar = FormatBar { [weak coordinator = context.coordinator] button in coordinator?.press(button) }
        bar.apply(palette)
        view.inputAccessoryView = bar
        context.coordinator.bar = bar
        return view
    }

    func updateUIView(_ view: CaretTextView, context: Context) {
        let coordinator = context.coordinator
        coordinator.parent = self
        view.isEditable = editable
        view.keyboardAppearance = palette.isDark ? .dark : .light
        if coordinator.palette.canvas != palette.canvas || coordinator.palette.accent != palette.accent {
            coordinator.palette = palette
            coordinator.bar?.apply(palette)
            coordinator.restyle()
        }
        if incoming.id != coordinator.lastIncoming {
            coordinator.lastIncoming = incoming.id
            coordinator.apply(incoming.text)
        }
        coordinator.people = people
        coordinator.placeCarets()
    }

    final class Coordinator: NSObject, UITextViewDelegate {
        var parent: NoteEditor
        weak var view: CaretTextView?
        var bar: FormatBar?
        var palette: Palette
        var lastIncoming = -1
        var people: [LivePerson] = []
        /// Somebody's typing that arrived while a word was still being composed (an accent, a
        /// Japanese or Chinese word): it waits for the word to be finished.
        private var waiting: String?
        private var quiet = false
        private var carets: [Int: PeerCaret] = [:]

        init(_ parent: NoteEditor) {
            self.parent = parent
            self.palette = parent.palette
        }

        // MARK: Text in and out

        func textViewDidChange(_ textView: UITextView) {
            guard !quiet else { return }
            restyle()
            parent.onEdit(textView.text)
            if textView.markedTextRange == nil, let text = waiting {
                waiting = nil
                apply(text)
            }
            placeCarets()
        }

        func textViewDidChangeSelection(_ textView: UITextView) {
            guard !quiet else { return }
            let upTo = (textView.text as NSString).substring(to: textView.selectedRange.location)
            parent.onCursor(upTo.utf8.count)
        }

        /// Shows `new` in place of the text, changing only what differs, so the caret and the
        /// scroll position stay with the words they were next to.
        func apply(_ new: String) {
            guard let view else { return }
            if view.markedTextRange != nil {
                waiting = new
                return
            }
            let old = view.text ?? ""
            if old == new { restyle(); return }
            let (removed, inserted) = Self.diff(old, new)
            let insertedLength = (inserted as NSString).length
            func carry(_ place: Int) -> Int {
                if place <= removed.location { return place }
                if place >= removed.location + removed.length { return place - removed.length + insertedLength }
                return removed.location + insertedLength
            }
            let selection = view.selectedRange
            let from = carry(selection.location)
            let to = carry(selection.location + selection.length)
            quiet = true
            view.textStorage.replaceCharacters(in: removed, with: inserted)
            view.selectedRange = NSRange(location: from, length: max(0, to - from))
            quiet = false
            restyle()
            placeCarets()
        }

        /// What differs between two texts: the range of the old text that goes, and what replaces it.
        static func diff(_ old: String, _ new: String) -> (removed: NSRange, inserted: String) {
            let before = old as NSString
            let after = new as NSString
            let shortest = min(before.length, after.length)
            var start = 0
            while start < shortest, before.character(at: start) == after.character(at: start) {
                start += 1
            }
            // Never split a character made of two UTF-16 units.
            if start > 0, UTF16.isLeadSurrogate(before.character(at: start - 1)) { start -= 1 }
            var tail = 0
            while tail < shortest - start,
                before.character(at: before.length - 1 - tail) == after.character(at: after.length - 1 - tail)
            {
                tail += 1
            }
            if tail > 0, UTF16.isTrailSurrogate(before.character(at: before.length - tail)) { tail -= 1 }
            return (
                NSRange(location: start, length: before.length - start - tail),
                after.substring(with: NSRange(location: start, length: after.length - start - tail))
            )
        }

        /// A formatting button was pressed: the change comes from the shared Rust code, and goes in
        /// as an edit, so it can be undone and is sent to everyone else like typing.
        func press(_ button: FormatButton) {
            guard let view, view.isEditable, view.markedTextRange == nil else { return }
            let selection = view.selectedRange
            let done = formatText(
                text: view.text, start: UInt32(selection.location),
                end: UInt32(selection.location + selection.length), button: button)
            guard done.text != view.text else { return }
            let (removed, inserted) = Self.diff(view.text, done.text)
            if let from = view.position(from: view.beginningOfDocument, offset: removed.location),
               let to = view.position(from: from, offset: removed.length),
               let range = view.textRange(from: from, to: to)
            {
                view.replace(range, withText: inserted)
            }
            view.selectedRange = NSRange(location: Int(done.start), length: Int(done.end - done.start))
            // Whether or not the text view tells its delegate about this, it is now what it is.
            textViewDidChange(view)
            textViewDidChangeSelection(view)
        }

        // MARK: Styling

        private func face(_ name: String, _ size: CGFloat) -> UIFont {
            UIFont(name: name, size: size) ?? UIFont.systemFont(ofSize: size)
        }

        private var body: UIFont { face("Ubuntu-Light", 18) }

        private var paragraph: NSParagraphStyle {
            let style = NSMutableParagraphStyle()
            style.lineSpacing = 5
            return style
        }

        private func ui(_ color: Color) -> UIColor { UIColor(color) }

        /// Styles the whole text. It is short enough that this is quicker than working out what
        /// changed, and it keeps every rule in one place.
        func restyle() {
            guard let view else { return }
            let storage = view.textStorage
            let whole = NSRange(location: 0, length: storage.length)
            let text = storage.string
            quiet = true
            let selection = view.selectedRange
            storage.beginEditing()
            storage.setAttributes(
                [.font: body, .foregroundColor: ui(palette.fgPrimary), .paragraphStyle: paragraph], range: whole)
            let dim = ui(palette.fgMuted).withAlphaComponent(0.7)

            func each(_ pattern: String, _ options: NSRegularExpression.Options = [], _ use: (NSTextCheckingResult) -> Void) {
                guard let regex = try? NSRegularExpression(pattern: pattern, options: options) else { return }
                for match in regex.matches(in: text, range: whole) { use(match) }
            }
            each(#"^(#{1,3}) .*$"#, .anchorsMatchLines) { match in
                let level = match.range(at: 1).length
                let size: CGFloat = level == 1 ? 26 : level == 2 ? 22 : 19
                storage.addAttributes(
                    [.font: face("Ubuntu-Light", size),
                     .foregroundColor: ui(level == 3 ? palette.fgPrimary : palette.accent)],
                    range: match.range)
            }
            each(#"^\s*(?:[-*+] (?:\[[ xX]\] )?|\d+\. )"#, .anchorsMatchLines) { match in
                storage.addAttribute(.foregroundColor, value: ui(palette.fgMuted), range: match.range)
            }
            each(#"^>.*$"#, .anchorsMatchLines) { match in
                storage.addAttribute(.foregroundColor, value: ui(palette.fgSecondary), range: match.range)
            }
            each(#"\*\*(.+?)\*\*"#) { match in
                storage.addAttribute(.font, value: face("DejaVuSans-Bold", 16), range: match.range(at: 1))
                storage.addAttribute(.foregroundColor, value: dim, range: NSRange(location: match.range.location, length: 2))
                storage.addAttribute(.foregroundColor, value: dim, range: NSRange(location: match.range.location + match.range.length - 2, length: 2))
            }
            each(#"(?<!\*)\*(?!\*)([^*\n]+?)(?<!\*)\*(?!\*)"#) { match in
                storage.addAttribute(.obliqueness, value: 0.22, range: match.range(at: 1))
                storage.addAttribute(.foregroundColor, value: dim, range: NSRange(location: match.range.location, length: 1))
                storage.addAttribute(.foregroundColor, value: dim, range: NSRange(location: match.range.location + match.range.length - 1, length: 1))
            }
            each(#"~~(.+?)~~"#) { match in
                storage.addAttribute(.strikethroughStyle, value: NSUnderlineStyle.single.rawValue, range: match.range(at: 1))
            }
            each(#"__(.+?)__"#) { match in
                storage.addAttribute(.underlineStyle, value: NSUnderlineStyle.single.rawValue, range: match.range(at: 1))
            }
            each(#"`([^`\n]+)`"#) { match in
                storage.addAttributes(
                    [.font: face("Hack-Regular", 15),
                     .backgroundColor: ui(palette.elevated)],
                    range: match.range)
            }
            each(#"\[\[[^\]\n]+\]\]"#) { match in
                storage.addAttribute(.foregroundColor, value: ui(palette.accent), range: match.range)
            }
            each(#"(?<![\w&])#[\p{L}\p{N}_/-]+"#) { match in
                storage.addAttribute(.foregroundColor, value: ui(palette.accent), range: match.range)
            }
            each(#"==\{#([0-9A-Fa-f]{6})\}(.+?)=="#) { match in
                if let hex = Range(match.range(at: 1), in: text), let color = Self.color(hex: String(text[hex])) {
                    storage.addAttribute(.backgroundColor, value: color.withAlphaComponent(0.35), range: match.range(at: 2))
                }
            }
            each(#"\{color:#([0-9A-Fa-f]{6})\}(.+?)\{/color\}"#) { match in
                if let hex = Range(match.range(at: 1), in: text), let color = Self.color(hex: String(text[hex])) {
                    storage.addAttribute(.foregroundColor, value: color, range: match.range(at: 2))
                }
            }
            storage.endEditing()
            view.typingAttributes = [.font: body, .foregroundColor: ui(palette.fgPrimary), .paragraphStyle: paragraph]
            if view.selectedRange != selection { view.selectedRange = selection }
            quiet = false
        }

        private static func color(hex: String) -> UIColor? {
            guard let value = UInt32(hex, radix: 16) else { return nil }
            return UIColor(
                red: CGFloat((value >> 16) & 255) / 255, green: CGFloat((value >> 8) & 255) / 255,
                blue: CGFloat(value & 255) / 255, alpha: 1)
        }

        // MARK: Other people's carets

        /// Where `byte` bytes into the text is, in UTF-16 units, as the text view counts.
        private func offset(ofByte byte: Int, in text: String) -> Int {
            let bytes = text.utf8
            var count = min(byte, bytes.count)
            while count > 0 {
                let index = bytes.index(bytes.startIndex, offsetBy: count)
                if let place = index.samePosition(in: text.utf16) {
                    return text.utf16.distance(from: text.utf16.startIndex, to: place)
                }
                count -= 1
            }
            return 0
        }

        func placeCarets() {
            guard let view else { return }
            let text = view.text ?? ""
            var alive: Set<Int> = []
            for (index, person) in people.enumerated() {
                guard let byte = person.cursor else { continue }
                alive.insert(index)
                let caret = carets[index] ?? {
                    let made = PeerCaret()
                    view.addSubview(made)
                    carets[index] = made
                    return made
                }()
                let place = offset(ofByte: Int(byte), in: text)
                guard let position = view.position(from: view.beginningOfDocument, offset: place) else { continue }
                let rect = view.caretRect(for: position)
                let color = UIColor(
                    red: CGFloat(person.red) / 255, green: CGFloat(person.green) / 255,
                    blue: CGFloat(person.blue) / 255, alpha: 1)
                caret.show(name: person.name, color: color, at: rect, typing: person.typing, within: view.bounds.width)
            }
            for (index, caret) in carets where !alive.contains(index) {
                caret.removeFromSuperview()
                carets[index] = nil
            }
        }
    }
}

/// A text view that says when it has been laid out again, so the carets can follow the text.
final class CaretTextView: UITextView {
    var onLayout: (() -> Void)?

    override func layoutSubviews() {
        super.layoutSubviews()
        onLayout?()
    }
}

/// One other person's caret: a thin bar in their colour with their name on a tag above it.
final class PeerCaret: UIView {
    private let bar = UIView()
    private let nameLabel = UILabel()

    override init(frame: CGRect) {
        super.init(frame: frame)
        isUserInteractionEnabled = false
        addSubview(bar)
        nameLabel.font = .systemFont(ofSize: 10, weight: .semibold)
        nameLabel.textColor = .white
        nameLabel.textAlignment = .center
        nameLabel.layer.cornerRadius = 4
        nameLabel.layer.masksToBounds = true
        addSubview(nameLabel)
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    func show(name: String, color: UIColor, at rect: CGRect, typing: Bool, within width: CGFloat) {
        bar.backgroundColor = color
        nameLabel.backgroundColor = color
        nameLabel.text = " \(name) "
        nameLabel.sizeToFit()
        var tagFrame = nameLabel.frame
        tagFrame.size.height = 15
        // The tag sits above the caret, or below it when the caret is on the first line.
        tagFrame.origin = CGPoint(x: 0, y: rect.minY < 18 ? rect.height + 1 : -16)
        let barWidth: CGFloat = typing ? 3 : 2
        frame = CGRect(x: min(rect.minX, max(0, width - tagFrame.width)), y: rect.minY, width: max(tagFrame.width, barWidth), height: rect.height)
        bar.frame = CGRect(x: rect.minX - frame.minX, y: 0, width: barWidth, height: rect.height)
        nameLabel.frame = tagFrame
    }
}
