use crate::icons::icon;
use crate::theme::Palette;
use egui::{
    text::{LayoutJob, TextFormat},
    Color32, CornerRadius, FontFamily, FontId, Margin, RichText, Sense, Stroke, Ui, Vec2,
};

/// Preset glow highlight colors available in the toolbar and right-click menu.
pub const HIGHLIGHT_PRESETS: &[(&str, [u8; 3])] = &[
    ("Amber Glow", [255, 196, 77]),
    ("Emerald Glow", [82, 224, 150]),
    ("Cyan Glow", [78, 205, 255]),
    ("Rose Glow", [255, 112, 150]),
];

/// Converts an RGB `[u8; 3]` color to `#RRGGBB` hex string.
pub fn rgb_to_hex(rgb: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
}

/// Parses a `#RRGGBB` or `RRGGBB` hex string into `Color32`.
pub fn parse_hex_color(hex: &str) -> Option<Color32> {
    let h = hex.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&h[0..2], 16).ok()?;
    let g = u8::from_str_radix(&h[2..4], 16).ok()?;
    let b = u8::from_str_radix(&h[4..6], 16).ok()?;
    Some(Color32::from_rgb(r, g, b))
}

/// Brightens a color toward white so glowing text remains crisp and luminous.
pub fn luminous_text_color(base: Color32, is_dark_theme: bool) -> Color32 {
    let [r, g, b, _] = base.to_array();
    if is_dark_theme {
        // Blend 45% toward white for neon glow legibility on dark backgrounds
        let nr = r.saturating_add(((255 - r) as u16 * 45 / 100) as u8);
        let ng = g.saturating_add(((255 - g) as u16 * 45 / 100) as u8);
        let nb = b.saturating_add(((255 - b) as u16 * 45 / 100) as u8);
        Color32::from_rgb(nr, ng, nb)
    } else {
        // Darken slightly for crisp contrast on warm paper backgrounds while keeping rich hue
        let nr = (r as u16 * 55 / 100) as u8;
        let ng = (g as u16 * 55 / 100) as u8;
        let nb = (b as u16 * 55 / 100) as u8;
        Color32::from_rgb(nr, ng, nb)
    }
}

/// The default glow tint used by a bare `==highlight==`.
pub const DEFAULT_HIGHLIGHT_COLOR: Color32 = Color32::from_rgb(255, 196, 77);

/// Every style a run of text can carry at once. Markers nest, so a run may be bold *and*
/// highlighted, italic *and* underlined, and so on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SpanStyle {
    pub bold: bool,
    pub italic: bool,
    pub strikethrough: bool,
    pub underline: bool,
    /// Explicit text color from `{color:#RRGGBB}…{/color}`.
    pub color: Option<Color32>,
    /// Glow tint from `=={#RRGGBB}…==`, or [`DEFAULT_HIGHLIGHT_COLOR`] for a bare `==…==`.
    pub highlight: Option<Color32>,
}

