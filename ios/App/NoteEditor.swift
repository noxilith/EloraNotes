import SwiftUI
import UIKit

/// Text the editor is told to show: when `id` changes, `text` replaces what is there, keeping the
/// caret with the words around it. Used for a note loading, and for somebody else's typing.
struct EditorText: Equatable {
    var id: Int
    var text: String
}

/// Words to put in at the caret (a picture's Markdown, say), once: a new `id` is a new request.
struct EditorInsertion: Equatable {
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
    var insertion: EditorInsertion?
    /// Fetches a picture the text names (`attachments/…`), for showing it under its line.
    var loadPicture: ((String, @escaping (UIImage?) -> Void) -> Void)?

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
        view.onLayout = { [weak coordinator = context.coordinator] in
            coordinator?.placeCarets()
            coordinator?.placePictures()
        }
        context.coordinator.view = view
        // A tap on a task's box ticks it.
        let tap = UITapGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.tapped(_:)))
        tap.delegate = context.coordinator
        view.addGestureRecognizer(tap)
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
        if let insertion, insertion.id != coordinator.lastInsertion {
            coordinator.lastInsertion = insertion.id
            coordinator.insertAtCaret(insertion.text)
        }
        coordinator.people = people
        coordinator.placeCarets()
    }

    final class Coordinator: NSObject, UITextViewDelegate, UIGestureRecognizerDelegate {
        var parent: NoteEditor
        weak var view: CaretTextView?
        var bar: FormatBar?
        var palette: Palette
        var lastIncoming = -1
        var lastInsertion = 0
        /// Pictures the text names, by address, once they have loaded; and the ones being fetched.
        private var pictures: [String: UIImage] = [:]
        private var fetching: Set<String> = []
        private var pictureViews: [UIImageView] = []
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

        /// Puts `words` in at the caret (or over the selection) as an edit like typing, so it is
        /// undone, saved and sent to everybody the same way.
        func insertAtCaret(_ words: String) {
            guard let view, view.isEditable, view.markedTextRange == nil,
                  let range = view.selectedTextRange
            else { return }
            view.replace(range, withText: words)
            textViewDidChange(view)
            textViewDidChangeSelection(view)
        }

        // MARK: Task boxes

        func gestureRecognizer(
            _ gestureRecognizer: UIGestureRecognizer,
            shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer
        ) -> Bool { true }

        /// A tap on the `- [ ]` at the start of a task line flips it to `- [x]` and back, as an
        /// edit like any other (so it is undone, saved and sent to everybody like typing).
        @objc func tapped(_ tap: UITapGestureRecognizer) {
            guard let view, view.isEditable, view.markedTextRange == nil, tap.state == .ended else { return }
            var point = tap.location(in: view)
            point.x -= view.textContainerInset.left
            point.y -= view.textContainerInset.top
            let storage = view.textStorage
            guard storage.length > 0 else { return }
            let glyph = view.layoutManager.glyphIndex(for: point, in: view.textContainer)
            let rect = view.layoutManager.boundingRect(forGlyphRange: NSRange(location: glyph, length: 1), in: view.textContainer)
            guard rect.insetBy(dx: -22, dy: -8).contains(point) else { return }
            let index = view.layoutManager.characterIndexForGlyph(at: glyph)
            let text = storage.string as NSString
            let line = text.lineRange(for: NSRange(location: min(index, text.length - 1), length: 0))
            guard let regex = try? NSRegularExpression(pattern: #"^(\s*[-*+] )\[( |x|X)\]"#),
                  let match = regex.firstMatch(in: text as String, range: line)
            else { return }
            // The box and a little either side of it, since it is a small target.
            let box = NSRange(location: match.range(at: 1).location + match.range(at: 1).length - 1, length: 5)
            guard index >= box.location, index <= box.location + box.length else { return }
            let mark = match.range(at: 2)
            let now = text.substring(with: mark)
            guard let from = view.position(from: view.beginningOfDocument, offset: mark.location),
                  let to = view.position(from: from, offset: mark.length),
                  let range = view.textRange(from: from, to: to)
            else { return }
            let keep = view.selectedRange
            view.replace(range, withText: now == " " ? "x" : " ")
            view.selectedRange = keep
            textViewDidChange(view)
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
            each(#"^\s*[-*+] \[[xX]\] (.*)$"#, .anchorsMatchLines) { match in
                storage.addAttributes(
                    [.foregroundColor: ui(palette.fgMuted),
                     .strikethroughStyle: NSUnderlineStyle.single.rawValue],
                    range: match.range(at: 1))
            }
            each(#"^>.*$"#, .anchorsMatchLines) { match in
                storage.addAttribute(.foregroundColor, value: ui(palette.fgSecondary), range: match.range)
            }
            each(#"\*\*\*(.+?)\*\*\*"#) { match in
                storage.addAttributes(
                    [.font: face("DejaVuSans-Bold", 16), .obliqueness: 0.22], range: match.range(at: 1))
                storage.addAttribute(.foregroundColor, value: dim, range: NSRange(location: match.range.location, length: 3))
                storage.addAttribute(.foregroundColor, value: dim, range: NSRange(location: match.range.location + match.range.length - 3, length: 3))
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
            each(#"(?<![=\{])==(?![=\{])(.+?)(?<!=)=="#) { match in
                storage.addAttribute(
                    .backgroundColor, value: UIColor(red: 1, green: 0.77, blue: 0.3, alpha: 0.4), range: match.range(at: 1))
                storage.addAttribute(.foregroundColor, value: dim, range: NSRange(location: match.range.location, length: 2))
                storage.addAttribute(.foregroundColor, value: dim, range: NSRange(location: match.range.location + match.range.length - 2, length: 2))
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
            // A line that is only a picture shows the picture under it: room is made, and the
            // picture itself is drawn into that room by `placePictures`.
            each(Self.pictureLine, .anchorsMatchLines) { match in
                guard let src = Range(match.range(at: 1), in: text).map({ String(text[$0]) }) else { return }
                storage.addAttributes(
                    [.foregroundColor: dim, .font: face("Ubuntu-Light", 13)], range: match.range)
                guard let image = pictures[src] else {
                    fetch(src)
                    return
                }
                let style = NSMutableParagraphStyle()
                style.lineSpacing = 5
                style.paragraphSpacing = pictureSize(image, width: Self.width(in: text, match)).height + 14
                storage.addAttribute(.paragraphStyle, value: style, range: match.range)
            }
            storage.endEditing()
            view.typingAttributes = [.font: body, .foregroundColor: ui(palette.fgPrimary), .paragraphStyle: paragraph]
            if view.selectedRange != selection { view.selectedRange = selection }
            quiet = false
        }

        // MARK: Pictures

        /// A whole line that is `![alt](address)`, with an optional `{width=N}` after it.
        static let pictureLine = #"^!\[[^\]\n]*\]\(\s*<?([^)\s>]+)>?\s*\)(?:\{width=(\d+)\})?[ \t]*$"#

        private static func width(in text: String, _ match: NSTextCheckingResult) -> CGFloat? {
            guard match.range(at: 2).location != NSNotFound, let digits = Range(match.range(at: 2), in: text)
            else { return nil }
            return Double(text[digits]).map { CGFloat($0) }
        }

        /// The size a picture is drawn at: as wide as the text, or as the note asks, and no wider
        /// than the picture itself.
        private func pictureSize(_ image: UIImage, width: CGFloat?) -> CGSize {
            guard let view else { return .zero }
            let room = max(60, view.bounds.width - view.textContainerInset.left - view.textContainerInset.right
                - 2 * view.textContainer.lineFragmentPadding)
            let wide = min(room, width ?? image.size.width, image.size.width)
            guard image.size.width > 0 else { return .zero }
            return CGSize(width: wide, height: wide * image.size.height / image.size.width)
        }

        /// Asks for a picture once; when it arrives the text is styled again to make room for it.
        private func fetch(_ src: String) {
            guard !fetching.contains(src), let load = parent.loadPicture else { return }
            fetching.insert(src)
            load(src) { [weak self] image in
                DispatchQueue.main.async {
                    guard let self else { return }
                    if let image {
                        self.pictures[src] = image
                        self.restyle()
                        self.view?.setNeedsLayout()
                    } else {
                        // Not found (yet): it is asked for again the next time the note opens.
                    }
                }
            }
        }

        /// Draws each loaded picture in the room left under its line.
        func placePictures() {
            guard let view else { return }
            for old in pictureViews { old.removeFromSuperview() }
            pictureViews.removeAll()
            let text = view.textStorage.string
            guard let regex = try? NSRegularExpression(pattern: Self.pictureLine, options: .anchorsMatchLines)
            else { return }
            let whole = NSRange(location: 0, length: (text as NSString).length)
            for match in regex.matches(in: text, range: whole) {
                guard let src = Range(match.range(at: 1), in: text).map({ String(text[$0]) }),
                      let image = pictures[src]
                else { continue }
                let glyphs = view.layoutManager.glyphRange(forCharacterRange: match.range, actualCharacterRange: nil)
                let line = view.layoutManager.boundingRect(forGlyphRange: glyphs, in: view.textContainer)
                let size = pictureSize(image, width: Self.width(in: text, match))
                let shown = UIImageView(image: image)
                shown.contentMode = .scaleAspectFit
                shown.clipsToBounds = true
                shown.layer.cornerRadius = 6
                shown.isUserInteractionEnabled = false
                shown.frame = CGRect(
                    x: view.textContainerInset.left + view.textContainer.lineFragmentPadding,
                    y: view.textContainerInset.top + line.maxY + 6,
                    width: size.width, height: size.height)
                view.insertSubview(shown, at: 0)
                pictureViews.append(shown)
            }
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
