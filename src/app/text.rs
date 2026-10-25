//! Plain-text work the editor does to a note's body: indentation, carrying a list or quote
//! onto the next line, and finding a run of text in it.

/// One level of indentation, in spaces. Markdown nests lists and code with spaces, and a literal
/// tab would show up in the preview as an uneven gap.
pub const INDENT: &str = "    ";

/// The byte offset of a character index, or the end of the text when it is past that.
pub(super) fn char_to_byte(text: &str, index: usize) -> usize {
    text.char_indices()
        .nth(index)
        .map_or(text.len(), |(byte, _)| byte)
}

/// How far the line starting at `line_start` runs into leading spaces.
fn leading_spaces(body: &str, line_start: usize) -> usize {
    let line = &body[line_start..];
    let end = line.find('\n').unwrap_or(line.len());
    let line = &line[..end];
    line.len() - line.trim_start_matches(' ').len()
}

/// Adds one level of indentation to every line the selection touches, or takes one away when
/// `dedent` is set. With no selection the indent goes in at the caret and shift+Tab takes a level
/// off the front of the caret's line. Returns the new text and the character range to select.
pub fn indent_selection(
    body: &str,
    start: usize,
    end: usize,
    dedent: bool,
) -> (String, usize, usize) {
    let (start, end) = (start.min(end), start.max(end));
    let start_byte = char_to_byte(body, start);
    let end_byte = char_to_byte(body, end);
    let level = INDENT.chars().count();

    // No selection: one indent at the caret, or one level off the front of its line.
    if start == end {
        let line_start = body[..start_byte].rfind('\n').map_or(0, |at| at + 1);
        let line_start_char = body[..line_start].chars().count();
        if dedent {
            let take = leading_spaces(body, line_start).min(INDENT.len());
            let mut out = String::with_capacity(body.len());
            out.push_str(&body[..line_start]);
            out.push_str(&body[line_start + take..]);
            // The caret follows the text it was in, never stepping back past the line's start.
            let removed_before_caret = take.min(start - line_start_char);
            let caret = start - removed_before_caret;
            return (out, caret, caret);
        }
        let mut out = String::with_capacity(body.len() + INDENT.len());
        out.push_str(&body[..start_byte]);
        out.push_str(INDENT);
        out.push_str(&body[start_byte..]);
        let caret = start + level;
        return (out, caret, caret);
    }

    let first_line = body[..start_byte].rfind('\n').map_or(0, |at| at + 1);
    // A selection that stops at the very start of a line does not touch that line.
    let last_touched = if end_byte > first_line && body[..end_byte].ends_with('\n') {
        end_byte - 1
    } else {
        end_byte
    };
    let last_line = body[..last_touched].rfind('\n').map_or(0, |at| at + 1);
    let mut line_char = body[..first_line].chars().count();

    let mut out = String::with_capacity(body.len() + 8);
    out.push_str(&body[..first_line]);
    let mut at = first_line;
    // How far the start and the end of the selection have to travel. Every indented line
    // before them moves them on by a level; only the line the caret itself is on can hold it
    // back, when the caret sits among the spaces being taken away.
    let mut start_shift: isize = 0;
    let mut end_shift: isize = 0;
    let shifted = |delta: isize, room: usize| -> isize {
        if delta >= 0 {
            delta
        } else {
            -((-delta).min(room as isize))
        }
    };
    while at <= last_line && at < body.len() {
        let line_end = body[at..].find('\n').map_or(body.len(), |i| at + i + 1);
        let line = &body[at..line_end];
        let content = line.strip_suffix('\n').unwrap_or(line);
        let newline = &line[content.len()..];
        let delta: isize = if dedent {
            let take = (content.len() - content.trim_start_matches(' ').len()).min(INDENT.len());
            out.push_str(&content[take..]);
            -(take as isize)
        } else if content.is_empty() {
            // An empty line has nothing to step in, and indenting it would only leave trailing
            // spaces behind.
            out.push_str(content);
            0
        } else {
            out.push_str(INDENT);
            out.push_str(content);
            level as isize
        };
        if delta != 0 {
            if at <= start_byte {
                start_shift = shifted(delta, start - line_char);
            }
            if at < end_byte {
                end_shift += shifted(delta, end - line_char);
            }
        }
        out.push_str(newline);
        line_char += line.chars().count();
        at = line_end;
    }
    out.push_str(&body[at..]);

    let new_start = (start as isize + start_shift).max(0) as usize;
    let new_end = (end as isize + end_shift).max(new_start as isize) as usize;
    (out, new_start, new_end)
}

