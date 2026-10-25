//! The Markdown preview: multi-word inline formatting, glowing highlights, clickable
//! checkboxes, pipe tables and `[[Wiki Links]]`.
use super::*;

/// What the preview hands back to the editor: the things a reader can do to the text while it is
/// being shown.
#[derive(Default)]
pub(super) struct PreviewActions {
    /// A `[[wiki link]]` that was clicked.
    pub(super) wiki_link: Option<String>,
    /// The task list item that was ticked or unticked, counted over the whole note.
    pub(super) toggled_task: Option<usize>,
    /// A picture dragged to a new size: which picture (counted over the whole note), and its
    /// width in points.
    pub(super) image: Option<(usize, u32)>,
}

pub(super) fn render_markdown_preview(
    ui: &mut Ui,
    markdown: &str,
    p: &Palette,
    note_dir: Option<&Path>,
    images: &mut NoteImages,
    actions: &mut PreviewActions,
) {
    let lines: Vec<&str> = markdown.lines().collect();
    let mut counters = PreviewCounters::default();
    render_preview_lines(
        ui,
        &lines,
        PreviewLook {
            p,
            note_dir,
            muted: false,
        },
        images,
        actions,
        &mut counters,
    );
}

/// The picture and task numbers of a note, carried from one block to the next, so a table inside a
/// quote does not restart the numbering of the pictures around it.
#[derive(Default)]
struct PreviewCounters {
    images: usize,
    tasks: usize,
    /// One per nested block drawn, so each gets an id of its own.
    blocks: usize,
}

/// How a run of lines is being drawn: the colours of the note, where its pictures live, and
/// whether this is the dimmer text inside a quote.
#[derive(Clone, Copy)]
struct PreviewLook<'a> {
    p: &'a Palette,
    note_dir: Option<&'a Path>,
    muted: bool,
}