impl SpanStyle {
    /// How many styles are applied. Single-style runs keep their simple span variants.
    fn layers(self) -> usize {
        usize::from(self.bold)
            + usize::from(self.italic)
            + usize::from(self.strikethrough)
            + usize::from(self.underline)
            + usize::from(self.color.is_some())
            + usize::from(self.highlight.is_some())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InlineSpan {
    Plain(String),
    Bold(String),
    Italic(String),
    BoldItalic(String),
    Strikethrough(String),
    Underline(String),
    Code(String),
    Highlight {
        text: String,
        color: Option<Color32>,
    },
    /// Text carrying more than one style at once, e.g. `**==bold glow==**`.
    Styled {
        text: String,
        style: SpanStyle,
    },
    Colored {
        text: String,
        color: Color32,
    },
    Tag(String),
    WikiLink {
        target: String,
        display: String,
    },
}

/// One piece of a rendered line: ordinary Markdown text, or a picture to draw on its own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinePiece {
    Text(String),
    /// `![alt](src)`: the picture GitHub would show, with its alt text and address. `width` is
    /// the size a `{width=320}` group after it asks for, which is how a picture keeps the size
    /// it was dragged to.
    Image {
        alt: String,
        src: String,
        width: Option<u32>,
    },
}

/// A `![alt](src)` picture found in a line, and where it sits in that line.
pub(crate) struct ImageMarker {
    /// Index of the `!` that starts the marker.
    pub start: usize,
    /// Index just past the closing `)`.
    pub body_end: usize,
    /// Index just past a `{…}` attribute group that follows the marker, when there is one.
    pub end: usize,
    pub alt: String,
    pub src: String,
    /// The width the attributes ask for, in points.
    pub width: Option<u32>,
}

/// Finds every `![alt](src){…}` picture in a line, in order. An empty address (`![]()`), a
/// half-written marker and an escaped one (`\![…`) are not pictures, so they are not returned.
pub(crate) fn image_markers(line: &str) -> Vec<ImageMarker> {
    let mut markers = Vec::new();
    let bytes = line.as_bytes();
    let mut from = 0usize;

    while let Some(offset) = line[from..].find("![") {
        let start = from + offset;
        // `\![…` is literal text, not a picture.
        if start > 0 && bytes[start - 1] == b'\\' {
            from = start + 2;
            continue;
        }
        let after = &line[start + 2..];
        let Some(close) = after.find(']') else { break };
        if !after[close + 1..].starts_with('(') {
            from = start + 2;
            continue;
        }
        let inside = &after[close + 2..];
        let Some(end) = inside.find(')') else { break };
        let target = inside[..end].trim();
        // `![alt](cat.png "a cat")`: the title after the address is not part of the path.
        let src = target
            .split_once(char::is_whitespace)
            .map(|(src, _)| src)
            .unwrap_or(target)
            .trim();
        if src.is_empty() {
            from = start + 2;
            continue;
        }
        let body_end = start + close + end + 5;
        let attributes = &line[body_end..];
        let (end_of_marker, width) = match attributes.starts_with('{') {
            true => match attributes.find('}') {
                Some(group_end) => (
                    body_end + group_end + 1,
                    width_attribute(&attributes[..=group_end]),
                ),
                None => (body_end, None),
            },
            false => (body_end, None),
        };
        markers.push(ImageMarker {
            start,
            body_end,
            end: end_of_marker,
            alt: after[..close].replace("\\]", "]"),
            src: src.to_string(),
            width,
        });
        from = end_of_marker;
    }

    markers
}

/// Splits a line into text and GitHub-style `![alt](src)` pictures. A line without pictures comes
/// back as a single piece of text, so rendering it is unchanged. An empty address (`![]()`) or a
/// half-written marker stays as text instead of turning into a broken picture.
pub fn split_images(line: &str) -> Vec<LinePiece> {
    let markers = image_markers(line);
    let mut pieces = Vec::new();
    let mut text_from = 0usize;

    for marker in markers {
        if marker.start > text_from {
            pieces.push(LinePiece::Text(line[text_from..marker.start].to_string()));
        }
        pieces.push(LinePiece::Image {
            alt: marker.alt,
            src: marker.src,
            width: marker.width,
        });
        text_from = marker.end;
    }

    if text_from < line.len() || pieces.is_empty() {
        pieces.push(LinePiece::Text(line[text_from..].to_string()));
    }
    pieces
}

/// The width in a `{width=320}` group, which may sit beside other attributes
/// (`{width=320 align=left}`) and may write its unit out (`{width=320px}`).
fn width_attribute(group: &str) -> Option<u32> {
    let inner = group.strip_prefix('{')?.strip_suffix('}')?;
    inner.split_whitespace().find_map(|attribute| {
        let (key, value) = attribute.split_once('=')?;
        if !key.eq_ignore_ascii_case("width") {
            return None;
        }
        let digits = value.trim().trim_end_matches("px");
        digits.parse::<u32>().ok().filter(|width| *width > 0)
    })
}

/// Sets the width of the `image_index`th picture in `markdown` (counting the same way the preview
/// draws them), keeping any other attributes it already carries. Returns the new text, or `None`
/// when there is no such picture or it already has that width.
pub fn set_image_width(markdown: &str, image_index: usize, width: u32) -> Option<String> {
    let mut out = String::with_capacity(markdown.len() + 16);
    let mut seen = 0usize;
    let mut changed = false;

    for line in markdown.split_inclusive('\n') {
        let without_newline = line.strip_suffix('\n').unwrap_or(line);
        let (content, carriage) = match without_newline.strip_suffix('\r') {
            Some(rest) => (rest, "\r"),
            None => (without_newline, ""),
        };
        let ending = if line.ends_with('\n') {
            format!("{carriage}\n")
        } else {
            String::new()
        };

        let markers = image_markers(content);
        let target = image_index.checked_sub(seen).filter(|i| *i < markers.len());
        match target {
            Some(i) => {
                let marker = &markers[i];
                let group = &content[marker.body_end..marker.end];
                let mut attributes: Vec<String> = group
                    .strip_prefix('{')
                    .and_then(|inner| inner.strip_suffix('}'))
                    .unwrap_or("")
                    .split_whitespace()
                    .filter(|attribute| {
                        !attribute
                            .split_once('=')
                            .is_some_and(|(key, _)| key.eq_ignore_ascii_case("width"))
                    })
                    .map(str::to_string)
                    .collect();
                attributes.push(format!("width={width}"));
                out.push_str(&content[..marker.body_end]);
                out.push('{');
                out.push_str(&attributes.join(" "));
                out.push('}');
                out.push_str(&content[marker.end..]);
                out.push_str(&ending);
                changed |= marker.width != Some(width);
                seen += i + 1;
                continue;
            }
            None => {
                seen += markers.len();
                out.push_str(line);
            }
        }
    }

    changed.then_some(out)
}

/// Parses inline Markdown spans from a single line of text:
/// - `***bold italic***`
/// - `**bold**`
/// - `=={#RRGGBB}custom glow highlight==` and `==highlight==`
/// - `~~strikethrough~~`
/// - `__underline__`
/// - `*italic*`
/// - `` `inline code` ``
/// - `[[Wiki Link]]` and `[[Target|Display]]`
/// - `#tag` tokens
///
/// Markers nest, so `**==bold inside a glow==**` and `==**glow bold**==` both come out as one
/// run that carries both styles.
pub fn parse_inline_spans(input: &str) -> Vec<InlineSpan> {
    let mut spans = Vec::new();
    parse_into(input, SpanStyle::default(), &mut spans);
    spans
}

/// Scans `input`, layering every marker it meets onto `style`.
fn parse_into(input: &str, style: SpanStyle, spans: &mut Vec<InlineSpan>) {
    let mut plain_buf = String::new();
    let mut idx = 0;
    let len = input.len();

    while idx < len {
        let rest = &input[idx..];

        // 1. Inline code: `code`
        if let Some(after) = rest.strip_prefix('`') {
            if let Some(end) = after.find('`') {
                if end > 0 {
                    flush_plain(&mut plain_buf, style, spans);
                    spans.push(InlineSpan::Code(after[..end].to_string()));
                    idx += 1 + end + 1;
                    continue;
                }
            }
        }

        // 2. Wiki link: [[Target|Alias]] or [[Target]]
        if let Some(after) = rest.strip_prefix("[[") {
            if let Some(end) = after.find("]]") {
                let inner = after[..end].trim();
                if !inner.is_empty() {
                    flush_plain(&mut plain_buf, style, spans);
                    let (target, display) = if let Some((t, a)) = inner.split_once('|') {
                        (t.trim().to_string(), a.trim().to_string())
                    } else {
                        (inner.to_string(), inner.to_string())
                    };
                    spans.push(InlineSpan::WikiLink { target, display });
                    idx += 2 + end + 2;
                    continue;
                }
            }
        }

        // 3. Text color: {color:#RRGGBB}text{/color}
        if let Some(after) = rest.strip_prefix("{color:#") {
            if after.len() >= 7 && after.as_bytes()[6] == b'}' {
                if let Some(color) = parse_hex_color(&after[..6]) {
                    let content = &after[7..];
                    if let Some(end) = content.find("{/color}") {
                        if end > 0 {
                            flush_plain(&mut plain_buf, style, spans);
                            let inner_style = SpanStyle {
                                color: Some(color),
                                ..style
                            };
                            parse_into(&content[..end], inner_style, spans);
                            idx += "{color:#".len() + 7 + end + "{/color}".len();
                            continue;
                        }
                    }
                }
            }
        }

        // 4. Highlight: =={#RRGGBB}text== or ==text==
        if let Some(after) = rest.strip_prefix("==") {
            if !after.starts_with('=') {
                if let Some(end) = after.find("==") {
                    if end > 0 {
                        let inner = &after[..end];
                        flush_plain(&mut plain_buf, style, spans);
                        let (color, text) = if let Some(after_brace) = inner.strip_prefix("{#") {
                            if after_brace.len() >= 7 && after_brace.as_bytes()[6] == b'}' {
                                (parse_hex_color(&after_brace[..6]), &after_brace[7..])
                            } else {
                                (None, inner)
                            }
                        } else {
                            (None, inner)
                        };
                        let inner_style = SpanStyle {
                            highlight: Some(color.unwrap_or(DEFAULT_HIGHLIGHT_COLOR)),
                            ..style
                        };
                        parse_into(text, inner_style, spans);
                        idx += 2 + end + 2;
                        continue;
                    }
                }
            }
        }

        // 5. Bold + Italic: ***text***
        if let Some(after) = rest.strip_prefix("***") {
            if let Some(end) = after.find("***") {
                if end > 0 {
                    flush_plain(&mut plain_buf, style, spans);
                    let inner_style = SpanStyle {
                        bold: true,
                        italic: true,
                        ..style
                    };
                    parse_into(&after[..end], inner_style, spans);
                    idx += 3 + end + 3;
                    continue;
                }
            }
        }

        // 6. Bold: **text**
        if let Some(after) = rest.strip_prefix("**") {
            if !after.starts_with('*') {
                if let Some(end) = after.find("**") {
                    if end > 0 {
                        flush_plain(&mut plain_buf, style, spans);
                        let inner_style = SpanStyle {
                            bold: true,
                            ..style
                        };
                        parse_into(&after[..end], inner_style, spans);
                        idx += 2 + end + 2;
                        continue;
                    }
                }
            }
        }

        // 7. Strikethrough: ~~text~~
        if let Some(after) = rest.strip_prefix("~~") {
            if let Some(end) = after.find("~~") {
                if end > 0 {
                    flush_plain(&mut plain_buf, style, spans);
                    let inner_style = SpanStyle {
                        strikethrough: true,
                        ..style
                    };
                    parse_into(&after[..end], inner_style, spans);
                    idx += 2 + end + 2;
                    continue;
                }
            }
        }

        // 8. Underline: __text__
        if let Some(after) = rest.strip_prefix("__") {
            if !after.starts_with('_') {
                if let Some(end) = after.find("__") {
                    if end > 0 {
                        flush_plain(&mut plain_buf, style, spans);
                        let inner_style = SpanStyle {
                            underline: true,
                            ..style
                        };
                        parse_into(&after[..end], inner_style, spans);
                        idx += 2 + end + 2;
                        continue;
                    }
                }
            }
        }

        // 9. Italic: *text*
        if let Some(after) = rest.strip_prefix('*') {
            if !after.starts_with('*') && !after.starts_with(' ') {
                if let Some(end) = after.find('*') {
                    if end > 0 && !after[..end].ends_with(' ') {
                        flush_plain(&mut plain_buf, style, spans);
                        let inner_style = SpanStyle {
                            italic: true,
                            ..style
                        };
                        parse_into(&after[..end], inner_style, spans);
                        idx += 1 + end + 1;
                        continue;
                    }
                }
            }
        }

        // 10. Inline #tag (must be preceded by start-of-line or whitespace)
        if rest.starts_with('#') {
            let prev_ok = idx == 0
                || input[..idx]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_whitespace() || c == '(' || c == '[');
            if prev_ok {
                let after_hash = rest.strip_prefix('#').unwrap_or(rest);
                if let Some(first_ch) = after_hash.chars().next() {
                    if first_ch.is_alphabetic() {
                        let tag_len: usize = after_hash
                            .chars()
                            .take_while(|c| {
                                c.is_alphanumeric() || *c == '-' || *c == '_' || *c == '/'
                            })
                            .map(|c| c.len_utf8())
                            .sum();
                        if tag_len > 0 {
                            flush_plain(&mut plain_buf, style, spans);
                            let tag_str = &rest[..1 + tag_len];
                            spans.push(InlineSpan::Tag(tag_str.to_string()));
                            idx += 1 + tag_len;
                            continue;
                        }
                    }
                }
            }
        }

        // Otherwise advance by one char
        if let Some(ch) = rest.chars().next() {
            plain_buf.push(ch);
            idx += ch.len_utf8();
        } else {
            break;
        }
    }