/// The live selection in the editor's own state as `(start, end, forward)`, or `fallback` when
/// the field has not stored a caret yet.
pub(super) fn caret_range(
    state: &egui::text_edit::TextEditState,
    fallback: usize,
) -> (usize, usize, bool) {
    state
        .cursor
        .char_range()
        .map(|range| {
            let (a, b) = (range.primary.index, range.secondary.index);
            (a.min(b), a.max(b), a <= b)
        })
        .unwrap_or((fallback, fallback, true))
}

/// The parts of a list item: its indentation, its marker (a bullet with any task box, or a
/// number and its punctuation, each with the space after it) and the text that follows. `None`
/// when the line is not a list item.
fn list_item(line: &str) -> Option<(&str, &str, &str)> {
    let indent_len = line.len() - line.trim_start_matches([' ', '\t']).len();
    let (indent, rest) = line.split_at(indent_len);

    for bullet in ["- ", "* ", "+ "] {
        if let Some(after) = rest.strip_prefix(bullet) {
            for task_box in ["[ ] ", "[x] ", "[X] "] {
                if let Some(content) = after.strip_prefix(task_box) {
                    let marker = bullet.len() + task_box.len();
                    return Some((indent, &rest[..marker], content));
                }
            }
            return Some((indent, &rest[..bullet.len()], after));
        }
    }

    let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits > 0 {
        let after_digits = &rest[digits..];
        let separator = after_digits
            .chars()
            .next()
            .filter(|c| *c == '.' || *c == ')');
        if let Some(separator) = separator {
            if let Some(content) = after_digits[separator.len_utf8()..].strip_prefix(' ') {
                let marker = digits + separator.len_utf8() + 1;
                return Some((indent, &rest[..marker], content));
            }
        }
    }
    None
}

/// The first `>` marker of a line — with up to three spaces before it and the space after it —
/// and what follows. One marker only: nesting is `> > `, and each later level is read as the
/// line is taken apart, so it can be kept or dropped on its own.
fn block_quote(line: &str) -> Option<(&str, &str)> {
    let bytes = line.as_bytes();
    let mut probe = 0;
    let mut spaces = 0;
    while probe < bytes.len() && bytes[probe] == b' ' && spaces < 3 {
        probe += 1;
        spaces += 1;
    }
    if probe >= bytes.len() || bytes[probe] != b'>' {
        return None;
    }
    probe += 1;
    if probe < bytes.len() && bytes[probe] == b' ' {
        probe += 1;
    }
    Some(line.split_at(probe))
}

/// The marker a list item hands to the line under it: the same bullet, a task back to unchecked,
/// and a number counted on.
fn next_marker(marker: &str) -> String {
    if marker.contains("[x]") || marker.contains("[X]") {
        marker.replace("[x]", "[ ]").replace("[X]", "[ ]")
    } else if marker.starts_with(|c: char| c.is_ascii_digit()) {
        let digits: String = marker.chars().take_while(|c| c.is_ascii_digit()).collect();
        let next = digits.parse::<u64>().unwrap_or(1).saturating_add(1);
        format!("{next}{}", &marker[digits.len()..])
    } else {
        marker.to_string()
    }
}

