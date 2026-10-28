//! The text changes behind the formatting buttons: wrap the selection in `**`, turn a line into a
//! heading, highlight a word. It works on text and a selection and knows nothing about windows, so
//! the desktop editor and the phone's editor make exactly the same change when the same button is
//! pressed. Selections are counted in characters (Unicode scalar values).

/// Converts an RGB `[u8; 3]` color to `#RRGGBB` hex string.
pub fn rgb_to_hex(rgb: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
}

/// Converts a char index (`usize`) to a byte offset in `s`.
pub fn char_to_byte_idx(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(b, _)| b)
        .unwrap_or(s.len())
}

fn chars(s: &str) -> usize {
    s.chars().count()
}

fn ordered(range: (usize, usize)) -> (usize, usize) {
    (range.0.min(range.1), range.0.max(range.1))
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
    let _ = wrap_selection_at(text, range, prefix, suffix, placeholder);
}

/// [`wrap_selection`], saying where the selection should be afterwards: the words that were
/// wrapped or unwrapped, or the placeholder that was put in. `None` when there was no range.
pub fn wrap_selection_at(
    text: &mut String,
    range: Option<(usize, usize)>,
    prefix: &str,
    suffix: &str,
    placeholder: &str,
) -> Option<(usize, usize)> {
    let Some(range) = range else {
        let snippet = format!(" {prefix}{placeholder}{suffix}");
        text.push_str(&snippet);
        return None;
    };
    let (lo, hi) = ordered(range);
    let b_lo = char_to_byte_idx(text, lo);
    let b_hi = char_to_byte_idx(text, hi);

    if b_lo < b_hi {
        let selected = &text[b_lo..b_hi];
        // If selection itself is already wrapped with prefix/suffix, unwrap it
        if selected.starts_with(prefix)
            && selected.ends_with(suffix)
            && selected.len() >= prefix.len() + suffix.len()
        {
            let inner = selected[prefix.len()..selected.len() - suffix.len()].to_string();
            let inner_chars = chars(&inner);
            text.replace_range(b_lo..b_hi, &inner);
            return Some((lo, lo + inner_chars));
        }
        // Or if the surrounding text right outside [b_lo..b_hi] has prefix/suffix, unwrap that
        if b_lo >= prefix.len()
            && b_hi + suffix.len() <= text.len()
            && &text[b_lo - prefix.len()..b_lo] == prefix
            && &text[b_hi..b_hi + suffix.len()] == suffix
        {
            let inner = selected.to_string();
            let (before, inner_chars) = (chars(prefix), chars(&inner));
            text.replace_range(b_lo - prefix.len()..b_hi + suffix.len(), &inner);
            return Some((lo - before, lo - before + inner_chars));
        }

        let inner_chars = chars(selected);
        let wrapped = format!("{prefix}{selected}{suffix}");
        text.replace_range(b_lo..b_hi, &wrapped);
        return Some((lo + chars(prefix), lo + chars(prefix) + inner_chars));
    }
    // Empty selection with known cursor position
    let snippet = format!("{prefix}{placeholder}{suffix}");
    text.insert_str(b_lo, &snippet);
    Some((lo + chars(prefix), lo + chars(prefix) + chars(placeholder)))
}

/// Applies a colored glowing highlight `=={#RRGGBB}text==` to the selected character range.
/// If the selection is already highlighted (`==...==`), updates its color or unwraps if identical.
pub fn apply_highlight_selection(text: &mut String, range: Option<(usize, usize)>, rgb: [u8; 3]) {
    let _ = highlight_selection_at(text, range, rgb);
}