    flush_plain(&mut plain_buf, style, spans);
}

/// Pushes the buffered plain text as a span that keeps the styles collected so far.
fn flush_plain(buf: &mut String, style: SpanStyle, spans: &mut Vec<InlineSpan>) {
    if buf.is_empty() {
        return;
    }
    spans.push(styled_span(std::mem::take(buf), style));
}

/// Turns text plus its accumulated style into the narrowest span variant that fits.
fn styled_span(text: String, style: SpanStyle) -> InlineSpan {
    match style.layers() {
        0 => InlineSpan::Plain(text),
        1 if style.bold => InlineSpan::Bold(text),
        1 if style.italic => InlineSpan::Italic(text),
        1 if style.strikethrough => InlineSpan::Strikethrough(text),
        1 if style.underline => InlineSpan::Underline(text),
        1 if style.color.is_some() => InlineSpan::Colored {
            text,
            color: style.color.unwrap_or(Color32::WHITE),
        },
        1 => InlineSpan::Highlight {
            text,
            color: style.highlight.filter(|c| *c != DEFAULT_HIGHLIGHT_COLOR),
        },
        2 if style.bold && style.italic => InlineSpan::BoldItalic(text),
        _ => InlineSpan::Styled { text, style },
    }
}

/// Starts a new line in a wrapping row when `width` does not fit at the end of the current one.
///
/// `horizontal_wrapped` breaks lines from the width a widget reports up front. A chip that sizes
/// itself from the space left on the line therefore shrinks into that space (or overhangs it)
/// instead of moving down, so chips are measured here and the line is broken before them.
fn break_line_if_needed(ui: &mut Ui, width: f32) {
    // Neither `available_width` nor `available_size_before_wrap` reports the space left on the
    // line in a wrapping layout: both return the whole row. Measure from the cursor instead.
    let row = ui.max_rect();
    let cursor = ui.cursor().min.x;
    let remaining = (row.right() - cursor).max(0.0);
    // Only break when the line already has something on it; a chip wider than the whole row
    // cannot be placed by starting a new line either.
    if width > remaining && cursor > row.left() + 0.5 {
        ui.add_space(remaining);
    }
}

/// Renders parsed inline spans inside a `horizontal_wrapped` UI row, including glowing highlights
/// and clickable `[[Wiki Links]]`.
/// `bold` marks the line as a heading: its text is drawn in the bold face, and inline markers
/// still stack on top of that.
pub fn render_inline_spans_ui(
    ui: &mut Ui,
    text: &str,
    font_size: f32,
    p: &Palette,
    default_color: Color32,
    clicked_wiki_link: &mut Option<String>,
    bold: bool,
) {
    let spans = parse_inline_spans(text);
    ui.spacing_mut().item_spacing.x = 0.0;

    for span in spans {
        match span {
            InlineSpan::Plain(s) => {
                if bold {
                    bold_label(ui, &s, font_size, default_color);
                } else {
                    ui.label(RichText::new(s).size(font_size).color(default_color));
                }
            }
            InlineSpan::Bold(s) => {
                bold_label(ui, &s, font_size + 0.5, p.fg_primary);
            }
            InlineSpan::Italic(s) => {
                ui.label(styled_rich_text(
                    &s,
                    SpanStyle {
                        italic: true,
                        bold,
                        ..Default::default()
                    },
                    font_size,
                    default_color,
                ));
            }
            InlineSpan::BoldItalic(s) => {
                bold_label(ui, &s, font_size + 0.5, p.fg_primary);
            }
            InlineSpan::Strikethrough(s) => {
                ui.label(styled_rich_text(
                    &s,
                    SpanStyle {
                        strikethrough: true,
                        bold,
                        ..Default::default()
                    },
                    font_size,
                    p.fg_muted,
                ));
            }
            InlineSpan::Underline(s) => {
                ui.label(styled_rich_text(
                    &s,
                    SpanStyle {
                        underline: true,
                        bold,
                        ..Default::default()
                    },
                    font_size,
                    p.fg_primary,
                ));
            }
            InlineSpan::Colored { text, color } => {
                ui.label(styled_rich_text(
                    &text,
                    SpanStyle {
                        color: Some(color),
                        bold,
                        ..Default::default()
                    },
                    font_size,
                    color,
                ));
            }
            InlineSpan::Styled { text, style } => {
                let style = SpanStyle {
                    bold: style.bold || bold,
                    ..style
                };
                if let Some(glow) = style.highlight {
                    render_glowing_highlight(
                        ui,
                        &text,
                        font_size,
                        glow,
                        p.bg_canvas,
                        p.glow_strength,
                        style,
                    );
                } else {
                    ui.label(styled_rich_text(&text, style, font_size, default_color));
                }
            }
            InlineSpan::Code(s) => {
                // A chip is a nested frame, and a wrapping row cannot move one to the next
                // line. At the end of a line it used to be given the few remaining pixels as
                // its width, so its label wrapped one character per row and the chip spilled
                // out of the pane as a vertical stripe. Measure it first and break the line.
                let code_width = ui.fonts(|fonts| {
                    fonts
                        .layout_no_wrap(s.clone(), FontId::monospace(font_size - 1.0), p.accent)
                        .size()
                        .x
                }) + 14.0;
                break_line_if_needed(ui, code_width);
                ui.add_space(2.0);
                egui::Frame::new()
                    .fill(p.bg_elevated)
                    .stroke(Stroke::new(1.0_f32, p.border))
                    .corner_radius(CornerRadius::same(4))
                    .inner_margin(Margin::symmetric(5, 1))
                    .show(ui, |ui| {
                        // `extend` keeps the chip on one line even if it is wider than the
                        // column, instead of stacking its characters.
                        ui.add(
                            egui::Label::new(
                                RichText::new(s)
                                    .monospace()
                                    .size(font_size - 1.0)
                                    .color(p.accent),
                            )
                            .extend(),
                        );
                    });
                ui.add_space(2.0);
            }
            InlineSpan::Highlight { text, color } => {
                let glow_col = color.unwrap_or(DEFAULT_HIGHLIGHT_COLOR);
                render_glowing_highlight(
                    ui,
                    &text,
                    font_size,
                    glow_col,
                    p.bg_canvas,
                    p.glow_strength,
                    SpanStyle {
                        bold,
                        ..Default::default()
                    },
                );
            }
            InlineSpan::Tag(tag) => {
                ui.label(RichText::new(tag).size(font_size).color(p.accent));
            }
            InlineSpan::WikiLink { target, display } => {
                // A link chip is a button, so a wrapping row would keep it on the current line
                // and let it overhang the pane. Measure it and start a new line when needed.
                let label = format!("{} {display}", icon::LINK);
                let chip_width = ui.fonts(|fonts| {
                    fonts
                        .layout_no_wrap(label, FontId::proportional(font_size - 0.5), p.accent)
                        .size()
                        .x
                }) + 2.0 * ui.spacing().button_padding.x
                    + 4.0;
                break_line_if_needed(ui, chip_width);
                ui.add_space(2.0);
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new(format!("{} {display}", icon::LINK))
                                .size(font_size - 0.5)
                                .color(p.accent),
                        )
                        .fill(p.accent_soft)
                        .corner_radius(CornerRadius::same(5)),
                    )
                    .on_hover_text(format!("Open or create note: {target}"))
                    .clicked()
                {
                    *clicked_wiki_link = Some(target);
                }
                ui.add_space(2.0);
            }
        }
    }
}

/// Builds a one-section layout job so the same text can be bold, italic, struck through,
/// underlined and colored at once.
fn styled_layout_job(text: &str, font_size: f32, style: SpanStyle, color: Color32) -> LayoutJob {
    let family = if style.bold {
        crate::icons::bold_family()
    } else {
        FontFamily::Proportional
    };
    let stroke = Stroke::new(1.0_f32, color);
    let mut job = LayoutJob::default();
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: FontId::new(font_size, family),
            color,
            italics: style.italic,
            underline: if style.underline {
                stroke
            } else {
                Stroke::NONE
            },
            strikethrough: if style.strikethrough {
                stroke
            } else {
                Stroke::NONE
            },
            ..Default::default()
        },
    );
    job
}