/// One leading piece of a line: a block quote's marker, or a list item's indentation and marker.
/// A line nests them, so `- > - item` starts with a bullet, then a quote, then another bullet.
#[derive(Debug, Clone, Copy)]
enum Piece<'a> {
    Quote(&'a str),
    Marker { indent: &'a str, marker: &'a str },
}

impl Piece<'_> {
    /// The piece exactly as it was written.
    fn text(self) -> String {
        match self {
            Self::Quote(quote) => quote.to_string(),
            Self::Marker { indent, marker } => format!("{indent}{marker}"),
        }
    }

    /// The same piece on the line underneath: a quote stays as it is, and a list item hands the
    /// next line its following marker.
    fn continued(self) -> String {
        match self {
            Self::Quote(quote) => quote.to_string(),
            Self::Marker { indent, marker } => format!("{indent}{}", next_marker(marker)),
        }
    }
}

/// Parts a line into the markers it starts with, however deeply they are nested, and the text
/// after them.
fn prefix_pieces(line: &str) -> (Vec<Piece<'_>>, &str) {
    let mut pieces = Vec::new();
    let mut rest = line;
    loop {
        if let Some((quote, after)) = block_quote(rest) {
            pieces.push(Piece::Quote(quote));
            rest = after;
            continue;
        }
        if let Some((indent, marker, content)) = list_item(rest) {
            pieces.push(Piece::Marker { indent, marker });
            rest = content;
            continue;
        }
        break;
    }
    (pieces, rest)
}

/// What pressing Enter on a line does to the line the caret is on.
pub enum LineBreak {
    /// A new line opens underneath it, starting with this text.
    Next(String),
    /// The line itself becomes this text and the caret moves to its end, because the innermost
    /// empty item or quote in it has been taken away rather than left sitting there.
    Here(String),
}

/// Works out the line Enter should leave behind: every quote marker and list marker the line is
/// nested in carries over, so a list inside a quote inside a list keeps going at every level,
/// with a done task coming back unchecked. Pressing Enter on an empty item or quote instead takes
/// away just that one level (`Here`), leaving whatever it was nested in. A line with no markers
/// gets a plain new line, keeping its indentation.
pub fn line_break(line: &str) -> LineBreak {
    let (pieces, content) = prefix_pieces(line);
    if pieces.is_empty() {
        let indent_len = line.len() - line.trim_start_matches([' ', '\t']).len();
        return LineBreak::Next(line[..indent_len].to_string());
    }

    if content.trim().is_empty() {
        // One level at a time: the innermost empty item or quote goes, and the caret lands at
        // the end of what it was nested in.
        let remaining: String = pieces[..pieces.len() - 1]
            .iter()
            .map(|piece| piece.text())
            .collect();
        return LineBreak::Here(remaining);
    }

    let last = pieces.len() - 1;
    let prefix: String = pieces
        .iter()
        .enumerate()
        .map(|(i, piece)| {
            if i == last {
                piece.continued()
            } else {
                piece.text()
            }
        })
        .collect();
    LineBreak::Next(prefix)
}

/// What a line that stays inside the same block should start with: the markers it is nested in,
/// with the innermost list marker turned into spaces as wide as it is, so the words line up under
/// the item's own and the line still belongs to it. A quote at the innermost level is simply
/// quoted again, and a line with no markers keeps its indentation.
pub fn continuation_line_prefix(line: &str) -> String {
    let (pieces, _) = prefix_pieces(line);
    if pieces.is_empty() {
        let indent_len = line.len() - line.trim_start_matches([' ', '\t']).len();
        return line[..indent_len].to_string();
    }

    let last = pieces.len() - 1;
    let mut prefix = String::new();
    for (i, piece) in pieces.iter().enumerate() {
        match piece {
            Piece::Quote(quote) => prefix.push_str(quote),
            Piece::Marker { indent, marker } => {
                prefix.push_str(indent);
                if i == last {
                    prefix.push_str(&" ".repeat(marker.chars().count()));
                } else {
                    prefix.push_str(marker);
                }
            }
        }
    }
    prefix
}