/// Draws a run of lines. A quote's contents and the block under a list item are drawn by this same
/// function one level in, so a table, a list or another quote nested in either one is read the way
/// it would be at the top of the note.
fn render_preview_lines(
    ui: &mut Ui,
    lines: &[&str],
    look: PreviewLook<'_>,
    images: &mut NoteImages,
    actions: &mut PreviewActions,
    counters: &mut PreviewCounters,
) {
    let p = look.p;
    let body_color = if look.muted {
        p.fg_secondary
    } else {
        p.fg_primary
    };
    let mut in_code_block = false;
    let mut code_buffer = String::new();
    let mut index = 0usize;

    while index < lines.len() {
        let line = lines[index];
        index += 1;
        let trimmed = line.trim_end();
        let pictures_here = split_images(line)
            .iter()
            .filter(|piece| matches!(piece, LinePiece::Image { .. }))
            .count();
        if trimmed.trim_start().starts_with("```") {
            counters.images += pictures_here;
            if in_code_block {
                Frame::new()
                    .fill(p.bg_elevated)
                    .stroke(Stroke::new(1.0_f32, p.border))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(
                            RichText::new(code_buffer.trim_end())
                                .monospace()
                                .size(13.0)
                                .color(p.fg_primary),
                        );
                    });
                code_buffer.clear();
                in_code_block = false;
                ui.add_space(6.0);
            } else {
                in_code_block = true;
            }
            continue;
        }

        if in_code_block {
            code_buffer.push_str(line);
            code_buffer.push('\n');
            counters.images += pictures_here;
            continue;
        }

        if trimmed.is_empty() {
            ui.add_space(6.0);
            continue;
        }

        // A header row followed by a delimiter row is a table: it takes the lines it owns, and the
        // pictures inside its cells are drawn there and counted as they are drawn.
        if let Some((table, used)) = parse_table(&lines[index - 1..]) {
            index += used - 1;
            render_table(ui, &table, look, images, actions, counters);
            continue;
        }

        // A quote is a block of its own: its lines are read again with the markers off, so a
        // table, a list or a picture written inside it is drawn inside the quote.
        if let Some((quote, used)) = quote_block(&lines[index - 1..]) {
            Frame::new()
                .fill(p.bg_sidebar)
                .stroke(Stroke::new(1.5_f32, p.accent))
                .corner_radius(CornerRadius::same(4))
                .inner_margin(Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    render_preview_lines(
                        ui,
                        &quote,
                        PreviewLook {
                            p,
                            note_dir: look.note_dir,
                            muted: true,
                        },
                        images,
                        actions,
                        counters,
                    );
                });
            index += used - 1;
            continue;
        }

        let trimmed_start = trimmed.trim_start();

        // `![alt](src)` is a picture, the way GitHub draws one. The text around it stays text,
        // and each picture gets its own line under the words it was written next to.
        let pieces = split_images(trimmed_start);
        if pieces
            .iter()
            .any(|piece| matches!(piece, LinePiece::Image { .. }))
        {
            for piece in pieces {
                match piece {
                    LinePiece::Text(text) if !text.trim().is_empty() => {
                        ui.horizontal_wrapped(|ui| {
                            render_inline_spans_ui(
                                ui,
                                text.trim(),
                                14.5,
                                p,
                                body_color,
                                &mut actions.wiki_link,
                                false,
                            );
                        });
                    }
                    LinePiece::Image { alt, src, width } => {
                        let index = counters.images;
                        counters.images += 1;
                        if let Some(kept) = images.show(ui, &src, &alt, width, look.note_dir, p) {
                            actions.image = Some((index, kept));
                        }
                        ui.add_space(4.0);
                    }
                    LinePiece::Text(_) => {}
                }
            }
            continue;
        }

        if trimmed_start == "---" || trimmed_start == "***" {
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(4.0);
        } else if let Some(h1) = trimmed_start.strip_prefix("# ") {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                render_inline_spans_ui(ui, h1, 21.5, p, p.fg_primary, &mut actions.wiki_link, true);
            });
            ui.add_space(2.0);
        } else if let Some(h2) = trimmed_start.strip_prefix("## ") {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                render_inline_spans_ui(ui, h2, 18.0, p, p.fg_primary, &mut actions.wiki_link, true);
            });
            ui.add_space(2.0);
        } else if let Some(h3) = trimmed_start.strip_prefix("### ") {
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                render_inline_spans_ui(
                    ui,
                    h3,
                    16.0,
                    p,
                    p.fg_secondary,
                    &mut actions.wiki_link,
                    true,
                );
            });
        } else if let Some(done) = trimmed_start
            .strip_prefix("- [x] ")
            .or_else(|| trimmed_start.strip_prefix("- [X] "))
        {
            let this_idx = counters.tasks;
            counters.tasks += 1;
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new(icon::SQUARE_CHECK).color(p.accent).size(15.5),
                        )
                        .frame(false),
                    )
                    .on_hover_text("Click to uncheck task")
                    .clicked()
                {
                    actions.toggled_task = Some(this_idx);
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new(done)
                        .strikethrough()
                        .color(p.fg_muted)
                        .size(14.5),
                );
            });
            index +=
                render_list_item_body(ui, &lines[index..], line, look, images, actions, counters);
        } else if let Some(todo) = trimmed_start.strip_prefix("- [ ] ") {
            let this_idx = counters.tasks;
            counters.tasks += 1;
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new(icon::SQUARE).color(p.fg_secondary).size(15.5),
                        )
                        .frame(false),
                    )
                    .on_hover_text("Click to complete task")
                    .clicked()
                {
                    actions.toggled_task = Some(this_idx);
                }
                ui.add_space(4.0);
                render_inline_spans_ui(
                    ui,
                    todo,
                    14.5,
                    p,
                    body_color,
                    &mut actions.wiki_link,
                    false,
                );
            });
            index +=
                render_list_item_body(ui, &lines[index..], line, look, images, actions, counters);
        } else if let Some(bullet) = trimmed_start
            .strip_prefix("- ")
            .or_else(|| trimmed_start.strip_prefix("* "))
        {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("•").color(p.accent).size(15.0));
                ui.add_space(5.0);
                render_inline_spans_ui(
                    ui,
                    bullet,
                    14.5,
                    p,
                    body_color,
                    &mut actions.wiki_link,
                    false,
                );
            });
            index +=
                render_list_item_body(ui, &lines[index..], line, look, images, actions, counters);
        } else {
            ui.horizontal_wrapped(|ui| {
                render_inline_spans_ui(
                    ui,
                    trimmed_start,
                    14.5,
                    p,
                    body_color,
                    &mut actions.wiki_link,
                    false,
                );
            });
        }
    }
}