/// A `RichText` carrying the flags of `style`, used for text that has no glow pill.
fn styled_rich_text(
    text: &str,
    style: SpanStyle,
    font_size: f32,
    default_color: Color32,
) -> RichText {
    let color = style.color.unwrap_or(default_color);
    let family = if style.bold {
        crate::icons::bold_family()
    } else {
        FontFamily::Proportional
    };
    let mut rt = RichText::new(text.to_string())
        .font(FontId::new(font_size, family))
        .color(color);
    if style.italic {
        rt = rt.italics();
    }
    if style.strikethrough {
        rt = rt.strikethrough();
    }
    if style.underline {
        rt = rt.underline();
    }
    rt
}

/// Draws a multi-layer neon/ambient glowing highlight pill behind the text and renders luminous
/// text on top, keeping any other styles the run carries (bold face, italics, strike, underline).
pub fn render_glowing_highlight(
    ui: &mut Ui,
    text: &str,
    font_size: f32,
    glow_color: Color32,
    bg: Color32,
    glow_strength: f32,
    style: SpanStyle,
) {
    let strength = glow_strength.clamp(0.0, 1.0);

    // The pill is a solid, fairly saturated tint of the glow hue. Text is picked for contrast
    // against that pill (near-white on dark tints, near-black on light tints) so the word stays
    // easy to read while the color still reads as a highlight.
    let text_col = if luma(bg) < 0.5 {
        mix_color(glow_color, Color32::WHITE, 0.7)
    } else {
        mix_color(glow_color, Color32::BLACK, 0.45)
    };
    let text_col = style.color.unwrap_or(text_col);

    let galley = ui
        .painter()
        .layout_job(styled_layout_job(text, font_size, style, text_col));

    // A highlighted run is laid out by hand, so a wrapping row would not move it to the next
    // line; break the line first when it does not fit.
    break_line_if_needed(ui, galley.size().x);

    // Reserve only the text's own line box, so the highlight never adds height to the line.
    // The glow is painted outward from this box without taking up layout space.
    let (text_rect, _resp) = ui.allocate_exact_size(galley.size(), Sense::hover());

    if ui.is_rect_visible(text_rect) {
        let painter = ui.painter();

        if strength > 0.0 {
            // The galley box includes line-height space above and below the glyphs; trim it so the
            // glow hugs the letters instead of extending past them.
            let glow_rect = text_rect.shrink2(egui::vec2(font_size * 0.06, font_size * 0.18));
            paint_soft_glow(painter, glow_rect, glow_color, strength);
        }

        painter.galley(text_rect.min, galley, text_col);
    }
}

