use crate::icons::icon;
use crate::theme::Palette;
use egui::{
    text::{LayoutJob, TextFormat},
    Color32, CornerRadius, FontFamily, FontId, Margin, RichText, Sense, Stroke, Ui,
    Vec2,
};

/// Preset glow highlight colors available in the toolbar and right-click menu.
pub const HIGHLIGHT_PRESETS: &[(&str, [u8; 3])] = &[
    ("Amber Glow", [255, 196, 77]),
    ("Emerald Glow", [82, 224, 150]),
    ("Cyan Glow", [78, 205, 255]),
    ("Violet Glow", [188, 132, 255]),
    ("Rose Glow", [255, 112, 150]),
    ("Coral Glow", [255, 142, 94]),
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
    Tag(String),
    WikiLink {
        target: String,
        display: String,
    },
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
pub fn parse_inline_spans(input: &str) -> Vec<InlineSpan> {
    let mut spans = Vec::new();
    let mut plain_buf = String::new();
    let mut idx = 0;
    let len = input.len();

    let flush_plain = |buf: &mut String, out: &mut Vec<InlineSpan>| {
        if !buf.is_empty() {
            out.push(InlineSpan::Plain(std::mem::take(buf)));
        }
    };

    while idx < len {
        let rest = &input[idx..];

        // 1. Inline code: `code`
        if let Some(after) = rest.strip_prefix('`') {
            if let Some(end) = after.find('`') {
                if end > 0 {
                    flush_plain(&mut plain_buf, &mut spans);
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
                    flush_plain(&mut plain_buf, &mut spans);
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

        // 3. Highlight: =={#RRGGBB}text== or ==text==
        if let Some(after) = rest.strip_prefix("==") {
            if !after.starts_with('=') {
                if let Some(end) = after.find("==") {
                    if end > 0 {
                        let inner = &after[..end];
                        flush_plain(&mut plain_buf, &mut spans);
                        let (color, text) = if let Some(after_brace) = inner.strip_prefix("{#") {
                            if after_brace.len() >= 7 && after_brace.as_bytes()[6] == b'}' {
                                let hex = &after_brace[..6];
                                let col = parse_hex_color(hex);
                                (col, after_brace[7..].to_string())
                            } else {
                                (None, inner.to_string())
                            }
                        } else {
                            (None, inner.to_string())
                        };
                        spans.push(InlineSpan::Highlight { text, color });
                        idx += 2 + end + 2;
                        continue;
                    }
                }
            }
        }

        // 4. Bold + Italic: ***text***
        if let Some(after) = rest.strip_prefix("***") {
            if let Some(end) = after.find("***") {
                if end > 0 {
                    flush_plain(&mut plain_buf, &mut spans);
                    spans.push(InlineSpan::BoldItalic(after[..end].to_string()));
                    idx += 3 + end + 3;
                    continue;
                }
            }
        }

        // 5. Bold: **text**
        if let Some(after) = rest.strip_prefix("**") {
            if !after.starts_with('*') {
                if let Some(end) = after.find("**") {
                    if end > 0 {
                        flush_plain(&mut plain_buf, &mut spans);
                        spans.push(InlineSpan::Bold(after[..end].to_string()));
                        idx += 2 + end + 2;
                        continue;
                    }
                }
            }
        }

        // 6. Strikethrough: ~~text~~
        if let Some(after) = rest.strip_prefix("~~") {
            if let Some(end) = after.find("~~") {
                if end > 0 {
                    flush_plain(&mut plain_buf, &mut spans);
                    spans.push(InlineSpan::Strikethrough(after[..end].to_string()));
                    idx += 2 + end + 2;
                    continue;
                }
            }
        }

        // 7. Underline: __text__
        if let Some(after) = rest.strip_prefix("__") {
            if !after.starts_with('_') {
                if let Some(end) = after.find("__") {
                    if end > 0 {
                        flush_plain(&mut plain_buf, &mut spans);
                        spans.push(InlineSpan::Underline(after[..end].to_string()));
                        idx += 2 + end + 2;
                        continue;
                    }
                }
            }
        }

        // 8. Italic: *text*
        if let Some(after) = rest.strip_prefix('*') {
            if !after.starts_with('*') && !after.starts_with(' ') {
                if let Some(end) = after.find('*') {
                    if end > 0 && !after[..end].ends_with(' ') {
                        flush_plain(&mut plain_buf, &mut spans);
                        spans.push(InlineSpan::Italic(after[..end].to_string()));
                        idx += 1 + end + 1;
                        continue;
                    }
                }
            }
        }

        // 9. Inline #tag (must be preceded by start-of-line or whitespace)
        if rest.starts_with('#') {
            let prev_ok = idx == 0
                || input[..idx]
                    .chars()
                    .next_back()
                    .is_some_and(|c| c.is_whitespace() || c == '(' || c == '[');
            if prev_ok {
                let after_hash = &rest[1..];
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
                            flush_plain(&mut plain_buf, &mut spans);
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

    flush_plain(&mut plain_buf, &mut spans);
    spans
}

/// Renders parsed inline spans inside a `horizontal_wrapped` UI row, including glowing highlights
/// and clickable `[[Wiki Links]]`.
pub fn render_inline_spans_ui(
    ui: &mut Ui,
    text: &str,
    font_size: f32,
    p: &Palette,
    _is_dark: bool,
    default_color: Color32,
    clicked_wiki_link: &mut Option<String>,
) {
    let spans = parse_inline_spans(text);
    ui.spacing_mut().item_spacing.x = 0.0;

    for span in spans {
        match span {
            InlineSpan::Plain(s) => {
                ui.label(RichText::new(s).size(font_size).color(default_color));
            }
            InlineSpan::Bold(s) => {
                bold_label(ui, &s, font_size + 0.5, p.fg_primary);
            }
            InlineSpan::Italic(s) => {
                ui.label(RichText::new(s).italics().size(font_size).color(default_color));
            }
            InlineSpan::BoldItalic(s) => {
                bold_label(ui, &s, font_size + 0.5, p.fg_primary);
            }
            InlineSpan::Strikethrough(s) => {
                ui.label(
                    RichText::new(s)
                        .strikethrough()
                        .size(font_size)
                        .color(p.fg_muted),
                );
            }
            InlineSpan::Underline(s) => {
                ui.label(
                    RichText::new(s)
                        .underline()
                        .size(font_size)
                        .color(p.fg_primary),
                );
            }
            InlineSpan::Code(s) => {
                ui.add_space(2.0);
                egui::Frame::new()
                    .fill(p.bg_elevated)
                    .stroke(Stroke::new(1.0_f32, p.border))
                    .corner_radius(CornerRadius::same(4))
                    .inner_margin(Margin::symmetric(5, 1))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(s)
                                .monospace()
                                .size(font_size - 1.0)
                                .color(p.accent),
                        );
                    });
                ui.add_space(2.0);
            }
            InlineSpan::Highlight { text, color } => {
                let glow_col = color.unwrap_or(Color32::from_rgb(255, 196, 77));
                render_glowing_highlight(ui, &text, font_size, glow_col, p.bg_canvas, p.glow_strength);
            }
            InlineSpan::Tag(tag) => {
                ui.label(RichText::new(tag).size(font_size).color(p.accent));
            }
            InlineSpan::WikiLink { target, display } => {
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

/// Draws a multi-layer neon/ambient glowing highlight pill behind the text and renders luminous text on top.
pub fn render_glowing_highlight(
    ui: &mut Ui,
    text: &str,
    font_size: f32,
    glow_color: Color32,
    bg: Color32,
    glow_strength: f32,
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

    let font_id = FontId::new(font_size, FontFamily::Proportional);
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_string(), font_id, text_col);

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
    let xs = axis(text_rect.left(), text_rect.right(), outer.left(), outer.right());
    let ys = axis(text_rect.top(), text_rect.bottom(), outer.top(), outer.bottom());

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

    let syntax_dim = Color32::from_rgba_unmultiplied(
        p.fg_muted.r(),
        p.fg_muted.g(),
        p.fg_muted.b(),
        140,
    );

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
pub fn apply_highlight_selection(
    text: &mut String,
    range: Option<(usize, usize)>,
    rgb: [u8; 3],
) {
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
            InlineSpan::Highlight { text, .. } => out.push_str(&text),
            InlineSpan::WikiLink { display, .. } => out.push_str(&display),
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
            InlineSpan::Strikethrough(t) => out.push_str(&format!("<del>{}</del>", html_escape(&t))),
            InlineSpan::Underline(t) => out.push_str(&format!("<u>{}</u>", html_escape(&t))),
            InlineSpan::Code(t) => out.push_str(&format!("<code>{}</code>", html_escape(&t))),
            InlineSpan::Highlight { text, .. } => {
                out.push_str(&format!("<mark>{}</mark>", html_escape(&text)))
            }
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

        if let Some(item) = trimmed.strip_prefix("- [ ] ").or_else(|| trimmed.strip_prefix("- [x] ")).or_else(|| trimmed.strip_prefix("- [X] ")) {
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

        if let Some(item) = trimmed.strip_prefix("- ").or_else(|| trimmed.strip_prefix("* ")) {
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
}