/// Draws what belongs to a list item under its text — a table, a nested list, another paragraph —
/// a step in from the marker, so it reads as part of the item. Returns how many lines it used.
fn render_list_item_body(
    ui: &mut Ui,
    lines: &[&str],
    marker_line: &str,
    look: PreviewLook<'_>,
    images: &mut NoteImages,
    actions: &mut PreviewActions,
    counters: &mut PreviewCounters,
) -> usize {
    let marker_indent = marker_line.len() - marker_line.trim_start().len();
    let Some((body, used)) = list_item_body(lines, marker_indent) else {
        return 0;
    };
    let block = counters.blocks;
    counters.blocks += 1;
    ui.indent(Id::new(("preview_list_item", block)), |ui| {
        render_preview_lines(ui, &body, look, images, actions, counters);
    });
    used
}

/// Room kept at either side of a cell's words, and between two columns.
const TABLE_CELL_PAD: f32 = 10.0;
const TABLE_GAP: f32 = 16.0;
/// Room a picture in a cell is given when the note did not set a width for it, so the column starts
/// out big enough to show it; the table's own squeeze still keeps it inside the pane.
const TABLE_PICTURE_WIDTH: f32 = 160.0;

/// The shape of the row being drawn: how wide each column is, where each one lines up, and whether
/// this is the header.
struct TableRow<'a> {
    widths: &'a [f32],
    aligns: &'a [ColumnAlign],
    header: bool,
}

/// Draws a pipe table: the header in bold with a hairline under it, and every cell in a column of
/// its own width, lined up the way the delimiter row asked for. Cells keep the inline formatting,
/// wiki links, tags and pictures a note has everywhere else, so a link in a cell opens like any
/// other and a picture in a cell is drawn in it.
fn render_table(
    ui: &mut Ui,
    table: &Table,
    look: PreviewLook<'_>,
    images: &mut NoteImages,
    actions: &mut PreviewActions,
    counters: &mut PreviewCounters,
) {
    let p = look.p;
    let columns = table.header.len();
    if columns == 0 {
        return;
    }

    // Every column is as wide as its widest cell, so a row never shifts the one under it. The
    // table is only squeezed when it would not fit the pane, and then the cells wrap.
    let mut widths = vec![0.0f32; columns];
    for (cell, width) in table.header.iter().zip(widths.iter_mut()) {
        *width = width.max(cell_width(ui, cell));
    }
    for row in &table.rows {
        for (cell, width) in row.iter().zip(widths.iter_mut()) {
            *width = width.max(cell_width(ui, cell));
        }
    }
    for width in &mut widths {
        *width += TABLE_CELL_PAD;
    }
    let gaps = TABLE_GAP * (columns - 1) as f32;
    let available = (ui.available_width() - 2.0 * 12.0).max(120.0);
    let total: f32 = widths.iter().sum::<f32>() + gaps;
    if total > available {
        let scale = (available - gaps).max(48.0) / widths.iter().sum::<f32>().max(1.0);
        for width in &mut widths {
            *width *= scale;
        }
    }

    Frame::new()
        .fill(p.bg_elevated)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(12, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let header = TableRow {
                widths: &widths,
                aligns: &table.aligns,
                header: true,
            };
            render_table_row(ui, &table.header, &header, look, images, actions, counters);

            // A hairline under the header, across the whole table.
            let line = ui.cursor().top() + 2.0;
            ui.painter().hline(
                ui.max_rect().x_range(),
                line,
                Stroke::new(1.0_f32, p.border),
            );
            ui.add_space(6.0);

            let body = TableRow {
                widths: &widths,
                aligns: &table.aligns,
                header: false,
            };
            for row in &table.rows {
                render_table_row(ui, row, &body, look, images, actions, counters);
            }
        });
    ui.add_space(6.0);
}