/// Paints a feathered glow centered on `text_rect` as one gradient mesh. Alpha falls off
/// smoothly outward in both axes, so there are no stacked layers or visible bands.
fn paint_soft_glow(painter: &egui::Painter, text_rect: egui::Rect, color: Color32, strength: f32) {
    const SPREAD: f32 = 4.0;
    const STEPS: usize = 8;
    let peak = 0.42 * strength;
    let outer = text_rect.expand(SPREAD);

    // Coordinates along each axis: the outer band up to the text edge, then the outer band
    // from the far text edge. Alpha is constant inside the text, so linear interpolation is exact there.
    let axis = |inner_min: f32, inner_max: f32, outer_min: f32, outer_max: f32| {
        let mut v = Vec::with_capacity(2 * STEPS + 2);
        for k in 0..=STEPS {
            v.push(outer_min + (inner_min - outer_min) * k as f32 / STEPS as f32);
        }
        for k in 0..=STEPS {
            v.push(inner_max + (outer_max - inner_max) * k as f32 / STEPS as f32);
        }
        v
    };
    let xs = axis(
        text_rect.left(),
        text_rect.right(),
        outer.left(),
        outer.right(),
    );
    let ys = axis(
        text_rect.top(),
        text_rect.bottom(),
        outer.top(),
        outer.bottom(),
    );

    let mut mesh = egui::Mesh::default();
    for &y in &ys {
        for &x in &xs {
            let fx = edge_falloff(x, text_rect.left(), text_rect.right(), SPREAD);
            let fy = edge_falloff(y, text_rect.top(), text_rect.bottom(), SPREAD);
            let a = (peak * fx * fy * 255.0).round().clamp(0.0, 255.0) as u8;
            mesh.colored_vertex(
                egui::pos2(x, y),
                Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a),
            );
        }
    }

    let nx = xs.len();
    for j in 0..ys.len() - 1 {
        for i in 0..nx - 1 {
            let a = (j * nx + i) as u32;
            let b = a + 1;
            let c = a + nx as u32;
            let d = c + 1;
            mesh.add_triangle(a, b, d);
            mesh.add_triangle(a, d, c);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// 1.0 inside `[lo, hi]`, smoothly falling to 0.0 at `spread` distance outside it.
fn edge_falloff(c: f32, lo: f32, hi: f32, spread: f32) -> f32 {
    let d = (lo - c).max(0.0).max(c - hi);
    let t = (1.0 - d / spread).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Linear blend of two colors: `t = 0` gives `a`, `t = 1` gives `b`.
fn mix_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(lerp(a.r(), b.r()), lerp(a.g(), b.g()), lerp(a.b(), b.b()))
}

/// Perceived brightness in `0.0..=1.0`.
fn luma(c: Color32) -> f32 {
    (0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32) / 255.0
}

/// Draws bold text. egui ships only a regular font weight, so the glyphs are overdrawn at
/// sub-pixel offsets to thicken the strokes and make bold visibly heavier.
pub fn bold_label(ui: &mut Ui, text: &str, font_size: f32, color: Color32) {
    let font_id = FontId::new(font_size, crate::icons::bold_family());
    ui.label(RichText::new(text).font(font_id).color(color));
}

/// Builds a live syntax-styled `LayoutJob` for the Markdown editor (`TextEdit::multiline`)
/// so `**bold**`, `*italic*`, `~~strike~~`, `__underline__`, `` `code` ``, `# Heading`,
/// and `=={#HEX}highlight==` are visually styled directly while editing.
pub fn build_editor_layout_job(
    text: &str,
    wrap_width: f32,
    p: &Palette,
    is_dark: bool,
    default_highlight_rgb: [u8; 3],
) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;

    let body_font = FontId::new(15.0, FontFamily::Proportional);
    let bold_font = FontId::new(15.0, crate::icons::bold_family());
    let h1_font = FontId::new(21.0, FontFamily::Proportional);
    let h2_font = FontId::new(18.0, FontFamily::Proportional);
    let h3_font = FontId::new(16.0, FontFamily::Proportional);
    let mono_font = FontId::new(13.5, FontFamily::Monospace);

    let syntax_dim =
        Color32::from_rgba_unmultiplied(p.fg_muted.r(), p.fg_muted.g(), p.fg_muted.b(), 140);

    let lines: Vec<&str> = text.split('\n').collect();
    for (line_i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();

        if trimmed.starts_with("# ") {
            job.append(
                line,
                0.0,
                TextFormat {
                    font_id: h1_font.clone(),
                    color: p.accent,
                    ..Default::default()
                },
            );
        } else if trimmed.starts_with("## ") {
            job.append(
                line,
                0.0,
                TextFormat {
                    font_id: h2_font.clone(),
                    color: p.accent,
                    ..Default::default()
                },
            );
        } else if trimmed.starts_with("### ") {
            job.append(
                line,
                0.0,
                TextFormat {
                    font_id: h3_font.clone(),
                    color: p.fg_primary,
                    ..Default::default()
                },
            );
        } else {
            append_styled_editor_line(
                &mut job,
                line,
                &body_font,
                &bold_font,
                &mono_font,
                p,
                syntax_dim,
                is_dark,
                default_highlight_rgb,
            );
        }

        if line_i + 1 < lines.len() {
            job.append(
                "\n",
                0.0,
                TextFormat {
                    font_id: body_font.clone(),
                    color: p.fg_primary,
                    ..Default::default()
                },
            );
        }
    }

    if text.is_empty() {
        job.append(
            "",
            0.0,
            TextFormat {
                font_id: body_font,
                color: p.fg_primary,
                ..Default::default()
            },
        );
    }

    job
}

#[allow(clippy::too_many_arguments)]
fn append_styled_editor_line(
    job: &mut LayoutJob,
    line: &str,
    body_font: &FontId,
    bold_font: &FontId,
    mono_font: &FontId,
    p: &Palette,
    syntax_dim: Color32,
    is_dark: bool,
    default_highlight_rgb: [u8; 3],
) {
    let mut idx = 0;
    let len = line.len();
    let mut plain_start = 0;

    let flush_plain = |job: &mut LayoutJob, s: &str| {
        if !s.is_empty() {
            job.append(
                s,
                0.0,
                TextFormat {
                    font_id: body_font.clone(),
                    color: p.fg_primary,
                    ..Default::default()
                },
            );
        }
    };

    while idx < len {
        let rest = &line[idx..];

        // Text color: {color:#RRGGBB}text{/color}
        if let Some(after) = rest.strip_prefix("{color:#") {
            if after.len() >= 7 && after.as_bytes()[6] == b'}' {
                if let Some(col) = parse_hex_color(&after[..6]) {
                    let content = &after[7..];
                    if let Some(end) = content.find("{/color}") {
                        if end > 0 {
                            flush_plain(job, &line[plain_start..idx]);
                            let open_len = "{color:#".len() + 7;
                            job.append(
                                &rest[..open_len],
                                0.0,
                                TextFormat {
                                    font_id: body_font.clone(),
                                    color: syntax_dim,
                                    ..Default::default()
                                },
                            );
                            job.append(
                                &content[..end],
                                0.0,
                                TextFormat {
                                    font_id: body_font.clone(),
                                    color: col,
                                    ..Default::default()
                                },
                            );
                            job.append(
                                "{/color}",
                                0.0,
                                TextFormat {
                                    font_id: body_font.clone(),
                                    color: syntax_dim,
                                    ..Default::default()
                                },
                            );
                            idx += open_len + end + "{/color}".len();
                            plain_start = idx;
                            continue;
                        }
                    }
                }
            }
        }

        // Highlight: =={#RRGGBB}text== or ==text==
        if let Some(after) = rest.strip_prefix("==") {
            if !after.starts_with('=') {
                if let Some(end) = after.find("==") {
                    if end > 0 {
                        flush_plain(job, &line[plain_start..idx]);
                        let inner = &after[..end];
                        let (hl_color, prefix_len) =
                            if let Some(after_brace) = inner.strip_prefix("{#") {
                                if after_brace.len() >= 7 && after_brace.as_bytes()[6] == b'}' {
                                    let col = parse_hex_color(&after_brace[..6]).unwrap_or(
                                        Color32::from_rgb(
                                            default_highlight_rgb[0],
                                            default_highlight_rgb[1],
                                            default_highlight_rgb[2],
                                        ),
                                    );
                                    (col, 9) // "{#RRGGBB}".len()
                                } else {
                                    (
                                        Color32::from_rgb(
                                            default_highlight_rgb[0],
                                            default_highlight_rgb[1],
                                            default_highlight_rgb[2],
                                        ),
                                        0,
                                    )
                                }
                            } else {
                                (
                                    Color32::from_rgb(
                                        default_highlight_rgb[0],
                                        default_highlight_rgb[1],
                                        default_highlight_rgb[2],
                                    ),
                                    0,
                                )
                            };

                        let [r, g, b, _] = hl_color.to_array();
                        let bg = Color32::from_rgba_unmultiplied(
                            r,
                            g,
                            b,
                            if is_dark { 85 } else { 110 },
                        );
                        let fg = luminous_text_color(hl_color, is_dark);

                        // Opening marker (`==` or `=={#RRGGBB}`)
                        let open_len = 2 + prefix_len;
                        job.append(
                            &rest[..open_len],
                            0.0,
                            TextFormat {
                                font_id: body_font.clone(),
                                color: syntax_dim,
                                background: bg,
                                ..Default::default()
                            },
                        );
                        // Highlighted content
                        job.append(
                            &inner[prefix_len..],
                            0.0,
                            TextFormat {
                                font_id: bold_font.clone(),
                                color: fg,
                                background: bg,
                                ..Default::default()
                            },
                        );
                        // Closing `==`
                        job.append(
                            "==",
                            0.0,
                            TextFormat {
                                font_id: body_font.clone(),
                                color: syntax_dim,
                                background: bg,
                                ..Default::default()
                            },
                        );

                        idx += 2 + end + 2;
                        plain_start = idx;
                        continue;
                    }
                }
            }
        }

        // Bold: **text**
        if let Some(after) = rest.strip_prefix("**") {
            if !after.starts_with('*') {
                if let Some(end) = after.find("**") {
                    if end > 0 {
                        flush_plain(job, &line[plain_start..idx]);
                        job.append(
                            "**",
                            0.0,
                            TextFormat {
                                font_id: body_font.clone(),
                                color: syntax_dim,
                                ..Default::default()
                            },
                        );
                        job.append(
                            &after[..end],
                            0.0,
                            TextFormat {
                                font_id: bold_font.clone(),
                                color: if is_dark {
                                    Color32::WHITE
                                } else {
                                    Color32::BLACK
                                },
                                ..Default::default()
                            },
                        );
                        job.append(
                            "**",
                            0.0,
                            TextFormat {
                                font_id: body_font.clone(),
                                color: syntax_dim,
                                ..Default::default()
                            },
                        );
                        idx += 2 + end + 2;
                        plain_start = idx;
                        continue;
                    }
                }
            }
        }

        // Strikethrough: ~~text~~
        if let Some(after) = rest.strip_prefix("~~") {
            if let Some(end) = after.find("~~") {
                if end > 0 {
                    flush_plain(job, &line[plain_start..idx]);
                    job.append(
                        "~~",
                        0.0,
                        TextFormat {
                            font_id: body_font.clone(),
                            color: syntax_dim,
                            ..Default::default()
                        },
                    );
                    job.append(
                        &after[..end],
                        0.0,
                        TextFormat {
                            font_id: body_font.clone(),
                            color: p.fg_muted,
                            strikethrough: Stroke::new(1.2_f32, p.fg_muted),
                            ..Default::default()
                        },
                    );
                    job.append(
                        "~~",
                        0.0,
                        TextFormat {
                            font_id: body_font.clone(),
                            color: syntax_dim,
                            ..Default::default()
                        },
                    );
                    idx += 2 + end + 2;
                    plain_start = idx;
                    continue;
                }
            }
        }

        // Underline: __text__
        if let Some(after) = rest.strip_prefix("__") {
            if !after.starts_with('_') {
                if let Some(end) = after.find("__") {
                    if end > 0 {
                        flush_plain(job, &line[plain_start..idx]);
                        job.append(
                            "__",
                            0.0,
                            TextFormat {
                                font_id: body_font.clone(),
                                color: syntax_dim,
                                ..Default::default()
                            },
                        );
                        job.append(
                            &after[..end],
                            0.0,
                            TextFormat {
                                font_id: body_font.clone(),
                                color: p.fg_primary,
                                underline: Stroke::new(1.2_f32, p.accent),
                                ..Default::default()
                            },
                        );
                        job.append(
                            "__",
                            0.0,
                            TextFormat {
                                font_id: body_font.clone(),
                                color: syntax_dim,
                                ..Default::default()
                            },
                        );
                        idx += 2 + end + 2;
                        plain_start = idx;
                        continue;
                    }
                }
            }
        }

        // Italic: *text*
        if let Some(after) = rest.strip_prefix('*') {
            if !after.starts_with('*') && !after.starts_with(' ') {
                if let Some(end) = after.find('*') {
                    if end > 0 && !after[..end].ends_with(' ') {
                        flush_plain(job, &line[plain_start..idx]);
                        job.append(
                            "*",
                            0.0,
                            TextFormat {
                                font_id: body_font.clone(),
                                color: syntax_dim,
                                ..Default::default()
                            },
                        );
                        job.append(
                            &after[..end],
                            0.0,
                            TextFormat {
                                font_id: body_font.clone(),
                                color: p.fg_secondary,
                                italics: true,
                                ..Default::default()
                            },
                        );
                        job.append(
                            "*",
                            0.0,
                            TextFormat {
                                font_id: body_font.clone(),
                                color: syntax_dim,
                                ..Default::default()
                            },
                        );
                        idx += 1 + end + 1;
                        plain_start = idx;
                        continue;
                    }
                }
            }
        }

        // Inline code: `code`
        if let Some(after) = rest.strip_prefix('`') {
            if !after.starts_with('`') {
                if let Some(end) = after.find('`') {
                    if end > 0 {
                        flush_plain(job, &line[plain_start..idx]);
                        let full = &rest[..1 + end + 1];
                        job.append(
                            full,
                            0.0,
                            TextFormat {
                                font_id: mono_font.clone(),
                                color: p.accent,
                                background: p.bg_elevated,
                                ..Default::default()
                            },
                        );
                        idx += 1 + end + 1;
                        plain_start = idx;
                        continue;
                    }
                }
            }
        }

        if let Some(ch) = rest.chars().next() {
            idx += ch.len_utf8();
        } else {
            break;
        }
    }

    flush_plain(job, &line[plain_start..len]);
}

/// Converts a char index (`usize`) to a byte offset in `s`.
pub fn char_to_byte_idx(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

/// Inserts `insert` at the character range `range`, replacing any selected text.
/// Appends to the end of `text` when no range is known.
pub fn insert_at_selection(text: &mut String, range: Option<(usize, usize)>, insert: &str) {
    let (lo, hi) = match range {
        Some((a, b)) => (a.min(b), a.max(b)),
        None => {
            let n = text.chars().count();
            (n, n)
        }
    };
    let b_lo = char_to_byte_idx(text, lo);
    let b_hi = char_to_byte_idx(text, hi);
    text.replace_range(b_lo..b_hi, insert);
}

/// Wraps (or unwraps if already wrapped) the character range `[start_char..end_char]` with `prefix` and `suffix`.
/// If `start_char == end_char` (no selection), inserts `prefix + placeholder + suffix` at the cursor.
pub fn wrap_selection(
    text: &mut String,
    range: Option<(usize, usize)>,
    prefix: &str,
    suffix: &str,
    placeholder: &str,
) {
    if let Some((c_start, c_end)) = range {
        let (lo, hi) = if c_start <= c_end {
            (c_start, c_end)
        } else {
            (c_end, c_start)
        };
        let b_lo = char_to_byte_idx(text, lo);
        let b_hi = char_to_byte_idx(text, hi);

        if b_lo < b_hi {
            let selected = &text[b_lo..b_hi];
            // If selection itself is already wrapped with prefix/suffix, unwrap it
            if selected.starts_with(prefix)
                && selected.ends_with(suffix)
                && selected.len() >= prefix.len() + suffix.len()
            {
                let inner = &selected[prefix.len()..selected.len() - suffix.len()].to_string();
                text.replace_range(b_lo..b_hi, inner);
                return;
            }
            // Or if the surrounding text right outside [b_lo..b_hi] has prefix/suffix, unwrap that
            if b_lo >= prefix.len()
                && b_hi + suffix.len() <= text.len()
                && &text[b_lo - prefix.len()..b_lo] == prefix
                && &text[b_hi..b_hi + suffix.len()] == suffix
            {
                let inner = selected.to_string();
                text.replace_range(b_lo - prefix.len()..b_hi + suffix.len(), &inner);
                return;
            }

            let wrapped = format!("{prefix}{selected}{suffix}");
            text.replace_range(b_lo..b_hi, &wrapped);
            return;
        }
        // Empty selection with known cursor position
        let snippet = format!("{prefix}{placeholder}{suffix}");
        text.insert_str(b_lo, &snippet);
    } else {
        let snippet = format!(" {prefix}{placeholder}{suffix}");
        text.push_str(&snippet);
    }
}

/// Applies a colored glowing highlight `=={#RRGGBB}text==` to the selected character range.
/// If the selection is already highlighted (`==...==`), updates its color or unwraps if identical.
pub fn apply_highlight_selection(text: &mut String, range: Option<(usize, usize)>, rgb: [u8; 3]) {
    let hex = rgb_to_hex(rgb);
    let prefix = format!("=={{{hex}}}");
    let suffix = "==";

    if let Some((c_start, c_end)) = range {
        let (lo, hi) = if c_start <= c_end {
            (c_start, c_end)
        } else {
            (c_end, c_start)
        };
        let b_lo = char_to_byte_idx(text, lo);
        let b_hi = char_to_byte_idx(text, hi);

        if b_lo < b_hi {
            let selected = text[b_lo..b_hi].to_string();
            let clean = strip_highlight_markers(&selected);
            let wrapped = format!("{prefix}{clean}{suffix}");
            text.replace_range(b_lo..b_hi, &wrapped);
            return;
        }
        let snippet = format!("{prefix}highlighted{suffix}");
        text.insert_str(b_lo, &snippet);
    } else {
        let snippet = format!(" {prefix}highlighted{suffix}");
        text.push_str(&snippet);
    }
}

/// Strips markdown formatting markers (`**`, `*`, `~~`, `__`, `` ` ``, `=={#HEX}`, `==`) from the selected range.
pub fn clear_formatting_selection(text: &mut String, range: Option<(usize, usize)>) {
    let Some((c_start, c_end)) = range else {
        return;
    };
    let (lo, hi) = if c_start <= c_end {
        (c_start, c_end)
    } else {
        (c_end, c_start)
    };
    if lo == hi {
        return;
    }
    let b_lo = char_to_byte_idx(text, lo);
    let b_hi = char_to_byte_idx(text, hi);
    let selected = &text[b_lo..b_hi];
    let cleaned = strip_all_inline_markers(selected);
    text.replace_range(b_lo..b_hi, &cleaned);
}

fn strip_highlight_markers(s: &str) -> String {
    let mut out = s.trim();
    if let Some(inner) = out.strip_prefix("==").and_then(|r| r.strip_suffix("==")) {
        out = inner;
    }
    if let Some(after_brace) = out.strip_prefix("{#") {
        if after_brace.len() >= 7 && after_brace.as_bytes()[6] == b'}' {
            out = &after_brace[7..];
        }
    }
    out.to_string()
}

pub fn strip_all_inline_markers(s: &str) -> String {
    let spans = parse_inline_spans(s);
    let mut out = String::new();
    for sp in spans {
        match sp {
            InlineSpan::Plain(t)
            | InlineSpan::Bold(t)
            | InlineSpan::Italic(t)
            | InlineSpan::BoldItalic(t)
            | InlineSpan::Strikethrough(t)
            | InlineSpan::Underline(t)
            | InlineSpan::Code(t)
            | InlineSpan::Tag(t) => out.push_str(&t),
            InlineSpan::Highlight { text, .. } | InlineSpan::Colored { text, .. } => {
                out.push_str(&text)
            }
            InlineSpan::WikiLink { display, .. } => out.push_str(&display),
            InlineSpan::Styled { text, .. } => out.push_str(&text),
        }
    }
    out
}

/// Draws a circular glowing color swatch button used in the toolbar and right-click context menu.
pub fn color_swatch_button(
    ui: &mut Ui,
    rgb: [u8; 3],
    selected: bool,
    tooltip: &str,
) -> egui::Response {
    let size = Vec2::new(18.0, 18.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let center = rect.center();
        let col = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
        let halo = Color32::from_rgba_unmultiplied(rgb[0], rgb[1], rgb[2], 70);
        ui.painter().circle_filled(center, 9.5, halo);
        ui.painter().circle_filled(center, 7.0, col);
        if selected {
            ui.painter()
                .circle_stroke(center, 8.5, Stroke::new(1.6_f32, Color32::WHITE));
        }
    }
    resp.on_hover_text(tooltip)
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Renders a run that carries several styles as nested HTML tags.
fn styled_to_html(text: &str, style: SpanStyle) -> String {
    let mut out = html_escape(text);
    if style.bold {
        out = format!("<strong>{out}</strong>");
    }
    if style.italic {
        out = format!("<em>{out}</em>");
    }
    if style.underline {
        out = format!("<u>{out}</u>");
    }
    if style.strikethrough {
        out = format!("<del>{out}</del>");
    }
    if let Some(glow) = style.highlight {
        out = format!(
            "<mark style=\"background-color:{};border-radius:3px;padding:0 2px\">{out}</mark>",
            rgb_to_hex([glow.r(), glow.g(), glow.b()])
        );
    }
    if let Some(color) = style.color {
        out = format!(
            "<span style=\"color:{}\">{out}</span>",
            rgb_to_hex([color.r(), color.g(), color.b()])
        );
    }
    out
}

fn inline_to_html(line: &str) -> String {
    let mut out = String::new();
    for span in parse_inline_spans(line) {
        match span {
            InlineSpan::Plain(t) => out.push_str(&html_escape(&t)),
            InlineSpan::Bold(t) => out.push_str(&format!("<strong>{}</strong>", html_escape(&t))),
            InlineSpan::Italic(t) => out.push_str(&format!("<em>{}</em>", html_escape(&t))),
            InlineSpan::BoldItalic(t) => {
                out.push_str(&format!("<strong><em>{}</em></strong>", html_escape(&t)))
            }
            InlineSpan::Strikethrough(t) => {
                out.push_str(&format!("<del>{}</del>", html_escape(&t)))
            }
            InlineSpan::Underline(t) => out.push_str(&format!("<u>{}</u>", html_escape(&t))),
            InlineSpan::Code(t) => out.push_str(&format!("<code>{}</code>", html_escape(&t))),
            InlineSpan::Highlight { text, .. } => {
                out.push_str(&format!("<mark>{}</mark>", html_escape(&text)))
            }
            InlineSpan::Styled { text, style } => {
                out.push_str(&styled_to_html(&text, style));
            }
            InlineSpan::Colored { text, color } => out.push_str(&format!(
                "<span style=\"color:{}\">{}</span>",
                rgb_to_hex([color.r(), color.g(), color.b()]),
                html_escape(&text)
            )),
            InlineSpan::Tag(t) => {
                out.push_str(&format!("<span class=\"tag\">{}</span>", html_escape(&t)))
            }
            InlineSpan::WikiLink { display, .. } => out.push_str(&html_escape(&display)),
        }
    }
    out
}

/// Converts a note into a standalone HTML document. Handles headings, lists, task lists,
/// blockquotes, fenced code and the inline syntax understood by `parse_inline_spans`.
pub fn markdown_to_html(title: &str, body: &str) -> String {
    let mut out = String::new();
    out.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<title>");
    out.push_str(&html_escape(title));
    out.push_str("</title>\n<style>body{max-width:720px;margin:2rem auto;padding:0 1rem;font-family:system-ui,sans-serif;line-height:1.6;color:#222}mark{background:#ffe9a8}code{background:#f2f2f2;padding:0 .2em}pre{background:#f2f2f2;padding:.8rem;overflow:auto}blockquote{border-left:3px solid #ccc;margin:0;padding-left:1rem;color:#555}.tag{color:#2a6fdb}@media print{body{margin:0}}</style>\n</head>\n<body>\n<h1>");
    out.push_str(&html_escape(title));
    out.push_str("</h1>\n");

    let mut para: Vec<String> = Vec::new();
    let mut list_open = false;
    let mut quote_open = false;
    let mut in_code = false;

    fn flush_para(out: &mut String, para: &mut Vec<String>) {
        if !para.is_empty() {
            out.push_str(&format!("<p>{}</p>\n", para.join("<br>\n")));
            para.clear();
        }
    }

    for line in body.lines() {
        let trimmed = line.trim();
        if in_code {
            if trimmed.starts_with("```") {
                out.push_str("</code></pre>\n");
                in_code = false;
            } else {
                out.push_str(&html_escape(line));
                out.push('\n');
            }
            continue;
        }
        if trimmed.starts_with("```") {
            flush_para(&mut out, &mut para);
            if list_open {
                out.push_str("</ul>\n");
                list_open = false;
            }
            if quote_open {
                out.push_str("</blockquote>\n");
                quote_open = false;
            }
            out.push_str("<pre><code>");
            in_code = true;
            continue;
        }
        if trimmed.is_empty() {
            flush_para(&mut out, &mut para);
            if list_open {
                out.push_str("</ul>\n");
                list_open = false;
            }
            if quote_open {
                out.push_str("</blockquote>\n");
                quote_open = false;
            }
            continue;
        }

        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            flush_para(&mut out, &mut para);
            let text = inline_to_html(trimmed[hashes..].trim());
            out.push_str(&format!("<h{hashes}>{text}</h{hashes}>\n"));
            continue;
        }

        if let Some(item) = trimmed
            .strip_prefix("- [ ] ")
            .or_else(|| trimmed.strip_prefix("- [x] "))
            .or_else(|| trimmed.strip_prefix("- [X] "))
        {
            flush_para(&mut out, &mut para);
            if !list_open {
                out.push_str("<ul class=\"tasks\">\n");
                list_open = true;
            }
            let checked = trimmed.starts_with("- [x] ") || trimmed.starts_with("- [X] ");
            let box_attr = if checked { " checked" } else { "" };
            out.push_str(&format!(
                "<li><input type=\"checkbox\" disabled{box_attr}> {}</li>\n",
                inline_to_html(item)
            ));
            continue;
        }

        if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            flush_para(&mut out, &mut para);
            if !list_open {
                out.push_str("<ul>\n");
                list_open = true;
            }
            out.push_str(&format!("<li>{}</li>\n", inline_to_html(item)));
            continue;
        }

        if let Some(quote) = trimmed.strip_prefix("> ") {
            flush_para(&mut out, &mut para);
            if !quote_open {
                out.push_str("<blockquote>\n");
                quote_open = true;
            }
            out.push_str(&format!("<p>{}</p>\n", inline_to_html(quote)));
            continue;
        }

        if list_open {
            out.push_str("</ul>\n");
            list_open = false;
        }
        para.push(inline_to_html(trimmed));
    }

    if in_code {
        out.push_str("</code></pre>\n");
    }
    flush_para(&mut out, &mut para);
    if list_open {
        out.push_str("</ul>\n");
    }
    if quote_open {
        out.push_str("</blockquote>\n");
    }
    out.push_str("</body>\n</html>\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_inline_spans_bold_italic_highlight_and_links() {
        let line = "Hello **bold world**, *italic*, ~~strike~~, and =={#4ECDFF}neon glow== with [[My Note|Alias]] #rust!";
        let spans = parse_inline_spans(line);

        assert!(spans.contains(&InlineSpan::Bold("bold world".to_string())));
        assert!(spans.contains(&InlineSpan::Italic("italic".to_string())));
        assert!(spans.contains(&InlineSpan::Strikethrough("strike".to_string())));
        assert!(spans.contains(&InlineSpan::Highlight {
            text: "neon glow".to_string(),
            color: Some(Color32::from_rgb(0x4E, 0xCD, 0xFF)),
        }));
        assert!(spans.contains(&InlineSpan::WikiLink {
            target: "My Note".to_string(),
            display: "Alias".to_string(),
        }));
        assert!(spans.contains(&InlineSpan::Tag("#rust".to_string())));
    }

    #[test]
    fn test_markdown_to_html_structure_and_escaping() {
        let body = "# Heading\n\nSome **bold** & <tag> text\n\n- [x] done\n- [ ] todo\n\n> quoted\n\n```\n<raw>\n```";
        let html = markdown_to_html("Title <x>", body);
        assert!(html.contains("<title>Title &lt;x&gt;</title>"));
        assert!(html.contains("<h1>Heading</h1>"));
        assert!(html.contains("<strong>bold</strong>"));
        assert!(html.contains("Some <strong>bold</strong> &amp; &lt;tag&gt; text"));
        assert!(html.contains("checked> done"));
        assert!(html.contains("<li><input type=\"checkbox\" disabled> todo</li>"));
        assert!(html.contains("<blockquote>"));
        assert!(html.contains("<pre><code>&lt;raw&gt;\n</code></pre>"));
        assert!(html.trim_end().ends_with("</html>"));
    }

    #[test]
    fn test_parse_text_color_span() {
        let spans = parse_inline_spans("a {color:#FF0000}red{/color} b");
        assert_eq!(
            spans,
            vec![
                InlineSpan::Plain("a ".to_string()),
                InlineSpan::Colored {
                    text: "red".to_string(),
                    color: Color32::from_rgb(255, 0, 0),
                },
                InlineSpan::Plain(" b".to_string()),
            ]
        );
        assert_eq!(
            strip_all_inline_markers("{color:#00FF00}green{/color}"),
            "green"
        );
        // An unterminated or invalid marker stays plain text.
        assert_eq!(
            parse_inline_spans("{color:#FF0000}open"),
            vec![InlineSpan::Plain("{color:#FF0000}open".to_string())]
        );
    }

    #[test]
    fn test_stacked_inline_styles() {
        // Bold and a glow highlight stack, whichever marker comes first.
        for line in ["**==glow bold==**", "==**glow bold**=="] {
            let spans = parse_inline_spans(line);
            assert_eq!(spans.len(), 1, "{line}: {spans:?}");
            match &spans[0] {
                InlineSpan::Styled { text, style } => {
                    assert_eq!(text, "glow bold");
                    assert!(style.bold, "{line} should stay bold: {style:?}");
                    assert!(style.highlight.is_some(), "{line} should stay lit");
                }
                other => panic!("expected one stacked span for {line}, got {other:?}"),
            }
        }

        // Three layers at once: bold italic inside a custom highlight.
        let spans = parse_inline_spans("***=={#4ECDFF}neon==***");
        match &spans[0] {
            InlineSpan::Styled { text, style } => {
                assert_eq!(text, "neon");
                assert!(style.bold && style.italic);
                assert_eq!(style.highlight, Some(Color32::from_rgb(0x4E, 0xCD, 0xFF)));
            }
            other => panic!("expected a three-layer span, got {other:?}"),
        }

        // Text color on the outside, bold on the inside.
        let spans = parse_inline_spans("{color:#FF0000}**red**{/color}");
        match &spans[0] {
            InlineSpan::Styled { text, style } => {
                assert_eq!(text, "red");
                assert!(style.bold);
                assert_eq!(style.color, Some(Color32::from_rgb(255, 0, 0)));
            }
            other => panic!("expected color + bold, got {other:?}"),
        }

        // Strikethrough inside bold, with plain text around it.
        let spans = parse_inline_spans("a **~~gone~~** b");
        assert_eq!(spans.first(), Some(&InlineSpan::Plain("a ".to_string())));
        assert_eq!(spans.last(), Some(&InlineSpan::Plain(" b".to_string())));
        match &spans[1] {
            InlineSpan::Styled { text, style } => {
                assert_eq!(text, "gone");
                assert!(style.bold && style.strikethrough);
            }
            other => panic!("expected bold + strike, got {other:?}"),
        }

        // A single marker still yields the simple variant.
        assert_eq!(
            parse_inline_spans("**x**"),
            vec![InlineSpan::Bold("x".to_string())]
        );
        assert_eq!(
            parse_inline_spans("==x=="),
            vec![InlineSpan::Highlight {
                text: "x".to_string(),
                color: None,
            }]
        );

        // Stacked styles survive the HTML export.
        let html = markdown_to_html("t", "**==lit and bold==**");
        assert!(html.contains("<strong>"), "{html}");
        assert!(html.contains("<mark"), "{html}");
    }

    #[test]
    fn test_insert_at_selection() {
        let mut text = "Hello world".to_string();
        insert_at_selection(&mut text, Some((5, 5)), "\u{ec36}");
        assert_eq!(text, "Hello\u{ec36} world");

        // A selected range is replaced by the inserted text.
        insert_at_selection(&mut text, Some((0, 5)), "X");
        assert_eq!(text, "X\u{ec36} world");

        // Without a known cursor, the text is appended.
        insert_at_selection(&mut text, None, "!");
        assert_eq!(text, "X\u{ec36} world!");
    }

    #[test]
    fn test_wrap_and_unwrap_selection() {
        let mut text = "Make this word bold please".to_string();
        // "word" is at chars 10..14
        wrap_selection(&mut text, Some((10, 14)), "**", "**", "bold");
        assert_eq!(text, "Make this **word** bold please");

        // Wrapping "**word**" (chars 10..18) again unwraps it
        wrap_selection(&mut text, Some((10, 18)), "**", "**", "bold");
        assert_eq!(text, "Make this word bold please");
    }

    #[test]
    fn test_apply_highlight_and_clear_formatting() {
        let mut text = "Important glowing phrase".to_string();
        apply_highlight_selection(&mut text, Some((10, 17)), [255, 196, 77]);
        assert_eq!(text, "Important =={#FFC44D}glowing== phrase");

        let len_chars = text.chars().count();
        clear_formatting_selection(&mut text, Some((0, len_chars)));
        assert_eq!(text, "Important glowing phrase");
    }

    #[test]
    fn test_split_images_finds_pictures_and_keeps_text() {
        assert_eq!(
            split_images("plain line"),
            vec![LinePiece::Text("plain line".to_string())]
        );
        assert_eq!(
            split_images("![a cat](pics/cat.png)"),
            vec![LinePiece::Image {
                alt: "a cat".to_string(),
                src: "pics/cat.png".to_string(),
                width: None,
            }]
        );
        // The optional title after the address is not part of the path.
        assert_eq!(
            split_images("![cat](pics/cat.png \"A cat\")"),
            vec![LinePiece::Image {
                alt: "cat".to_string(),
                src: "pics/cat.png".to_string(),
                width: None,
            }]
        );
        // Text around a picture is kept on both sides.
        assert_eq!(
            split_images("see ![x](y.png) now"),
            vec![
                LinePiece::Text("see ".to_string()),
                LinePiece::Image {
                    alt: "x".to_string(),
                    src: "y.png".to_string(),
                    width: None,
                },
                LinePiece::Text(" now".to_string()),
            ]
        );
        // Two pictures on one line stay separate pictures.
        assert_eq!(split_images("![a](a.png) ![b](b.png)").len(), 3);
        // An empty address, an unfinished marker or an escaped marker is ordinary text.
        assert_eq!(
            split_images("![]()"),
            vec![LinePiece::Text("![]()".to_string())]
        );
        assert_eq!(
            split_images("![open](x"),
            vec![LinePiece::Text("![open](x".to_string())]
        );
        assert_eq!(
            split_images("\\![not](x.png)"),
            vec![LinePiece::Text("\\![not](x.png)".to_string())]
        );
    }

    #[test]
    fn test_split_images_reads_a_width_after_the_picture() {
        // The size a picture was dragged to lives in a group after it, where other Markdown
        // renderers simply ignore it.
        assert_eq!(
            split_images("![cat](cat.png){width=320}"),
            vec![LinePiece::Image {
                alt: "cat".to_string(),
                src: "cat.png".to_string(),
                width: Some(320),
            }]
        );
        // A unit and other attributes are allowed, and the text after the group stays text.
        assert_eq!(
            split_images("![cat](cat.png){width=320px align=left} after"),
            vec![
                LinePiece::Image {
                    alt: "cat".to_string(),
                    src: "cat.png".to_string(),
                    width: Some(320),
                },
                LinePiece::Text(" after".to_string()),
            ]
        );
        // A group without a usable width leaves the picture at its own size.
        assert_eq!(
            split_images("![cat](cat.png){align=left}"),
            vec![LinePiece::Image {
                alt: "cat".to_string(),
                src: "cat.png".to_string(),
                width: None,
            }]
        );
        assert_eq!(
            split_images("![cat](cat.png){width=wide}"),
            vec![LinePiece::Image {
                alt: "cat".to_string(),
                src: "cat.png".to_string(),
                width: None,
            }]
        );
    }

    #[test]
    fn test_set_image_width_writes_into_the_chosen_picture() {
        // No group yet: one is added after the marker.
        assert_eq!(
            set_image_width("before\n![cat](cat.png)\nafter", 0, 320),
            Some("before\n![cat](cat.png){width=320}\nafter".to_string())
        );
        // An existing width is replaced, and the other attributes are kept.
        assert_eq!(
            set_image_width("![cat](cat.png){width=200 align=left}", 0, 480),
            Some("![cat](cat.png){align=left width=480}".to_string())
        );
        // The title inside the address is left exactly as it was.
        assert_eq!(
            set_image_width("![cat](cat.png \"A cat\")", 0, 250),
            Some("![cat](cat.png \"A cat\"){width=250}".to_string())
        );
        // Only the picture at the chosen index changes.
        assert_eq!(
            set_image_width("![a](a.png) ![b](b.png)", 1, 120),
            Some("![a](a.png) ![b](b.png){width=120}".to_string())
        );
        // A trailing newline and Windows line endings survive.
        assert_eq!(
            set_image_width("![a](a.png)\r\n", 0, 90),
            Some("![a](a.png){width=90}\r\n".to_string())
        );
        // Asking for the width it already has, or for a picture that is not there, changes nothing.
        assert_eq!(set_image_width("![a](a.png){width=90}", 0, 90), None);
        assert_eq!(set_image_width("![a](a.png)", 3, 90), None);
        assert_eq!(set_image_width("plain text", 0, 90), None);
    }
}