/// [`apply_highlight_selection`], saying where the selection should be afterwards.
pub fn highlight_selection_at(
    text: &mut String,
    range: Option<(usize, usize)>,
    rgb: [u8; 3],
) -> Option<(usize, usize)> {
    let hex = rgb_to_hex(rgb);
    let prefix = format!("=={{{hex}}}");
    let suffix = "==";

    let Some(range) = range else {
        let snippet = format!(" {prefix}highlighted{suffix}");
        text.push_str(&snippet);
        return None;
    };
    let (lo, hi) = ordered(range);
    let b_lo = char_to_byte_idx(text, lo);
    let b_hi = char_to_byte_idx(text, hi);

    if b_lo < b_hi {
        let selected = text[b_lo..b_hi].to_string();
        let clean = strip_highlight_markers(&selected);
        let clean_chars = chars(&clean);
        let wrapped = format!("{prefix}{clean}{suffix}");
        text.replace_range(b_lo..b_hi, &wrapped);
        return Some((lo + chars(&prefix), lo + chars(&prefix) + clean_chars));
    }
    let snippet = format!("{prefix}highlighted{suffix}");
    text.insert_str(b_lo, &snippet);
    Some((
        lo + chars(&prefix),
        lo + chars(&prefix) + chars("highlighted"),
    ))
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

/// Takes the formatting marks off a piece of text: `**bold**`, `*italic*`, `~~strike~~`,
/// `__underline__`, `` `code` ``, highlights, text colours and `[[links]]` all become the plain
/// words they hold.
pub fn strip_inline_markers(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while !rest.is_empty() {
        // A colour: {color:#RRGGBB}words{/color}
        if let Some(after) = rest.strip_prefix("{color:#") {
            if after.len() >= 7
                && after.as_bytes()[..6].iter().all(u8::is_ascii_hexdigit)
                && after.as_bytes()[6] == b'}'
            {
                let body = &after[7..];
                if let Some(end) = body.find("{/color}") {
                    out.push_str(&strip_inline_markers(&body[..end]));
                    rest = &body[end + "{/color}".len()..];
                    continue;
                }
            }
        }
        // A highlight: =={#RRGGBB}words== or ==words==
        if let Some(after) = rest.strip_prefix("==") {
            let body = match after.strip_prefix('{') {
                Some(brace)
                    if brace.len() >= 8
                        && brace.as_bytes()[0] == b'#'
                        && brace.as_bytes()[1..7].iter().all(u8::is_ascii_hexdigit)
                        && brace.as_bytes()[7] == b'}' =>
                {
                    &brace[8..]
                }
                _ => after,
            };
            if let Some(end) = body.find("==") {
                if end > 0 {
                    out.push_str(&strip_inline_markers(&body[..end]));
                    rest = &body[end + 2..];
                    continue;
                }
            }
        }
        // A link: [[Note]] or [[Note|shown]]
        if let Some(after) = rest.strip_prefix("[[") {
            if let Some(end) = after.find("]]") {
                let inside = &after[..end];
                out.push_str(inside.rsplit('|').next().unwrap_or(inside));
                rest = &after[end + 2..];
                continue;
            }
        }
        // Paired marks, longest first.
        let mut matched = false;
        for mark in ["***", "**", "__", "~~", "*", "`"] {
            if let Some(after) = rest.strip_prefix(mark) {
                if let Some(end) = after.find(mark) {
                    if end > 0 {
                        out.push_str(&strip_inline_markers(&after[..end]));
                        rest = &after[end + mark.len()..];
                        matched = true;
                        break;
                    }
                }
            }
        }
        if matched {
            continue;
        }
        let next = rest.chars().next().map_or(1, char::len_utf8);
        out.push_str(&rest[..next]);
        rest = &rest[next..];
    }
    out
}

/// Takes the formatting off the selected text. Returns where the selection is afterwards.
pub fn clear_formatting_at(
    text: &mut String,
    range: Option<(usize, usize)>,
) -> Option<(usize, usize)> {
    let (lo, hi) = ordered(range?);
    if lo == hi {
        return Some((lo, hi));
    }
    let (b_lo, b_hi) = (char_to_byte_idx(text, lo), char_to_byte_idx(text, hi));
    let cleaned = strip_inline_markers(&text[b_lo..b_hi]);
    let kept = chars(&cleaned);
    text.replace_range(b_lo..b_hi, &cleaned);
    Some((lo, lo + kept))
}

/// One of the formatting buttons.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op {
    Bold,
    Italic,
    Strikethrough,
    Underline,
    InlineCode,
    Highlight([u8; 3]),
    TextColor([u8; 3]),
    WikiLink,
    Heading1,
    Heading2,
    Bullet,
    Checklist,
    Quote,
    CodeFence,
    ClearFormatting,
}

/// The text after a button was pressed, and where the selection is.
#[derive(Clone, Debug, PartialEq)]
pub struct Applied {
    pub text: String,
    pub selection: (usize, usize),
}

