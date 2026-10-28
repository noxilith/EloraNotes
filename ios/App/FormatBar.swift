import SwiftUI
import UIKit

/// The formatting buttons above the keyboard: the same set as the desktop editor's toolbar. Each
/// button asks the shared Rust code for the change, so it does exactly what the desktop's does.
final class FormatBar: UIView {
    private let scroll = UIScrollView()
    private let row = UIStackView()
    private let divider = UIView()
    private var buttons: [UIButton] = []
    private var accent: UIColor = .systemGreen
    private let press: (FormatButton) -> Void

    /// The desktop's default glow colour.
    private let glow: (UInt8, UInt8, UInt8) = (255, 196, 77)
    private let colours: [(String, UInt8, UInt8, UInt8)] = [
        ("Red", 229, 72, 77), ("Orange", 247, 107, 21), ("Yellow", 255, 197, 61),
        ("Green", 70, 167, 88), ("Cyan", 5, 162, 194), ("Blue", 62, 99, 221),
        ("Purple", 142, 78, 198), ("Pink", 214, 64, 159),
    ]

    init(press: @escaping (FormatButton) -> Void) {
        self.press = press
        super.init(frame: CGRect(x: 0, y: 0, width: 320, height: 50))
        autoresizingMask = .flexibleWidth

        scroll.showsHorizontalScrollIndicator = false
        scroll.translatesAutoresizingMaskIntoConstraints = false
        addSubview(scroll)
        row.axis = .horizontal
        row.spacing = 7
        row.alignment = .center
        row.translatesAutoresizingMaskIntoConstraints = false
        scroll.addSubview(row)
        divider.translatesAutoresizingMaskIntoConstraints = false
        addSubview(divider)
        NSLayoutConstraint.activate([
            scroll.leadingAnchor.constraint(equalTo: leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: trailingAnchor),
            scroll.topAnchor.constraint(equalTo: topAnchor),
            scroll.bottomAnchor.constraint(equalTo: bottomAnchor),
            row.leadingAnchor.constraint(equalTo: scroll.contentLayoutGuide.leadingAnchor, constant: 10),
            row.trailingAnchor.constraint(equalTo: scroll.contentLayoutGuide.trailingAnchor, constant: -10),
            row.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor, constant: 8),
            row.bottomAnchor.constraint(equalTo: scroll.contentLayoutGuide.bottomAnchor, constant: -4),
            row.heightAnchor.constraint(equalToConstant: 38),
            scroll.contentLayoutGuide.heightAnchor.constraint(equalTo: scroll.frameLayoutGuide.heightAnchor),
            divider.leadingAnchor.constraint(equalTo: leadingAnchor),
            divider.trailingAnchor.constraint(equalTo: trailingAnchor),
            divider.topAnchor.constraint(equalTo: topAnchor),
            divider.heightAnchor.constraint(equalToConstant: 1),
        ])

        let plain: [(String, FormatButton, String)] = [
            ("h-1", .heading1, "Heading 1"), ("h-2", .heading2, "Heading 2"),
            ("bold", .bold, "Bold"), ("italic", .italic, "Italic"),
            ("underline", .underline, "Underline"), ("strikethrough", .strikethrough, "Strikethrough"),
            ("code", .inlineCode, "Code"), ("list", .bullet, "Bullet list"),
            ("list-check", .checklist, "Checklist"), ("quote", .quote, "Quote"),
            ("link", .wikiLink, "Link to a note"), ("clear-formatting", .clearFormatting, "Clear formatting"),
        ]
        for (icon, button, label) in plain {
            add(icon: icon, label: label) { [weak self] in self?.press(button) }
        }
        let glowButton = add(icon: "highlight", label: "Glow highlight") { [weak self] in
            guard let self else { return }
            self.press(.highlight(red: self.glow.0, green: self.glow.1, blue: self.glow.2))
        }
        glowButton.layer.borderWidth = 1
        let colour = add(icon: "palette", label: "Text colour", action: nil)
        colour.showsMenuAsPrimaryAction = true
        colour.menu = UIMenu(
            title: "Text colour",
            children: colours.map { name, r, g, b in
                UIAction(
                    title: name,
                    image: UIImage(systemName: "circle.fill")?.withTintColor(
                        UIColor(red: CGFloat(r) / 255, green: CGFloat(g) / 255, blue: CGFloat(b) / 255, alpha: 1),
                        renderingMode: .alwaysOriginal)
                ) { [weak self] _ in self?.press(.textColor(red: r, green: g, blue: b)) }
            })
        colour.layer.borderWidth = 1
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    @discardableResult
    private func add(icon: String, label: String, action: (() -> Void)?) -> UIButton {
        let button = UIButton(type: .system)
        button.setTitle(Icon.named(icon), for: .normal)
        button.titleLabel?.font = UIFont(name: "tabler-icons", size: 20)
        button.layer.cornerRadius = 8
        button.accessibilityLabel = label
        button.widthAnchor.constraint(equalToConstant: 44).isActive = true
        button.heightAnchor.constraint(equalToConstant: 38).isActive = true
        if let action {
            button.addAction(UIAction { _ in action() }, for: .touchUpInside)
        }
        row.addArrangedSubview(button)
        buttons.append(button)
        return button
    }

    func apply(_ palette: Palette) {
        backgroundColor = UIColor(palette.sidebar)
        divider.backgroundColor = UIColor(palette.border)
        accent = UIColor(palette.accent)
        for button in buttons {
            button.backgroundColor = UIColor(palette.elevated)
            button.setTitleColor(UIColor(palette.fgSecondary), for: .normal)
            button.tintColor = UIColor(palette.fgSecondary)
            if button.layer.borderWidth > 0 { button.layer.borderColor = accent.withAlphaComponent(0.7).cgColor }
        }
    }

    override var intrinsicContentSize: CGSize { CGSize(width: UIView.noIntrinsicMetric, height: 50) }
}