/// One row of a [table](render_table): each cell gets its column's width, and that column's
/// alignment decides where its words sit inside it. A picture in a cell is drawn the way one on a
/// line of its own is — under the words beside it, inside the cell — and counted in the note's
/// picture numbering as it is drawn, so dragging its corner writes back to the right one.
fn render_table_row(
    ui: &mut Ui,
    cells: &[String],
    row: &TableRow<'_>,
    look: PreviewLook<'_>,
    images: &mut NoteImages,
    actions: &mut PreviewActions,
    counters: &mut PreviewCounters,
) {
    let p = look.p;
    ui.horizontal_top(|ui| {
        for (i, cell) in cells.iter().enumerate() {
            if i > 0 {
                ui.add_space(TABLE_GAP);
            }
            let width = row.widths[i];
            // egui's grid and its layouts line a cell's first widget up on the left whatever
            // alignment it is given, so the words are moved in from that edge by the space their
            // own measurement leaves over. The measurement is the one the column widths came
            // from, so the widest cell sits flush and the others line up with it.
            let room = (width - cell_width(ui, cell)).max(0.0);
            let leading = match row.aligns[i] {
                ColumnAlign::Left => 0.0,
                ColumnAlign::Center => room / 2.0,
                ColumnAlign::Right => room,
            };
            ui.allocate_ui_with_layout(Vec2::new(width, 0.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(width);
                for piece in split_images(cell) {
                    match piece {
                        LinePiece::Text(text) if !text.trim().is_empty() => {
                            ui.horizontal_wrapped(|ui| {
                                if leading > 0.0 {
                                    ui.add_space(leading);
                                }
                                render_inline_spans_ui(
                                    ui,
                                    text.trim(),
                                    14.0,
                                    p,
                                    p.fg_primary,
                                    &mut actions.wiki_link,
                                    row.header,
                                );
                            });
                        }
                        LinePiece::Image { alt, src, width } => {
                            let index = counters.images;
                            counters.images += 1;
                            let kept = ui
                                .horizontal(|ui| {
                                    ui.add_space(leading);
                                    images.show(ui, &src, &alt, width, look.note_dir, p)
                                })
                                .inner;
                            if let Some(kept) = kept {
                                actions.image = Some((index, kept));
                            }
                        }
                        LinePiece::Text(_) => {}
                    }
                }
            });
        }
    });
    ui.add_space(3.0);
}

/// How wide a cell needs to be: its words in the preview's own font, or a picture in it that is
/// wider. Markers are left out; bold and code runs come out a little wider than this, which is what
/// the cell padding is for. A picture asks for the width the note gave it, or else room for a
/// thumbnail-sized view.
fn cell_width(ui: &Ui, text: &str) -> f32 {
    let mut words = String::new();
    let mut picture = 0.0f32;
    for piece in split_images(text) {
        match piece {
            LinePiece::Text(text) => words.push_str(&text),
            LinePiece::Image { width, .. } => {
                picture = picture.max(width.map_or(TABLE_PICTURE_WIDTH, |width| width as f32));
            }
        }
    }
    let plain = crate::markdown::strip_all_inline_markers(&words);
    let words = ui.fonts(|fonts| {
        fonts
            .layout_no_wrap(plain, FontId::proportional(14.0), Color32::WHITE)
            .size()
            .x
    });
    words.max(picture)
}