/// Presses `op` with the selection at `range` (in characters; equal ends mean a caret). The same
/// changes the desktop editor's buttons make.
pub fn apply(op: Op, text: &str, range: (usize, usize)) -> Applied {
    let mut text = text.to_string();
    let some = Some(range);
    let selection = match op {
        Op::Bold => wrap_selection_at(&mut text, some, "**", "**", "bold text"),
        Op::Italic => wrap_selection_at(&mut text, some, "*", "*", "italic text"),
        Op::Strikethrough => wrap_selection_at(&mut text, some, "~~", "~~", "strikethrough"),
        Op::Underline => wrap_selection_at(&mut text, some, "__", "__", "underlined"),
        Op::InlineCode => wrap_selection_at(&mut text, some, "`", "`", "code"),
        Op::Highlight(rgb) => highlight_selection_at(&mut text, some, rgb),
        Op::TextColor(rgb) => {
            let open = format!("{{color:{}}}", rgb_to_hex(rgb));
            wrap_selection_at(&mut text, some, &open, "{/color}", "colored text")
        }
        Op::WikiLink => wrap_selection_at(&mut text, some, "[[", "]]", "Note Title"),
        Op::Heading1 => wrap_selection_at(&mut text, some, "\n# ", "\n", "Heading 1"),
        Op::Heading2 => wrap_selection_at(&mut text, some, "\n## ", "\n", "Heading 2"),
        Op::Bullet => wrap_selection_at(&mut text, some, "\n- ", "", "List item"),
        Op::Checklist => wrap_selection_at(&mut text, some, "\n- [ ] ", "", "Task item"),
        Op::Quote => wrap_selection_at(&mut text, some, "\n> ", "", "Quote"),
        Op::CodeFence => wrap_selection_at(&mut text, some, "\n```\n", "\n```", "code block"),
        Op::ClearFormatting => clear_formatting_at(&mut text, some),
    };
    Applied {
        text,
        selection: selection.unwrap_or(range),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(op: Op, text: &str, from: usize, to: usize) -> (String, String) {
        let done = apply(op, text, (from, to));
        let picked: String = done
            .text
            .chars()
            .skip(done.selection.0)
            .take(done.selection.1 - done.selection.0)
            .collect();
        (done.text, picked)
    }

    #[test]
    fn a_button_wraps_the_selection_and_keeps_it_selected() {
        assert_eq!(
            press(Op::Bold, "say hello now", 4, 9),
            ("say **hello** now".into(), "hello".into())
        );
        assert_eq!(
            press(Op::Italic, "say hello now", 4, 9),
            ("say *hello* now".into(), "hello".into())
        );
        assert_eq!(
            press(Op::Strikethrough, "ab", 0, 2),
            ("~~ab~~".into(), "ab".into())
        );
        assert_eq!(
            press(Op::Underline, "ab", 0, 2),
            ("__ab__".into(), "ab".into())
        );
        assert_eq!(
            press(Op::InlineCode, "ab", 0, 2),
            ("`ab`".into(), "ab".into())
        );
        assert_eq!(
            press(Op::WikiLink, "Spec", 0, 4),
            ("[[Spec]]".into(), "Spec".into())
        );
    }

    #[test]
    fn pressing_it_again_takes_the_marks_off() {
        let (once, picked) = press(Op::Bold, "say hello now", 4, 9);
        assert_eq!(picked, "hello");
        let again = apply(Op::Bold, &once, (6, 11));
        assert_eq!(again.text, "say hello now");
        assert_eq!(again.selection, (4, 9));
        // The same when the marks are inside the selection.
        let inside = apply(Op::Bold, "say **hello** now", (4, 13));
        assert_eq!(inside.text, "say hello now");
        assert_eq!(inside.selection, (4, 9));
    }

    #[test]
    fn with_nothing_selected_a_placeholder_goes_in_and_is_selected() {
        assert_eq!(
            press(Op::Bold, "ab", 1, 1),
            ("a**bold text**b".into(), "bold text".into())
        );
        assert_eq!(
            press(Op::WikiLink, "", 0, 0),
            ("[[Note Title]]".into(), "Note Title".into())
        );
    }

    #[test]
    fn the_line_buttons_make_the_changes_the_desktop_makes() {
        assert_eq!(apply(Op::Heading1, "", (0, 0)).text, "\n# Heading 1\n");
        assert_eq!(apply(Op::Heading2, "Title", (0, 5)).text, "\n## Title\n");
        assert_eq!(apply(Op::Bullet, "", (0, 0)).text, "\n- List item");
        assert_eq!(apply(Op::Checklist, "", (0, 0)).text, "\n- [ ] Task item");
        assert_eq!(apply(Op::Quote, "wise", (0, 4)).text, "\n> wise");
        assert_eq!(
            apply(Op::CodeFence, "x = 1", (0, 5)).text,
            "\n```\nx = 1\n```"
        );
    }

    #[test]
    fn highlight_and_colour_use_the_colour_given() {
        let (text, picked) = press(Op::Highlight([255, 200, 0]), "a key word", 2, 5);
        assert_eq!(text, "a =={#FFC800}key== word");
        assert_eq!(picked, "key");
        // Highlighting again changes the colour instead of stacking another.
        let again = apply(
            Op::Highlight([0, 128, 255]),
            &text,
            (2, 2 + "=={#FFC800}key==".chars().count()),
        );
        assert_eq!(again.text, "a =={#0080FF}key== word");
        let (coloured, picked) = press(Op::TextColor([255, 0, 0]), "red alert", 0, 3);
        assert_eq!(coloured, "{color:#FF0000}red{/color} alert");
        assert_eq!(picked, "red");
    }

    #[test]
    fn clearing_formatting_leaves_the_plain_words() {
        let marked = "**bold** and *it* ~~old~~ __u__ `c` ==hi== =={#FFC800}glow== {color:#FF0000}red{/color} [[Note|shown]]";
        assert_eq!(
            strip_inline_markers(marked),
            "bold and it old u c hi glow red shown"
        );
        let done = apply(Op::ClearFormatting, "x **bold** y", (2, 10));
        assert_eq!(done.text, "x bold y");
        assert_eq!(done.selection, (2, 6));
        // Nothing selected, nothing to clear.
        assert_eq!(apply(Op::ClearFormatting, "**a**", (1, 1)).text, "**a**");
        // A star that is not a pair stays.
        assert_eq!(strip_inline_markers("2 * 3 = 6"), "2 * 3 = 6");
    }

    #[test]
    fn selections_count_characters_not_bytes() {
        // Two emoji and an accent before the selection: positions are characters.
        let text = "😀é😀 word";
        let (made, picked) = press(Op::Bold, text, 4, 8);
        assert_eq!(made, "😀é😀 **word**");
        assert_eq!(picked, "word");
    }
}