/// The byte range of the line `at` sits on, without the newline that ends it.
fn line_around(body: &str, at: usize) -> (usize, usize) {
    let line_start = body[..at].rfind('\n').map_or(0, |i| i + 1);
    let line_end = body[at..].find('\n').map_or(body.len(), |i| at + i);
    (line_start, line_end)
}

/// Puts a line break and `prefix` where the selection is, replacing whatever was selected, and
/// answers where the caret lands.
fn break_line(
    body: &str,
    start: usize,
    start_byte: usize,
    end_byte: usize,
    prefix: &str,
) -> (String, usize) {
    let mut out = String::with_capacity(body.len() + prefix.len() + 1);
    out.push_str(&body[..start_byte]);
    out.push('\n');
    out.push_str(prefix);
    out.push_str(&body[end_byte..]);
    (out, start + 1 + prefix.chars().count())
}

/// Splits the text at the caret, putting the line's own layout on the line that opens
/// underneath, or taking an empty block away from the line itself. Pressing Enter on a list item
/// with nothing in it ends the list; pressing it inside an empty quote that sits in an item takes
/// only the quote and leaves the item ready to type in. A selection is always replaced by the
/// line break, as it would be by a plain Enter. Returns the new text and where the caret lands.
pub fn newline_with_prefix(body: &str, start: usize, end: usize) -> (String, usize) {
    let (start, end) = (start.min(end), start.max(end));
    let start_byte = char_to_byte(body, start);
    let end_byte = char_to_byte(body, end);
    let (line_start, line_end) = line_around(body, start_byte);

    let line = &body[line_start..line_end];
    match line_break(line) {
        // With a selection the line is only broken: the selected text is what Enter replaces,
        // so the empty block it is in is kept.
        LineBreak::Here(text) if start == end => {
            let mut out = String::with_capacity(body.len() + text.len());
            out.push_str(&body[..line_start]);
            out.push_str(&text);
            out.push_str(&body[line_end..]);
            (out, line_start + text.chars().count())
        }
        LineBreak::Here(_) => break_line(body, start, start_byte, end_byte, ""),
        LineBreak::Next(prefix) => break_line(body, start, start_byte, end_byte, &prefix),
    }
}

/// Shift+Enter keeps the same item going instead of starting a new one: the marker is not
/// repeated, and the new line is stepped in only as far as the marker is wide, so its words line
/// up under the item's own words and it still belongs to that item.
pub fn newline_continuing_item(body: &str, start: usize, end: usize) -> (String, usize) {
    let (start, end) = (start.min(end), start.max(end));
    let start_byte = char_to_byte(body, start);
    let end_byte = char_to_byte(body, end);
    let (line_start, line_end) = line_around(body, start_byte);
    let prefix = continuation_line_prefix(&body[line_start..line_end]);
    break_line(body, start, start_byte, end_byte, &prefix)
}

/// The next (or previous) match to step to, wrapping around at either end.
pub fn step_match(current: usize, total: usize, backwards: bool) -> usize {
    if total == 0 {
        return 0;
    }
    if backwards {
        (current + total - 1) % total
    } else {
        (current + 1) % total
    }
}

/// Where `query` occurs in `body`, ignoring case, as byte offsets into `body`. Each character is
/// compared through its lowercase form, so a match is never reported at an offset that is in the
/// middle of a character.
pub fn find_matches(body: &str, query: &str) -> Vec<usize> {
    let needle: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    if needle.is_empty() {
        return Vec::new();
    }
    let hay: Vec<(usize, char)> = body.char_indices().collect();
    let mut hits = Vec::new();
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        let matched = needle.iter().enumerate().all(|(k, wanted)| {
            let mut lowered = hay[i + k].1.to_lowercase();
            lowered.next() == Some(*wanted) && lowered.next().is_none()
        });
        if matched {
            hits.push(hay[i].0);
            i += needle.len();
        } else {
            i += 1;
        }
    }
    hits
}
