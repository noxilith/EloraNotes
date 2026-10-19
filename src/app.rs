use crate::icon_names::TABLER_ICON_NAMES;
use crate::icons::{icon, install_tabler_icons, tabler_glyph};
use crate::markdown::{
    apply_highlight_selection, build_editor_layout_job, clear_formatting_selection,
    color_swatch_button, insert_at_selection, markdown_to_html, render_inline_spans_ui, rgb_to_hex,
    wrap_selection, HIGHLIGHT_PRESETS,
};
use crate::storage::{
    extract_wiki_links, fuzzy_subsequence, normalize_tag, sort_notes, AppSettings, Note,
    NoteStorage, NoteTemplate, SortOrder,
};
use crate::theme::{ColorOverrides, Palette, ThemeMode};
use chrono::{Local, Utc};
use egui::{
    Align, Align2, Color32, CornerRadius, FontFamily, FontId, Frame, Id, Key, Layout, Margin, Pos2,
    RichText, ScrollArea, Sense, Stroke, TextEdit, TextStyle, Ui, Vec2,
};
use std::collections::BTreeMap;
use std::f32::consts::TAU;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorViewMode {
    Edit,
    Split,
    Preview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarSection {
    AllNotes,
    Pinned,
    Trash,
}

/// Undo/redo stacks of note bodies for one note.
#[derive(Default)]
struct EditHistory {
    undo: Vec<String>,
    redo: Vec<String>,
    last_change: Option<Instant>,
}

pub struct EloraApp {
    /// Per-note undo/redo history, keyed by note id.
    edit_history: BTreeMap<String, EditHistory>,
    /// Note id and body as of the end of the last editor frame, used to detect edits.
    last_seen_body: Option<(String, String)>,
    storage: NoteStorage,
    notes: Vec<Note>,
    selected_id: Option<String>,
    search_query: String,
    active_tag: Option<String>,
    sidebar_section: SidebarSection,
    sort_order: SortOrder,
    new_tag_input: String,
    theme: ThemeMode,
    focus_mode: bool,
    view_mode: EditorViewMode,
    show_formatting_bar: bool,
    /// Current glow highlight color `[R, G, B]` selected by the user's color picker.
    highlight_rgb: [u8; 3],
    /// Color used by the toolbar's "Text Color" button for the next wrapped selection.
    text_color_rgb: [u8; 3],
    /// Note that the icon picker was opened for; the picker closes if another note is selected.
    icon_picker_note_id: Option<String>,
    /// User-chosen overrides for background, text, accent and pin colors.
    colors: ColorOverrides,
    /// Body text size in points.
    font_size: f32,
    /// Glow halo intensity for highlights, 0.0 to 1.0.
    glow_strength: f32,
    show_settings: bool,
    /// Note graph (live notes and their wiki links). `Some` while the graph window is open.
    graph: Option<Vec<GraphNode>>,
    /// Tracks the latest character selection range `(start_char, end_char)` in the active note editor.
    last_editor_selection: Option<(usize, usize)>,
    confirm_delete_id: Option<String>,
    command_palette_open: bool,
    icon_picker_open: bool,
    /// True when the picker is inserting a glyph into the body instead of setting the header icon.
    icon_picker_for_body: bool,
    icon_search: String,
    command_palette_query: String,
    command_palette_selected: usize,
    focus_search_next_frame: bool,
    focus_title_next_frame: bool,
    focus_palette_next_frame: bool,
    status_message: Option<String>,
}

/// One live note in the graph view. `links` holds indices of the notes it links to.
struct GraphNode {
    id: String,
    label: String,
    links: Vec<usize>,
}

#[derive(Clone)]
enum PaletteItem {
    OpenNote {
        id: String,
        title: String,
        pinned: bool,
        tags: Vec<String>,
    },
    Action {
        icon_glyph: &'static str,
        label: String,
        shortcut: &'static str,
        cmd: PaletteCommand,
    },
}

#[derive(Clone, Copy)]
enum PaletteCommand {
    NewBlankNote,
    OpenTodayDailyNote,
    NewMeetingNote,
    NewProjectSpec,
    ToggleFocusMode,
    SetViewMode(EditorViewMode),
    TogglePinSelected,
    SetTheme(ThemeMode),
    CycleSortOrder,
    ToggleFormattingBar,
    CopyNoteMarkdown,
    ExportNoteHtml,
    ExportVaultHtml,
    ImportMarkdown,
    ToggleGraph,
    SwitchSection(SidebarSection),
}

#[derive(Clone, Copy)]
enum EditorFormatOp {
    Bold,
    Italic,
    Strikethrough,
    Underline,
    InlineCode,
    GlowHighlight([u8; 3]),
    TextColor([u8; 3]),
    WikiLink,
    Heading1,
    Heading2,
    BulletItem,
    ChecklistItem,
    Blockquote,
    CodeFence,
    ClearFormatting,
}

/// One row of the Settings color section: a checkbox that enables an override, plus a color
/// picker shown while enabled. Returns true when the value changed.
fn color_override_row(
    ui: &mut Ui,
    label: &str,
    value: &mut Option<[u8; 3]>,
    default: Color32,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let mut enabled = value.is_some();
        if ui.checkbox(&mut enabled, label).changed() {
            *value = if enabled {
                let [r, g, b, _] = default.to_array();
                Some([r, g, b])
            } else {
                None
            };
            changed = true;
        }
        if let Some(rgb) = value.as_mut() {
            changed |= ui.color_edit_button_srgb(rgb).changed();
        }
    });
    changed
}

impl EloraApp {
    pub fn new(cc: &eframe::CreationContext<'_>, storage: NoteStorage) -> Self {
        install_tabler_icons(&cc.egui_ctx);

        let settings = storage.load_settings();
        let theme = ThemeMode::from_name(&settings.theme_name);
        theme.apply(
            &cc.egui_ctx,
            &settings.colors,
            settings.font_size,
            settings.glow_strength,
        );

        let sort_order = settings.sort_order;
        let notes = storage.load_all(sort_order).unwrap_or_default();
        let selected_id = notes.iter().find(|n| !n.trashed).map(|n| n.id.clone());

        Self {
            storage,
            notes,
            selected_id,
            search_query: String::new(),
            active_tag: None,
            sidebar_section: SidebarSection::AllNotes,
            sort_order,
            new_tag_input: String::new(),
            theme,
            focus_mode: false,
            view_mode: EditorViewMode::Split,
            show_formatting_bar: settings.show_formatting_bar,
            highlight_rgb: settings.highlight_rgb,
            colors: settings.colors,
            font_size: settings.font_size,
            glow_strength: settings.glow_strength,
            show_settings: false,
            graph: None,
            last_editor_selection: None,
            edit_history: BTreeMap::new(),
            last_seen_body: None,
            confirm_delete_id: None,
            command_palette_open: false,
            icon_picker_open: false,
            icon_picker_for_body: false,
            text_color_rgb: settings.text_color_rgb,
            icon_picker_note_id: None,
            icon_search: String::new(),
            command_palette_query: String::new(),
            command_palette_selected: 0,
            focus_search_next_frame: false,
            focus_title_next_frame: false,
            focus_palette_next_frame: false,
            status_message: None,
        }
    }

    fn persist_settings(&self) {
        let settings = AppSettings {
            theme_name: self.theme.name().to_string(),
            sort_order: self.sort_order,
            show_formatting_bar: self.show_formatting_bar,
            highlight_rgb: self.highlight_rgb,
            text_color_rgb: self.text_color_rgb,
            font_size: self.font_size,
            glow_strength: self.glow_strength,
            colors: self.colors,
        };
        self.storage.save_settings(&settings);
    }

    fn apply_look(&self, ctx: &egui::Context) {
        self.theme
            .apply(ctx, &self.colors, self.font_size, self.glow_strength);
    }

    /// Settings window: theme, text size, color overrides, glow strength and layout toggles.
    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let mut open = true;
        let mut changed = false;
        let defaults = self.theme.palette();

        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(340.0)
            .show(ctx, |ui| {
                ui.label(RichText::new("Appearance").strong());
                ui.horizontal_wrapped(|ui| {
                    for t in [
                        ThemeMode::Obsidian,
                        ThemeMode::Paper,
                        ThemeMode::Nordic,
                        ThemeMode::Matcha,
                        ThemeMode::Sepia,
                        ThemeMode::Midnight,
                        ThemeMode::Matugen,
                    ] {
                        if ui.selectable_label(self.theme == t, t.name()).clicked() {
                            self.theme = t;
                            changed = true;
                        }
                    }
                });
                changed |= ui
                    .add(egui::Slider::new(&mut self.font_size, 11.0..=22.0).text("Text size"))
                    .changed();
                ui.separator();

                ui.label(RichText::new("Colors").strong());
                ui.label("Tick a color to override the theme's default.");
                changed |=
                    color_override_row(ui, "Background", &mut self.colors.bg, defaults.bg_canvas);
                changed |= color_override_row(
                    ui,
                    "Sidebar",
                    &mut self.colors.sidebar,
                    defaults.bg_sidebar,
                );
                changed |= color_override_row(ui, "Text", &mut self.colors.fg, defaults.fg_primary);
                changed |=
                    color_override_row(ui, "Accent", &mut self.colors.accent, defaults.accent);
                changed |= color_override_row(ui, "Pin", &mut self.colors.pin, defaults.pin_color);
                changed |= color_override_row(
                    ui,
                    "Muted text",
                    &mut self.colors.fg_muted,
                    defaults.fg_muted,
                );
                changed |=
                    color_override_row(ui, "Borders", &mut self.colors.border, defaults.border);
                changed |=
                    color_override_row(ui, "Danger", &mut self.colors.danger, defaults.danger);
                if ui.button("Reset colors to theme").clicked() {
                    self.colors = Default::default();
                    changed = true;
                }
                ui.separator();

                ui.label(RichText::new("Glow highlights").strong());
                changed |= ui
                    .add(
                        egui::Slider::new(&mut self.glow_strength, 0.0..=1.0).text("Glow strength"),
                    )
                    .changed();
                ui.horizontal(|ui| {
                    ui.label("Default glow color");
                    changed |= ui.color_edit_button_srgb(&mut self.highlight_rgb).changed();
                });
                ui.separator();

                changed |= ui
                    .checkbox(&mut self.show_formatting_bar, "Show formatting bar")
                    .changed();
                ui.separator();

                if ui
                    .button(format!("{}  Reset all settings to defaults", icon::RESTORE))
                    .clicked()
                {
                    let d = AppSettings::default();
                    self.theme = ThemeMode::from_name(&d.theme_name);
                    self.colors = d.colors;
                    self.font_size = d.font_size;
                    self.glow_strength = d.glow_strength;
                    self.highlight_rgb = d.highlight_rgb;
                    self.show_formatting_bar = d.show_formatting_bar;
                    changed = true;
                }
            });

        if !open {
            self.show_settings = false;
        }
        if changed {
            self.apply_look(ctx);
            self.persist_settings();
        }
    }

    fn resort(&mut self) {
        sort_notes(&mut self.notes, self.sort_order);
    }

    fn all_tags_with_counts(&self) -> Vec<(String, usize)> {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for note in self.notes.iter().filter(|n| !n.trashed) {
            for tag in note.all_tags() {
                *counts.entry(tag).or_insert(0) += 1;
            }
        }
        counts.into_iter().collect()
    }

    fn filtered_indices(&self) -> Vec<usize> {
        let mut indices: Vec<usize> = self
            .notes
            .iter()
            .enumerate()
            .filter(|(_, note)| {
                match self.sidebar_section {
                    SidebarSection::AllNotes => {
                        if note.trashed {
                            return false;
                        }
                    }
                    SidebarSection::Pinned => {
                        if note.trashed || !note.pinned {
                            return false;
                        }
                    }
                    SidebarSection::Trash => {
                        if !note.trashed {
                            return false;
                        }
                    }
                }
                note.matches_filter(&self.search_query, self.active_tag.as_deref())
            })
            .map(|(i, _)| i)
            .collect::<Vec<usize>>();
        // Best matches first; the stable sort keeps the existing order for ties.
        // Each score is computed once, not once per comparison.
        if !self.search_query.trim().is_empty() {
            let mut scored: Vec<(usize, usize)> = indices
                .iter()
                .map(|&i| (self.notes[i].relevance_score(&self.search_query), i))
                .collect();
            scored.sort_by_key(|&(score, _)| std::cmp::Reverse(score));
            indices = scored.into_iter().map(|(_, i)| i).collect();
        }
        indices
    }

    /// Opens the note graph, or closes it when it is already open. Builds the links once on open.
    fn toggle_graph(&mut self) {
        if self.graph.is_some() {
            self.graph = None;
            return;
        }
        let live: Vec<&Note> = self.notes.iter().filter(|n| !n.trashed).collect();
        let mut nodes: Vec<GraphNode> = live
            .iter()
            .map(|n| GraphNode {
                id: n.id.clone(),
                label: n.display_title(),
                links: Vec::new(),
            })
            .collect();
        for (i, note) in live.iter().enumerate() {
            for target in extract_wiki_links(&note.body) {
                let found = live.iter().position(|n| {
                    n.title.eq_ignore_ascii_case(&target)
                        || n.display_title().eq_ignore_ascii_case(&target)
                });
                if let Some(j) = found {
                    if j != i && !nodes[i].links.contains(&j) {
                        nodes[i].links.push(j);
                    }
                }
            }
        }
        self.graph = Some(nodes);
    }

    /// Floating window that places live notes on a ring and draws a line per wiki link.
    /// Clicking a note's dot opens that note.
    fn graph_window(&mut self, ctx: &egui::Context) {
        let Some(nodes) = self.graph.as_ref() else {
            return;
        };
        let p = self.theme.palette_with(&self.colors, self.glow_strength);
        let mut open = true;
        let mut clicked: Option<String> = None;

        egui::Window::new("Note Graph")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([520.0, 480.0])
            .show(ctx, |ui| {
                let size = ui.available_size().max(Vec2::splat(240.0));
                let (rect, response) = ui.allocate_exact_size(size, Sense::click());
                let painter = ui.painter_at(rect);
                painter.rect_filled(rect, CornerRadius::same(6), p.bg_canvas);

                if nodes.is_empty() {
                    painter.text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        "No notes yet",
                        FontId::proportional(14.0),
                        p.fg_muted,
                    );
                    return;
                }

                let center = rect.center();
                let radius = rect.width().min(rect.height()) * 0.38;
                let count = nodes.len() as f32;
                let positions: Vec<Pos2> = (0..nodes.len())
                    .map(|i| {
                        // Start at the top of the ring and go clockwise.
                        let a = TAU * i as f32 / count - TAU / 4.0;
                        center + Vec2::new(a.cos(), a.sin()) * radius
                    })
                    .collect();

                for (i, node) in nodes.iter().enumerate() {
                    for &j in &node.links {
                        painter.line_segment(
                            [positions[i], positions[j]],
                            Stroke::new(1.0_f32, p.border),
                        );
                    }
                }

                let hover = response
                    .hover_pos()
                    .and_then(|pos| positions.iter().position(|q| q.distance(pos) <= 10.0));
                for (i, node) in nodes.iter().enumerate() {
                    let color = if hover == Some(i) {
                        p.accent
                    } else {
                        p.fg_secondary
                    };
                    painter.circle_filled(positions[i], 6.0, color);
                    painter.text(
                        positions[i] + Vec2::new(0.0, 10.0),
                        Align2::CENTER_TOP,
                        &node.label,
                        FontId::proportional(12.0),
                        p.fg_primary,
                    );
                }

                if response.clicked() {
                    if let Some(i) = hover {
                        clicked = Some(nodes[i].id.clone());
                    }
                }
            });

        if let Some(id) = clicked {
            self.selected_id = Some(id);
            self.graph = None;
        } else if !open {
            self.graph = None;
        }
    }

    /// Writes HTML copies of the selected note (or every live note) to the Downloads folder.
    /// Falls back to the notes folder when no Downloads folder is available.
    /// Imports `.md` files from the `Import` folder inside the notes folder, then reloads the list.
    fn import_markdown(&mut self) {
        let import_dir = self.storage.root_dir().join("Import");
        if let Err(e) = std::fs::create_dir_all(&import_dir) {
            self.status_message = Some(format!("Import failed: {e}"));
            return;
        }
        match self.storage.import_markdown_dir(&import_dir) {
            Ok(0) => {
                self.status_message = Some(format!(
                    "No .md files in {} to import",
                    import_dir.display()
                ));
            }
            Ok(count) => {
                self.notes = self.storage.load_all(self.sort_order).unwrap_or_default();
                self.status_message = Some(format!("Imported {count} note(s)"));
            }
            Err(e) => self.status_message = Some(format!("Import failed: {e}")),
        }
    }

    fn export_html(&mut self, whole_vault: bool) {
        let notes: Vec<&Note> = if whole_vault {
            self.notes.iter().filter(|n| !n.trashed).collect()
        } else {
            match self
                .selected_id
                .as_ref()
                .and_then(|id| self.notes.iter().find(|n| &n.id == id))
            {
                Some(note) => vec![note],
                None => {
                    self.status_message = Some("Select a note to export".to_string());
                    return;
                }
            }
        };

        let base = dirs::download_dir().unwrap_or_else(|| self.storage.root_dir().to_path_buf());
        let dir = if whole_vault {
            base.join("EloraNotes Export")
        } else {
            base
        };
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.status_message = Some(format!("Export failed: {e}"));
            return;
        }

        // File names use the note id so two notes with the same title never overwrite each other.
        let mut written = 0;
        for note in &notes {
            let html = markdown_to_html(&note.display_title(), &note.body);
            let path = dir.join(format!("{}.html", note.id));
            if let Err(e) = std::fs::write(&path, html) {
                self.status_message = Some(format!("Export failed: {e}"));
                return;
            }
            written += 1;
        }
        self.status_message = Some(format!("Exported {written} note(s) to {}", dir.display()));
    }

    fn create_note_from_template(&mut self, template: NoteTemplate) {
        let mut note = template.build(self.storage.root_dir());
        if let Some(tag) = &self.active_tag {
            note.add_tag(tag);
        }
        if self.sidebar_section == SidebarSection::Pinned {
            note.pinned = true;
        }
        if self.sidebar_section == SidebarSection::Trash {
            self.sidebar_section = SidebarSection::AllNotes;
        }
        if let Err(e) = self.storage.save_note(&note) {
            self.status_message = Some(format!("Failed to save note: {e}"));
        }
        let id = note.id.clone();
        self.notes.push(note);
        self.resort();
        self.selected_id = Some(id);
        if self.view_mode == EditorViewMode::Preview {
            self.view_mode = EditorViewMode::Edit;
        }
        self.focus_title_next_frame = true;
        self.status_message = Some("Created new note".to_string());
    }

    fn open_or_create_daily_note(&mut self) {
        let today_title = format!("Daily Note — {}", Local::now().format("%Y-%m-%d"));
        if let Some(existing) = self
            .notes
            .iter()
            .find(|n| !n.trashed && n.title.eq_ignore_ascii_case(&today_title))
        {
            self.selected_id = Some(existing.id.clone());
            self.sidebar_section = SidebarSection::AllNotes;
            self.status_message = Some(format!("Opened {today_title}"));
        } else {
            self.create_note_from_template(NoteTemplate::DailyJournal);
            self.status_message = Some(format!("Created {today_title}"));
        }
    }

    fn open_or_create_wiki_link(&mut self, target_title: &str) {
        let target_trimmed = target_title.trim();
        if target_trimmed.is_empty() {
            return;
        }
        if let Some(existing) = self.notes.iter().find(|n| {
            !n.trashed
                && (n.title.eq_ignore_ascii_case(target_trimmed)
                    || n.display_title().eq_ignore_ascii_case(target_trimmed))
        }) {
            self.selected_id = Some(existing.id.clone());
            self.status_message = Some(format!("Opened [[{target_trimmed}]]"));
            return;
        }

        let note = Note::new(
            target_trimmed,
            "Linked from another note.\n\n#linked",
            self.storage.root_dir(),
        );
        let _ = self.storage.save_note(&note);
        let id = note.id.clone();
        self.notes.push(note);
        self.resort();
        self.selected_id = Some(id);
        self.status_message = Some(format!("Created linked note [[{target_trimmed}]]"));
    }

    fn backlinks_for(&self, note_idx: usize) -> Vec<(String, String)> {
        let Some(target_note) = self.notes.get(note_idx) else {
            return Vec::new();
        };
        let target_title = target_note.display_title().to_lowercase();
        let target_id = target_note.id.clone();

        self.notes
            .iter()
            .filter(|n| !n.trashed && n.id != target_id)
            // Notes without a `[[` cannot link anywhere, so skip parsing them.
            .filter(|n| n.body.contains("[["))
            .filter(|n| {
                n.outbound_links()
                    .iter()
                    .any(|link| link.to_lowercase() == target_title)
            })
            .map(|n| (n.id.clone(), n.display_title()))
            .collect()
    }

    fn move_to_trash(&mut self, id: &str) {
        if let Some(note) = self.notes.iter_mut().find(|n| n.id == id) {
            note.trashed = true;
            note.pinned = false;
            note.updated_at = Utc::now();
            let _ = self.storage.save_note(note);
            self.resort();
            self.select_first_visible();
            self.status_message = Some("Moved note to Trash".to_string());
        }
    }

    fn restore_from_trash(&mut self, id: &str) {
        if let Some(note) = self.notes.iter_mut().find(|n| n.id == id) {
            note.trashed = false;
            note.updated_at = Utc::now();
            let _ = self.storage.save_note(note);
            self.resort();
            self.status_message = Some("Restored note from Trash".to_string());
        }
    }

    fn permanently_delete_by_id(&mut self, id: &str) {
        if let Some(pos) = self.notes.iter().position(|n| n.id == id) {
            let removed = self.notes.remove(pos);
            let _ = self.storage.delete_note(&removed);
            self.confirm_delete_id = None;
            self.select_first_visible();
            self.status_message = Some("Permanently deleted note".to_string());
        }
    }

    fn select_first_visible(&mut self) {
        let filtered = self.filtered_indices();
        self.selected_id = filtered
            .first()
            .and_then(|&idx| self.notes.get(idx))
            .map(|n| n.id.clone());
    }

    fn toggle_pin_selected(&mut self) {
        if let Some(id) = self.selected_id.clone() {
            if let Some(note) = self.notes.iter_mut().find(|n| n.id == id) {
                note.pinned = !note.pinned;
                note.updated_at = Utc::now();
                let _ = self.storage.save_note(note);
                self.resort();
            }
        }
    }

    fn apply_editor_format(&mut self, note_idx: usize, op: EditorFormatOp) {
        let range = self.last_editor_selection;
        let body = &mut self.notes[note_idx].body;

        match op {
            EditorFormatOp::Bold => wrap_selection(body, range, "**", "**", "bold text"),
            EditorFormatOp::Italic => wrap_selection(body, range, "*", "*", "italic text"),
            EditorFormatOp::Strikethrough => {
                wrap_selection(body, range, "~~", "~~", "strikethrough")
            }
            EditorFormatOp::Underline => wrap_selection(body, range, "__", "__", "underlined"),
            EditorFormatOp::InlineCode => wrap_selection(body, range, "`", "`", "code"),
            EditorFormatOp::GlowHighlight(rgb) => {
                self.highlight_rgb = rgb;
                apply_highlight_selection(body, range, rgb);
                self.persist_settings();
            }
            EditorFormatOp::TextColor(rgb) => {
                let open = format!("{{color:{}}}", rgb_to_hex(rgb));
                wrap_selection(body, range, &open, "{/color}", "colored text")
            }
            EditorFormatOp::WikiLink => wrap_selection(body, range, "[[", "]]", "Note Title"),
            EditorFormatOp::Heading1 => wrap_selection(body, range, "\n# ", "\n", "Heading 1"),
            EditorFormatOp::Heading2 => wrap_selection(body, range, "\n## ", "\n", "Heading 2"),
            EditorFormatOp::BulletItem => wrap_selection(body, range, "\n- ", "", "List item"),
            EditorFormatOp::ChecklistItem => {
                wrap_selection(body, range, "\n- [ ] ", "", "Task item")
            }
            EditorFormatOp::Blockquote => wrap_selection(body, range, "\n> ", "", "Quote"),
            EditorFormatOp::CodeFence => {
                wrap_selection(body, range, "\n```\n", "\n```", "code block")
            }
            EditorFormatOp::ClearFormatting => clear_formatting_selection(body, range),
        }

        self.notes[note_idx].updated_at = Utc::now();
        if let Err(e) = self.storage.save_note(&self.notes[note_idx]) {
            self.status_message = Some(format!("Auto-save error: {e}"));
        }
    }

    fn execute_palette_command(&mut self, cmd: PaletteCommand, ctx: &egui::Context) {
        match cmd {
            PaletteCommand::NewBlankNote => self.create_note_from_template(NoteTemplate::Blank),
            PaletteCommand::OpenTodayDailyNote => self.open_or_create_daily_note(),
            PaletteCommand::NewMeetingNote => {
                self.create_note_from_template(NoteTemplate::MeetingNotes)
            }
            PaletteCommand::NewProjectSpec => {
                self.create_note_from_template(NoteTemplate::ProjectSpec)
            }
            PaletteCommand::ToggleFocusMode => self.focus_mode = !self.focus_mode,
            PaletteCommand::SetViewMode(mode) => self.view_mode = mode,
            PaletteCommand::TogglePinSelected => self.toggle_pin_selected(),
            PaletteCommand::SetTheme(t) => {
                self.theme = t;
                self.apply_look(ctx);
                self.persist_settings();
                self.status_message = Some(format!("Theme: {}", self.theme.name()));
            }
            PaletteCommand::CycleSortOrder => {
                self.sort_order = self.sort_order.next();
                self.resort();
                self.persist_settings();
                self.status_message = Some(format!("Sorted by {}", self.sort_order.label()));
            }
            PaletteCommand::ToggleFormattingBar => {
                self.show_formatting_bar = !self.show_formatting_bar;
                self.persist_settings();
            }
            PaletteCommand::ExportNoteHtml => self.export_html(false),
            PaletteCommand::ExportVaultHtml => self.export_html(true),
            PaletteCommand::ImportMarkdown => self.import_markdown(),
            PaletteCommand::ToggleGraph => self.toggle_graph(),
            PaletteCommand::CopyNoteMarkdown => {
                if let Some(id) = &self.selected_id {
                    if let Some(note) = self.notes.iter().find(|n| &n.id == id) {
                        ctx.copy_text(format!("# {}\n\n{}", note.display_title(), note.body));
                        self.status_message = Some("Copied note Markdown to clipboard".to_string());
                    }
                }
            }
            PaletteCommand::SwitchSection(sec) => {
                self.sidebar_section = sec;
                self.select_first_visible();
            }
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let mut create_note = false;
        let mut open_daily = false;
        let mut focus_search = false;
        let mut toggle_palette = false;
        let mut toggle_focus = false;
        let mut toggle_preview = false;
        let mut toggle_split = false;
        let mut toggle_pin = false;
        let mut format_bold = false;
        let mut format_italic = false;
        let mut format_highlight = false;
        let mut press_escape = false;

        ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::COMMAND, Key::Comma) {
                self.show_settings = true;
            }
            if i.consume_key(egui::Modifiers::COMMAND, Key::K)
                || i.consume_key(egui::Modifiers::COMMAND, Key::P) && i.modifiers.shift
            {
                toggle_palette = true;
            }
            if i.consume_key(egui::Modifiers::COMMAND, Key::N) {
                create_note = true;
            }
            if i.consume_key(egui::Modifiers::COMMAND, Key::D) {
                open_daily = true;
            }
            if i.consume_key(egui::Modifiers::COMMAND, Key::F) {
                focus_search = true;
            }
            if i.consume_key(egui::Modifiers::COMMAND, Key::Backslash) {
                toggle_focus = true;
            }
            if i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::E) {
                toggle_split = true;
            } else if i.consume_key(egui::Modifiers::COMMAND, Key::E) {
                toggle_preview = true;
            }
            if i.consume_key(egui::Modifiers::COMMAND, Key::B) {
                format_bold = true;
            }
            if i.consume_key(egui::Modifiers::COMMAND, Key::I) {
                format_italic = true;
            }
            if i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::H) {
                format_highlight = true;
            }
            if i.consume_key(egui::Modifiers::COMMAND, Key::P) {
                toggle_pin = true;
            }
            if i.consume_key(egui::Modifiers::NONE, Key::Escape) {
                press_escape = true;
            }
        });

        if toggle_palette {
            self.command_palette_open = !self.command_palette_open;
            if self.command_palette_open {
                self.command_palette_query.clear();
                self.command_palette_selected = 0;
                self.focus_palette_next_frame = true;
            }
            return;
        }

        if press_escape {
            if self.command_palette_open {
                self.command_palette_open = false;
                return;
            }
            self.confirm_delete_id = None;
            if !self.search_query.is_empty()
                || self.active_tag.is_some()
                || self.sidebar_section != SidebarSection::AllNotes
            {
                self.search_query.clear();
                self.active_tag = None;
                self.sidebar_section = SidebarSection::AllNotes;
            }
        }

        if create_note {
            self.create_note_from_template(NoteTemplate::Blank);
        }
        if open_daily {
            self.open_or_create_daily_note();
        }
        if focus_search {
            self.focus_mode = false;
            self.focus_search_next_frame = true;
        }
        if toggle_focus {
            self.focus_mode = !self.focus_mode;
        }
        if toggle_split {
            self.view_mode = if self.view_mode == EditorViewMode::Split {
                EditorViewMode::Edit
            } else {
                EditorViewMode::Split
            };
        }
        if toggle_preview {
            self.view_mode = match self.view_mode {
                EditorViewMode::Preview => EditorViewMode::Edit,
                _ => EditorViewMode::Preview,
            };
        }
        if toggle_pin {
            self.toggle_pin_selected();
        }
        if format_bold || format_italic || format_highlight {
            if let Some(sel_id) = self.selected_id.clone() {
                if let Some(idx) = self.notes.iter().position(|n| n.id == sel_id) {
                    if format_bold {
                        self.apply_editor_format(idx, EditorFormatOp::Bold);
                    }
                    if format_italic {
                        self.apply_editor_format(idx, EditorFormatOp::Italic);
                    }
                    if format_highlight {
                        let rgb = self.highlight_rgb;
                        self.apply_editor_format(idx, EditorFormatOp::GlowHighlight(rgb));
                    }
                }
            }
        }
    }

    fn build_palette_items(&self) -> Vec<PaletteItem> {
        let q = self.command_palette_query.trim().to_lowercase();
        let mut items = Vec::new();

        for note in self.notes.iter().filter(|n| !n.trashed) {
            let title = note.display_title();
            let title_lower = title.to_lowercase();
            let tags = note.all_tags();
            let matches = q.is_empty()
                || title_lower.contains(&q)
                || fuzzy_subsequence(&title_lower, &q)
                || tags.iter().any(|t| t.contains(&q));
            if matches {
                items.push(PaletteItem::OpenNote {
                    id: note.id.clone(),
                    title,
                    pinned: note.pinned,
                    tags,
                });
            }
        }

        let actions: &[(&'static str, &str, &'static str, PaletteCommand)] = &[
            (
                icon::FILE_PLUS,
                "New Blank Note",
                "Ctrl+N",
                PaletteCommand::NewBlankNote,
            ),
            (
                icon::CALENDAR_EVENT,
                "Open Today's Daily Note",
                "Ctrl+D",
                PaletteCommand::OpenTodayDailyNote,
            ),
            (
                icon::NOTES,
                "New Note from Template: Meeting Notes",
                "",
                PaletteCommand::NewMeetingNote,
            ),
            (
                icon::COMPASS,
                "New Note from Template: Project Spec",
                "",
                PaletteCommand::NewProjectSpec,
            ),
            (
                icon::COLUMNS,
                "View: Split Editor + Live Preview",
                "Ctrl+Shift+E",
                PaletteCommand::SetViewMode(EditorViewMode::Split),
            ),
            (
                icon::EYE,
                "View: Clean Markdown Preview",
                "Ctrl+E",
                PaletteCommand::SetViewMode(EditorViewMode::Preview),
            ),
            (
                icon::PENCIL,
                "View: Markdown Editor",
                "",
                PaletteCommand::SetViewMode(EditorViewMode::Edit),
            ),
            (
                icon::MAXIMIZE,
                "Toggle Distraction-Free Focus Mode",
                "Ctrl+\\",
                PaletteCommand::ToggleFocusMode,
            ),
            (
                icon::PIN,
                "Pin / Unpin Current Note",
                "Ctrl+P",
                PaletteCommand::TogglePinSelected,
            ),
            (
                icon::COPY,
                "Copy Current Note Markdown to Clipboard",
                "",
                PaletteCommand::CopyNoteMarkdown,
            ),
            (
                icon::FILE_EXPORT,
                "Export Current Note as HTML",
                "",
                PaletteCommand::ExportNoteHtml,
            ),
            (
                icon::DOWNLOAD,
                "Export All Notes as HTML",
                "",
                PaletteCommand::ExportVaultHtml,
            ),
            (
                icon::FILE_TEXT,
                "Import Markdown From Import Folder",
                "",
                PaletteCommand::ImportMarkdown,
            ),
            (
                icon::LINK,
                "Show Note Graph",
                "",
                PaletteCommand::ToggleGraph,
            ),
            (
                icon::ARROWS_SORT,
                "Cycle Note Sort Order (Modified / Created / Title)",
                "",
                PaletteCommand::CycleSortOrder,
            ),
            (
                icon::HEADING,
                "Toggle Formatting Toolbar",
                "",
                PaletteCommand::ToggleFormattingBar,
            ),
            (
                icon::PALETTE,
                "Theme: Obsidian (Dark Slate)",
                "",
                PaletteCommand::SetTheme(ThemeMode::Obsidian),
            ),
            (
                icon::SUN,
                "Theme: Paper (Warm Light)",
                "",
                PaletteCommand::SetTheme(ThemeMode::Paper),
            ),
            (
                icon::MOON,
                "Theme: Nordic (Arctic Dark)",
                "",
                PaletteCommand::SetTheme(ThemeMode::Nordic),
            ),
            (
                icon::SPARKLES,
                "Theme: Matcha (Soft Sage Light)",
                "",
                PaletteCommand::SetTheme(ThemeMode::Matcha),
            ),
            (
                icon::PALETTE,
                "Theme: Sepia (Warm Paper)",
                "",
                PaletteCommand::SetTheme(ThemeMode::Sepia),
            ),
            (
                icon::MOON,
                "Theme: Midnight (Deep Blue)",
                "",
                PaletteCommand::SetTheme(ThemeMode::Midnight),
            ),
            (
                icon::PALETTE,
                "Theme: Matugen (From Wallpaper Colors)",
                "",
                PaletteCommand::SetTheme(ThemeMode::Matugen),
            ),
            (
                icon::TRASH,
                "Open Trash Folder",
                "",
                PaletteCommand::SwitchSection(SidebarSection::Trash),
            ),
        ];

        for (glyph, label, shortcut, cmd) in actions {
            let label_lower = label.to_lowercase();
            if q.is_empty() || label_lower.contains(&q) || fuzzy_subsequence(&label_lower, &q) {
                items.push(PaletteItem::Action {
                    icon_glyph: glyph,
                    label: (*label).to_string(),
                    shortcut,
                    cmd: *cmd,
                });
            }
        }

        items
    }

    fn render_command_palette(&mut self, ctx: &egui::Context, p: &Palette) {
        if !self.command_palette_open {
            return;
        }

        let items = self.build_palette_items();
        if self.command_palette_selected >= items.len() && !items.is_empty() {
            self.command_palette_selected = items.len() - 1;
        }

        let mut move_down = false;
        let mut move_up = false;
        let mut trigger_selected = false;

        ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::NONE, Key::ArrowDown) {
                move_down = true;
            }
            if i.consume_key(egui::Modifiers::NONE, Key::ArrowUp) {
                move_up = true;
            }
            if i.consume_key(egui::Modifiers::NONE, Key::Enter) {
                trigger_selected = true;
            }
        });

        if move_down && !items.is_empty() {
            self.command_palette_selected = (self.command_palette_selected + 1) % items.len();
        }
        if move_up && !items.is_empty() {
            self.command_palette_selected = if self.command_palette_selected == 0 {
                items.len() - 1
            } else {
                self.command_palette_selected - 1
            };
        }

        let mut activated_item: Option<PaletteItem> = None;
        if trigger_selected {
            activated_item = items.get(self.command_palette_selected).cloned();
        }

        egui::Window::new("elora_command_palette")
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, Vec2::new(0.0, 68.0))
            .fixed_size(Vec2::new(540.0, 360.0))
            .frame(
                Frame::new()
                    .fill(p.bg_sidebar)
                    .stroke(Stroke::new(1.5_f32, p.accent))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::symmetric(14, 12)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::COMMAND).color(p.accent).size(16.0));
                    let resp = ui.add(
                        TextEdit::singleline(&mut self.command_palette_query)
                            .hint_text(
                                "Type a note name or command… (↑/↓ to navigate, Enter to run)",
                            )
                            .frame(false)
                            .desired_width(440.0),
                    );
                    if self.focus_palette_next_frame {
                        resp.request_focus();
                        self.focus_palette_next_frame = false;
                    }
                    if resp.changed() {
                        self.command_palette_selected = 0;
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(icon::X).size(14.0).color(p.fg_muted),
                                )
                                .frame(false),
                            )
                            .clicked()
                        {
                            self.command_palette_open = false;
                        }
                    });
                });

                ui.add_space(6.0);
                ui.separator();
                ui.add_space(4.0);

                ScrollArea::vertical()
                    .id_salt("palette_scroll")
                    .max_height(290.0)
                    .show(ui, |ui| {
                        for (idx, item) in items.iter().enumerate() {
                            let is_sel = idx == self.command_palette_selected;
                            let row_bg = if is_sel {
                                p.bg_selected
                            } else {
                                Color32::TRANSPARENT
                            };
                            let row_stroke = if is_sel {
                                Stroke::new(1.0_f32, p.accent)
                            } else {
                                Stroke::new(1.0_f32, Color32::TRANSPARENT)
                            };

                            let frame_resp = Frame::new()
                                .fill(row_bg)
                                .stroke(row_stroke)
                                .corner_radius(CornerRadius::same(6))
                                .inner_margin(Margin::symmetric(10, 6))
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal(|ui| match item {
                                        PaletteItem::OpenNote {
                                            title,
                                            pinned,
                                            tags,
                                            ..
                                        } => {
                                            let g = if *pinned {
                                                icon::PINNED
                                            } else {
                                                icon::FILE_TEXT
                                            };
                                            let g_col =
                                                if *pinned { p.pin_color } else { p.fg_secondary };
                                            ui.label(RichText::new(g).color(g_col).size(14.5));
                                            ui.label(
                                                RichText::new(title)
                                                    .strong()
                                                    .size(13.5)
                                                    .color(p.fg_primary),
                                            );
                                            for t in tags.iter().take(2) {
                                                ui.label(
                                                    RichText::new(format!("#{t}"))
                                                        .size(11.5)
                                                        .color(p.accent),
                                                );
                                            }
                                            ui.with_layout(
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    ui.label(
                                                        RichText::new("Jump to note")
                                                            .size(11.0)
                                                            .color(p.fg_muted),
                                                    );
                                                },
                                            );
                                        }
                                        PaletteItem::Action {
                                            icon_glyph,
                                            label,
                                            shortcut,
                                            ..
                                        } => {
                                            ui.label(
                                                RichText::new(*icon_glyph)
                                                    .color(p.accent)
                                                    .size(14.5),
                                            );
                                            ui.label(
                                                RichText::new(label).size(13.5).color(p.fg_primary),
                                            );
                                            if !shortcut.is_empty() {
                                                ui.with_layout(
                                                    Layout::right_to_left(Align::Center),
                                                    |ui| {
                                                        ui.label(
                                                            RichText::new(*shortcut)
                                                                .monospace()
                                                                .size(11.0)
                                                                .color(p.fg_muted),
                                                        );
                                                    },
                                                );
                                            }
                                        }
                                    });
                                });

                            let click = ui.interact(
                                frame_resp.response.rect,
                                Id::new(("palette_row", idx)),
                                Sense::click(),
                            );
                            if click.hovered() {
                                self.command_palette_selected = idx;
                            }
                            if click.clicked() {
                                activated_item = Some(item.clone());
                            }
                        }
                    });
            });

        if let Some(item) = activated_item {
            self.command_palette_open = false;
            match item {
                PaletteItem::OpenNote { id, .. } => {
                    self.sidebar_section = SidebarSection::AllNotes;
                    self.selected_id = Some(id);
                }
                PaletteItem::Action { cmd, .. } => {
                    self.execute_palette_command(cmd, ctx);
                }
            }
        }
    }

    fn render_sidebar(&mut self, ui: &mut Ui, p: &Palette) {
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            ui.label(RichText::new(icon::FEATHER).color(p.accent).size(19.0));
            ui.label(
                RichText::new("elora")
                    .font(FontId::new(19.5, FontFamily::Proportional))
                    .strong()
                    .color(p.fg_primary),
            );
            ui.label(
                RichText::new("notes")
                    .font(FontId::new(14.5, FontFamily::Proportional))
                    .color(p.fg_muted),
            );

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let new_btn = ui
                    .add(
                        egui::Button::new(
                            RichText::new(format!("{} New", icon::PLUS))
                                .color(p.bg_canvas)
                                .strong()
                                .size(12.5),
                        )
                        .fill(p.accent)
                        .corner_radius(CornerRadius::same(6)),
                    )
                    .on_hover_text("Create new note (Ctrl+N)");
                if new_btn.clicked() {
                    self.create_note_from_template(NoteTemplate::Blank);
                }

                if ui
                    .add(
                        egui::Button::new(
                            RichText::new(icon::CALENDAR_EVENT)
                                .size(15.0)
                                .color(p.fg_secondary),
                        )
                        .frame(false),
                    )
                    .on_hover_text("Open Today's Daily Note (Ctrl+D)")
                    .clicked()
                {
                    self.open_or_create_daily_note();
                }

                if ui
                    .add(
                        egui::Button::new(
                            RichText::new(icon::COMMAND)
                                .size(15.0)
                                .color(p.fg_secondary),
                        )
                        .frame(false),
                    )
                    .on_hover_text("Command Palette & Quick Switcher (Ctrl+K)")
                    .clicked()
                {
                    self.command_palette_open = true;
                    self.command_palette_query.clear();
                    self.focus_palette_next_frame = true;
                }

                if ui
                    .add(
                        egui::Button::new(
                            RichText::new(icon::SETTINGS)
                                .size(15.0)
                                .color(p.fg_secondary),
                        )
                        .frame(false),
                    )
                    .on_hover_text("Settings (Ctrl+,)")
                    .clicked()
                {
                    self.show_settings = true;
                }

                let theme_glyph = if self.theme.is_dark() {
                    icon::SUN
                } else {
                    icon::MOON
                };
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new(theme_glyph).size(15.0).color(p.fg_secondary),
                        )
                        .frame(false),
                    )
                    .on_hover_text(format!(
                        "Theme: {} (Click to cycle Obsidian / Paper / Nordic / Matcha)",
                        self.theme.name()
                    ))
                    .clicked()
                {
                    self.theme = self.theme.next();
                    self.apply_look(ui.ctx());
                    self.persist_settings();
                }
            });
        });

        ui.add_space(8.0);

        let search_id = Id::new("elora_search_input");
        Frame::new()
            .fill(p.bg_elevated)
            .stroke(Stroke::new(1.0_f32, p.border))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::SEARCH).color(p.fg_muted).size(14.5));
                    let resp = ui.add(
                        TextEdit::singleline(&mut self.search_query)
                            .id(search_id)
                            .hint_text("Search notes or #tags… (Ctrl+F)")
                            .frame(false)
                            .desired_width(f32::INFINITY),
                    );
                    if self.focus_search_next_frame {
                        resp.request_focus();
                        self.focus_search_next_frame = false;
                    }
                    if !self.search_query.is_empty()
                        && ui
                            .add(
                                egui::Button::new(
                                    RichText::new(icon::X).size(13.5).color(p.fg_muted),
                                )
                                .frame(false),
                            )
                            .on_hover_text("Clear search (Esc)")
                            .clicked()
                    {
                        self.search_query.clear();
                    }
                });
            });

        ui.add_space(6.0);

        let active_count = self.notes.iter().filter(|n| !n.trashed).count();
        let pinned_count = self.notes.iter().filter(|n| !n.trashed && n.pinned).count();
        let trash_count = self.notes.iter().filter(|n| n.trashed).count();

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
            if nav_pill(
                ui,
                icon::NOTES,
                "Notes",
                active_count,
                self.sidebar_section == SidebarSection::AllNotes && self.active_tag.is_none(),
                p,
            )
            .clicked()
            {
                self.sidebar_section = SidebarSection::AllNotes;
                self.active_tag = None;
                self.select_first_visible();
            }

            if nav_pill(
                ui,
                icon::PINNED,
                "Pinned",
                pinned_count,
                self.sidebar_section == SidebarSection::Pinned,
                p,
            )
            .clicked()
            {
                self.sidebar_section = SidebarSection::Pinned;
                self.select_first_visible();
            }

            if (trash_count > 0 || self.sidebar_section == SidebarSection::Trash)
                && nav_pill(
                    ui,
                    icon::TRASH,
                    "Trash",
                    trash_count,
                    self.sidebar_section == SidebarSection::Trash,
                    p,
                )
                .clicked()
            {
                self.sidebar_section = SidebarSection::Trash;
                self.select_first_visible();
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new(format!(
                                "{} {}",
                                icon::ARROWS_SORT,
                                self.sort_order.label()
                            ))
                            .size(11.0)
                            .color(p.fg_muted),
                        )
                        .frame(false),
                    )
                    .on_hover_text("Change sort order")
                    .clicked()
                {
                    self.sort_order = self.sort_order.next();
                    self.resort();
                    self.persist_settings();
                }
            });
        });

        let all_tags = self.all_tags_with_counts();
        if !all_tags.is_empty() && self.sidebar_section != SidebarSection::Trash {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(5.0, 4.0);
                for (tag, count) in all_tags {
                    let is_active = self.active_tag.as_deref() == Some(tag.as_str());
                    let label = format!("{} {tag}", icon::HASH);
                    if filter_chip(ui, &label, count, is_active, p).clicked() {
                        if is_active {
                            self.active_tag = None;
                        } else {
                            self.active_tag = Some(tag);
                        }
                    }
                }
            });
        }

        ui.add_space(6.0);
        ui.separator();
        ui.add_space(4.0);

        let filtered = self.filtered_indices();
        if filtered.is_empty() {
            ui.add_space(28.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(icon::NOTES).size(26.0).color(p.fg_muted));
                ui.add_space(4.0);
                let msg = if self.sidebar_section == SidebarSection::Trash {
                    "Trash is empty"
                } else {
                    "No matching notes"
                };
                ui.label(RichText::new(msg).color(p.fg_secondary));
                if !self.search_query.is_empty() || self.active_tag.is_some() {
                    ui.add_space(4.0);
                    if ui
                        .small_button(format!("{} Reset filters", icon::RESTORE))
                        .clicked()
                    {
                        self.search_query.clear();
                        self.active_tag = None;
                    }
                }
            });
            return;
        }

        let pinned_indices: Vec<usize> = filtered
            .iter()
            .copied()
            .filter(|&i| self.notes[i].pinned && !self.notes[i].trashed)
            .collect();
        let unpinned_indices: Vec<usize> = filtered
            .iter()
            .copied()
            .filter(|&i| !self.notes[i].pinned || self.notes[i].trashed)
            .collect();

        let mut clicked_note_id: Option<String> = None;
        let mut clicked_tag: Option<String> = None;

        ScrollArea::vertical()
            .id_salt("notes_sidebar_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if !pinned_indices.is_empty() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(icon::PINNED).size(11.0).color(p.pin_color));
                        ui.label(
                            RichText::new("PINNED")
                                .size(10.5)
                                .strong()
                                .color(p.fg_muted),
                        );
                    });
                    ui.add_space(2.0);
                    for idx in &pinned_indices {
                        let note = &self.notes[*idx];
                        let is_selected = self.selected_id.as_deref() == Some(note.id.as_str());
                        if let Some(action) =
                            render_note_card(ui, note, is_selected, p, &self.search_query)
                        {
                            match action {
                                CardAction::SelectNote(id) => clicked_note_id = Some(id),
                                CardAction::FilterTag(t) => clicked_tag = Some(t),
                            }
                        }
                        ui.add_space(4.0);
                    }
                    if !unpinned_indices.is_empty() {
                        ui.add_space(6.0);
                    }
                }

                if !unpinned_indices.is_empty() {
                    if !pinned_indices.is_empty() {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(icon::NOTES).size(11.0).color(p.fg_muted));
                            ui.label(
                                RichText::new("ALL NOTES")
                                    .size(10.5)
                                    .strong()
                                    .color(p.fg_muted),
                            );
                        });
                        ui.add_space(2.0);
                    }
                    for idx in &unpinned_indices {
                        let note = &self.notes[*idx];
                        let is_selected = self.selected_id.as_deref() == Some(note.id.as_str());
                        if let Some(action) =
                            render_note_card(ui, note, is_selected, p, &self.search_query)
                        {
                            match action {
                                CardAction::SelectNote(id) => clicked_note_id = Some(id),
                                CardAction::FilterTag(t) => clicked_tag = Some(t),
                            }
                        }
                        ui.add_space(4.0);
                    }
                }
            });

        if let Some(id) = clicked_note_id {
            self.selected_id = Some(id);
            self.confirm_delete_id = None;
        }
        if let Some(tag) = clicked_tag {
            if self.active_tag.as_deref() == Some(tag.as_str()) {
                self.active_tag = None;
            } else {
                self.active_tag = Some(tag);
            }
        }
    }

    /// Renders the right-click context menu on the note editor with Bold, Italic, Glowing Highlight,
    /// Color Picker, Presets, and Markdown structure tools.
    fn render_editor_context_menu(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        queued_op: &mut Option<EditorFormatOp>,
    ) {
        ui.set_min_width(240.0);

        let has_selection = self.last_editor_selection.is_some_and(|(a, b)| a != b);
        let header_txt = if has_selection {
            "Format Selection"
        } else {
            "Insert Formatting"
        };
        ui.label(
            RichText::new(format!("{} {header_txt}", icon::FLARE))
                .size(11.5)
                .strong()
                .color(p.fg_muted),
        );
        ui.add_space(2.0);

        if ui
            .button(RichText::new(format!("{}  Bold  (Ctrl+B)", icon::BOLD)).size(13.0))
            .clicked()
        {
            *queued_op = Some(EditorFormatOp::Bold);
            ui.close_menu();
        }
        if ui
            .button(RichText::new(format!("{}  Italic  (Ctrl+I)", icon::ITALIC)).size(13.0))
            .clicked()
        {
            *queued_op = Some(EditorFormatOp::Italic);
            ui.close_menu();
        }
        if ui
            .button(RichText::new(format!("{}  Underline", icon::UNDERLINE)).size(13.0))
            .clicked()
        {
            *queued_op = Some(EditorFormatOp::Underline);
            ui.close_menu();
        }
        if ui
            .button(RichText::new(format!("{}  Strikethrough", icon::STRIKETHROUGH)).size(13.0))
            .clicked()
        {
            *queued_op = Some(EditorFormatOp::Strikethrough);
            ui.close_menu();
        }
        if ui
            .button(RichText::new(format!("{}  Inline Code", icon::CODE)).size(13.0))
            .clicked()
        {
            *queued_op = Some(EditorFormatOp::InlineCode);
            ui.close_menu();
        }

        ui.separator();

        // Glowing Highlight Section + Color Picker + Quick Swatches
        let hl_col = Color32::from_rgb(
            self.highlight_rgb[0],
            self.highlight_rgb[1],
            self.highlight_rgb[2],
        );
        if ui
            .button(
                RichText::new(format!(
                    "{}  Glow Highlight  (Ctrl+Shift+H)",
                    icon::HIGHLIGHT
                ))
                .strong()
                .color(hl_col)
                .size(13.0),
            )
            .clicked()
        {
            *queued_op = Some(EditorFormatOp::GlowHighlight(self.highlight_rgb));
            ui.close_menu();
        }

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            ui.label(
                RichText::new(format!("{} Glow Color:", icon::COLOR_PICKER))
                    .size(11.5)
                    .color(p.fg_secondary),
            );
            if ui.color_edit_button_srgb(&mut self.highlight_rgb).changed() {
                self.persist_settings();
            }
            if ui
                .small_button(RichText::new("Apply").color(hl_col))
                .on_hover_text("Highlight selection with picked color")
                .clicked()
            {
                *queued_op = Some(EditorFormatOp::GlowHighlight(self.highlight_rgb));
                ui.close_menu();
            }
        });

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for (name, rgb) in HIGHLIGHT_PRESETS {
                let is_sel = self.highlight_rgb == *rgb;
                if color_swatch_button(
                    ui,
                    *rgb,
                    is_sel,
                    &format!("{name} ({}) — click to highlight", rgb_to_hex(*rgb)),
                )
                .clicked()
                {
                    self.highlight_rgb = *rgb;
                    *queued_op = Some(EditorFormatOp::GlowHighlight(*rgb));
                    ui.close_menu();
                }
            }
        });

        ui.separator();

        if ui
            .button(RichText::new(format!("{}  Make [[Wiki Link]]", icon::LINK)).size(13.0))
            .clicked()
        {
            *queued_op = Some(EditorFormatOp::WikiLink);
            ui.close_menu();
        }
        if ui
            .button(RichText::new(format!("{}  Heading 1", icon::H1)).size(13.0))
            .clicked()
        {
            *queued_op = Some(EditorFormatOp::Heading1);
            ui.close_menu();
        }
        if ui
            .button(RichText::new(format!("{}  Heading 2", icon::H2)).size(13.0))
            .clicked()
        {
            *queued_op = Some(EditorFormatOp::Heading2);
            ui.close_menu();
        }
        if ui
            .button(RichText::new(format!("{}  Task Checkbox", icon::LIST_CHECK)).size(13.0))
            .clicked()
        {
            *queued_op = Some(EditorFormatOp::ChecklistItem);
            ui.close_menu();
        }
        if ui
            .button(RichText::new(format!("{}  Blockquote", icon::QUOTE)).size(13.0))
            .clicked()
        {
            *queued_op = Some(EditorFormatOp::Blockquote);
            ui.close_menu();
        }
        if ui
            .button(RichText::new(format!("{}  Code Block", icon::CODE)).size(13.0))
            .clicked()
        {
            *queued_op = Some(EditorFormatOp::CodeFence);
            ui.close_menu();
        }

        if has_selection {
            ui.separator();
            if ui
                .button(
                    RichText::new(format!("{}  Clear Formatting", icon::CLEAR_FORMATTING))
                        .size(12.5)
                        .color(p.fg_secondary),
                )
                .clicked()
            {
                *queued_op = Some(EditorFormatOp::ClearFormatting);
                ui.close_menu();
            }
        }
    }

    /// Renders a live syntax-styled `TextEdit::multiline` widget with selection tracking
    /// and a right-click formatting menu.
    fn render_multiline_markdown_editor(
        &mut self,
        ui: &mut Ui,
        note_idx: usize,
        min_height: f32,
        p: &Palette,
        needs_save: &mut bool,
        queued_op: &mut Option<EditorFormatOp>,
    ) {
        let note_id = self.notes[note_idx].id.clone();
        let editor_id = Id::new(("md_editor", note_id.clone()));

        // 1. Record edits made since the last frame (typing, formatting, context-menu actions).
        //    Rapid typing is coalesced into a single undo step.
        if let Some((seen_id, seen_body)) = &self.last_seen_body {
            if *seen_id == note_id && *seen_body != self.notes[note_idx].body {
                let hist = self.edit_history.entry(note_id.clone()).or_default();
                let now = Instant::now();
                let coalesce = hist
                    .last_change
                    .is_some_and(|t| now.duration_since(t) < Duration::from_millis(800));
                if !coalesce {
                    hist.undo.push(seen_body.clone());
                    if hist.undo.len() > 200 {
                        hist.undo.remove(0);
                    }
                }
                hist.redo.clear();
                hist.last_change = Some(now);
            }
        }

        // 2. Undo / redo while the editor has focus. Consumed here so the TextEdit never sees them.
        if ui.memory(|m| m.has_focus(editor_id)) {
            let redo = ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::Z)
                    || i.consume_key(egui::Modifiers::COMMAND, Key::Y)
            });
            let undo = !redo && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Z));
            if undo || redo {
                let hist = self.edit_history.entry(note_id.clone()).or_default();
                let target = if undo {
                    hist.undo.pop()
                } else {
                    hist.redo.pop()
                };
                if let Some(prev) = target {
                    let current = std::mem::replace(&mut self.notes[note_idx].body, prev);
                    if undo {
                        hist.redo.push(current);
                    } else {
                        hist.undo.push(current);
                    }
                    hist.last_change = None;
                    self.notes[note_idx].updated_at = Utc::now();
                    *needs_save = true;
                }
            }
        }

        self.last_seen_body = Some((note_id, self.notes[note_idx].body.clone()));

        let is_dark = self.theme.is_dark();
        let p_copy = *p;
        let hl_rgb = self.highlight_rgb;

        let mut layouter =
            move |ui: &Ui, text: &str, wrap_width: f32| -> std::sync::Arc<egui::Galley> {
                let job = build_editor_layout_job(text, wrap_width, &p_copy, is_dark, hl_rgb);
                ui.fonts(|f| f.layout_job(job))
            };

        let available = ui.available_size();
        let output = TextEdit::multiline(&mut self.notes[note_idx].body)
            .id(editor_id)
            .hint_text(
                "Write in Markdown… Select any text & Right-Click to Bold, Glow Highlight, or Link.",
            )
            .font(TextStyle::Body)
            .frame(false)
            .desired_width(f32::INFINITY)
            .min_size(Vec2::new(available.x, min_height))
            .layouter(&mut layouter)
            .show(ui);

        if let Some(cursor_range) = output.cursor_range {
            let a = cursor_range.primary.ccursor.index;
            let b = cursor_range.secondary.ccursor.index;
            self.last_editor_selection = Some((a.min(b), a.max(b)));
        }

        if output.response.changed() {
            self.notes[note_idx].updated_at = Utc::now();
            *needs_save = true;
        }

        output.response.context_menu(|ui| {
            self.render_editor_context_menu(ui, p, queued_op);
        });
    }

    /// Floating, searchable Tabler icon picker for the selected note.
    /// Returns `Some(Some(name))` to set an icon, `Some(None)` to remove it.
    fn render_icon_picker(
        &mut self,
        ctx: &egui::Context,
        p: &Palette,
        note_idx: usize,
    ) -> Option<Option<String>> {
        if !self.icon_picker_open {
            return None;
        }
        // The picker targets the note it was opened for; close it if the user switched notes.
        if self.icon_picker_note_id.as_deref() != Some(self.notes[note_idx].id.as_str()) {
            self.icon_picker_open = false;
            self.icon_picker_for_body = false;
            self.icon_picker_note_id = None;
            return None;
        }
        let current = if self.icon_picker_for_body {
            None
        } else {
            self.notes[note_idx].icon.clone()
        };
        let query = self.icon_search.trim().to_lowercase().replace(' ', "-");
        let mut choice: Option<Option<String>> = None;
        let mut open = true;

        let title = if self.icon_picker_for_body {
            "Insert icon"
        } else {
            "Note icon"
        };
        egui::Window::new(title)
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, Vec2::new(0.0, 68.0))
            .fixed_size(Vec2::new(420.0, 380.0))
            .frame(
                Frame::new()
                    .fill(p.bg_sidebar)
                    .stroke(Stroke::new(1.5_f32, p.accent))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::symmetric(14, 12)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::SEARCH).color(p.accent).size(16.0));
                    ui.add(
                        TextEdit::singleline(&mut self.icon_search)
                            .hint_text("Search Tabler icons… (e.g. book, star, flare)")
                            .frame(false)
                            .desired_width(340.0),
                    );
                    if current.is_some()
                        && ui
                            .button(RichText::new("Remove").size(12.0).color(p.danger))
                            .clicked()
                    {
                        choice = Some(None);
                    }
                });
                ui.separator();

                let matches: Vec<(&str, &str)> = TABLER_ICON_NAMES
                    .iter()
                    .filter(|(name, _)| query.is_empty() || name.contains(query.as_str()))
                    .copied()
                    .collect();
                let total = matches.len();
                ui.label(
                    RichText::new(format!("{total} icons"))
                        .size(11.0)
                        .color(p.fg_muted),
                );

                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            for &(name, glyph) in matches.iter().take(400) {
                                let selected = current.as_deref() == Some(name);
                                let btn = egui::Button::new(
                                    RichText::new(glyph).size(18.0).color(if selected {
                                        p.accent
                                    } else {
                                        p.fg_primary
                                    }),
                                )
                                .frame(selected)
                                .min_size(Vec2::splat(34.0));
                                if ui.add(btn).on_hover_text(name).clicked() {
                                    choice = Some(Some(name.to_string()));
                                }
                            }
                        });
                    });
            });

        if !open || choice.is_some() {
            self.icon_picker_open = false;
            self.icon_picker_for_body = false;
            self.icon_picker_note_id = None;
            self.icon_search.clear();
        }
        choice
    }

    fn render_editor(&mut self, ui: &mut Ui, p: &Palette) {
        let Some(selected_id) = self.selected_id.clone() else {
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() * 0.30);
                    ui.label(RichText::new(icon::FEATHER).size(32.0).color(p.fg_muted));
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("Select or create a note")
                            .size(18.0)
                            .color(p.fg_secondary),
                    );
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        let pad = (ui.available_width() - 320.0).max(0.0) * 0.5;
                        ui.add_space(pad);
                        if ui
                            .button(
                                RichText::new(format!("{} New Blank Note (Ctrl+N)", icon::PLUS))
                                    .color(p.accent),
                            )
                            .clicked()
                        {
                            self.create_note_from_template(NoteTemplate::Blank);
                        }
                        if ui
                            .button(
                                RichText::new(format!(
                                    "{} Daily Note (Ctrl+D)",
                                    icon::CALENDAR_EVENT
                                ))
                                .color(p.fg_primary),
                            )
                            .clicked()
                        {
                            self.open_or_create_daily_note();
                        }
                    });
                });
            });
            return;
        };

        let Some(note_idx) = self.notes.iter().position(|n| n.id == selected_id) else {
            return;
        };

        let mut needs_save = false;
        let mut needs_resort = false;
        let mut action_move_trash = false;
        let mut action_restore_trash = false;
        let mut action_perm_delete = false;
        let mut queued_format_op: Option<EditorFormatOp> = None;

        let is_trashed = self.notes[note_idx].trashed;

        // Top Minimal Action Bar with Tabler Icons
        ui.horizontal(|ui| {
            let (focus_icon, focus_label) = if self.focus_mode {
                (icon::SIDEBAR_EXPAND, "Sidebar")
            } else {
                (icon::SIDEBAR_COLLAPSE, "Focus")
            };
            if ui
                .add(
                    egui::Button::new(
                        RichText::new(format!("{focus_icon} {focus_label}"))
                            .size(12.5)
                            .color(p.fg_secondary),
                    )
                    .frame(false),
                )
                .on_hover_text("Toggle distraction-free focus mode (Ctrl+\\)")
                .clicked()
            {
                self.focus_mode = !self.focus_mode;
            }

            ui.label(RichText::new("·").color(p.fg_muted));

            // Note icon picker button
            let note_icon = self.notes[note_idx].icon.clone();
            let icon_glyph = note_icon
                .as_deref()
                .and_then(tabler_glyph)
                .unwrap_or(icon::PLUS);
            let icon_tooltip = match note_icon.as_deref() {
                Some(name) => format!("Note icon: {name} (click to change)"),
                None => "Add an icon to this note".to_string(),
            };
            if ui
                .add(
                    egui::Button::new(RichText::new(icon_glyph).size(13.0).color(
                        if note_icon.is_some() {
                            p.accent
                        } else {
                            p.fg_secondary
                        },
                    ))
                    .frame(self.icon_picker_open),
                )
                .on_hover_text(icon_tooltip)
                .clicked()
            {
                self.icon_picker_for_body = false;
                self.icon_picker_open = !self.icon_picker_open;
                self.icon_picker_note_id = Some(self.notes[note_idx].id.clone());
            }

            ui.label(RichText::new("·").color(p.fg_muted));

            let updated_text = self.notes[note_idx].formatted_updated();
            let words = self.notes[note_idx].word_count();
            let chars = self.notes[note_idx].char_count();
            let read_mins = self.notes[note_idx].reading_time_minutes();
            let (done_tasks, total_tasks) = self.notes[note_idx].task_progress();

            let mut stats = format!(
                "{} {}   ·   {} words ({} chars)",
                icon::CLOCK,
                updated_text,
                words,
                chars
            );
            if read_mins > 0 {
                stats.push_str(&format!(" (~{read_mins}m read)"));
            }
            if total_tasks > 0 {
                stats.push_str(&format!(
                    "   ·   {} {done_tasks}/{total_tasks}",
                    icon::LIST_CHECK
                ));
            }
            ui.label(RichText::new(stats).size(12.0).color(p.fg_muted));

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if is_trashed {
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(format!("{} Delete Forever", icon::TRASH_X))
                                    .size(12.0)
                                    .color(Color32::WHITE),
                            )
                            .fill(p.danger),
                        )
                        .clicked()
                    {
                        action_perm_delete = true;
                    }
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(format!("{} Restore", icon::RESTORE))
                                    .size(12.5)
                                    .color(p.accent),
                            )
                            .frame(true),
                        )
                        .clicked()
                    {
                        action_restore_trash = true;
                    }
                } else if self.confirm_delete_id.as_deref() == Some(selected_id.as_str()) {
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(format!("{} Move to Trash", icon::TRASH))
                                    .size(12.0)
                                    .color(Color32::WHITE),
                            )
                            .fill(p.danger),
                        )
                        .clicked()
                    {
                        action_move_trash = true;
                    }
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new("Cancel").size(12.0).color(p.fg_secondary),
                            )
                            .frame(false),
                        )
                        .clicked()
                    {
                        self.confirm_delete_id = None;
                    }
                } else {
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(icon::TRASH).size(14.5).color(p.fg_muted),
                            )
                            .frame(false),
                        )
                        .on_hover_text("Move note to Trash")
                        .clicked()
                    {
                        self.confirm_delete_id = Some(selected_id.clone());
                    }

                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(icon::COPY).size(14.5).color(p.fg_secondary),
                            )
                            .frame(false),
                        )
                        .on_hover_text("Copy Markdown to clipboard")
                        .clicked()
                    {
                        ui.ctx().copy_text(format!(
                            "# {}\n\n{}",
                            self.notes[note_idx].display_title(),
                            self.notes[note_idx].body
                        ));
                        self.status_message = Some("Copied Markdown to clipboard".to_string());
                    }
                }

                ui.add_space(4.0);

                for (mode, glyph, label, hint) in [
                    (
                        EditorViewMode::Preview,
                        icon::EYE,
                        "Preview",
                        "Clean Markdown Preview (Ctrl+E)",
                    ),
                    (
                        EditorViewMode::Split,
                        icon::COLUMNS,
                        "Split",
                        "Side-by-side Editor & Live Preview (Ctrl+Shift+E)",
                    ),
                    (
                        EditorViewMode::Edit,
                        icon::PENCIL,
                        "Edit",
                        "Markdown Editor",
                    ),
                ] {
                    let active = self.view_mode == mode;
                    let col = if active { p.accent } else { p.fg_secondary };
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(format!("{glyph} {label}"))
                                    .size(12.0)
                                    .color(col),
                            )
                            .frame(active),
                        )
                        .on_hover_text(hint)
                        .clicked()
                    {
                        self.view_mode = mode;
                    }
                }

                ui.add_space(4.0);

                if !is_trashed {
                    let is_pinned = self.notes[note_idx].pinned;
                    let (pin_glyph, pin_text) = if is_pinned {
                        (icon::PINNED, "Pinned")
                    } else {
                        (icon::PIN, "Pin")
                    };
                    let pin_color = if is_pinned {
                        p.pin_color
                    } else {
                        p.fg_secondary
                    };
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(format!("{pin_glyph} {pin_text}"))
                                    .size(12.0)
                                    .color(pin_color),
                            )
                            .frame(is_pinned),
                        )
                        .on_hover_text("Pin note to top of list (Ctrl+P)")
                        .clicked()
                    {
                        self.notes[note_idx].pinned = !is_pinned;
                        self.notes[note_idx].updated_at = Utc::now();
                        needs_save = true;
                        needs_resort = true;
                    }
                }
            });
        });

        let inserting = self.icon_picker_for_body;
        if let Some(choice) = self.render_icon_picker(ui.ctx(), p, note_idx) {
            if inserting {
                if let Some(glyph) = choice.as_deref().and_then(tabler_glyph) {
                    let range = self.last_editor_selection;
                    insert_at_selection(&mut self.notes[note_idx].body, range, glyph);
                    self.notes[note_idx].updated_at = Utc::now();
                    needs_save = true;
                }
            } else {
                self.notes[note_idx].icon = choice;
                self.notes[note_idx].updated_at = Utc::now();
                needs_save = true;
            }
        }

        ui.add_space(8.0);

        // Title Field
        let title_id = Id::new("elora_note_title_input");
        let title_resp = ui.add(
            TextEdit::singleline(&mut self.notes[note_idx].title)
                .id(title_id)
                .hint_text("Note title…")
                .font(TextStyle::Name("NoteTitle".into()))
                .frame(false)
                .desired_width(f32::INFINITY),
        );
        if self.focus_title_next_frame {
            title_resp.request_focus();
            self.focus_title_next_frame = false;
        }
        if title_resp.changed() {
            self.notes[note_idx].updated_at = Utc::now();
            needs_save = true;
        }

        ui.add_space(4.0);

        // Tag Row
        let current_tags = self.notes[note_idx].all_tags();
        let explicit_tags = self.notes[note_idx].explicit_tags.clone();
        let mut tag_to_remove: Option<String> = None;
        let mut tag_to_filter: Option<String> = None;

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
            ui.label(RichText::new(icon::TAGS).size(13.5).color(p.fg_muted));

            for tag in &current_tags {
                let is_explicit = explicit_tags.iter().any(|t| normalize_tag(t) == *tag);
                let is_active_filter = self.active_tag.as_deref() == Some(tag.as_str());

                Frame::new()
                    .fill(if is_active_filter {
                        p.accent_soft
                    } else {
                        p.bg_elevated
                    })
                    .stroke(Stroke::new(
                        1.0_f32,
                        if is_active_filter { p.accent } else { p.border },
                    ))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::symmetric(8, 2))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing = Vec2::new(4.0, 0.0);
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(format!("#{tag}")).size(11.5).color(p.accent),
                                    )
                                    .frame(false),
                                )
                                .on_hover_text("Click to filter by this tag")
                                .clicked()
                            {
                                tag_to_filter = Some(tag.clone());
                            }
                            if is_explicit
                                && ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new(icon::X).size(10.5).color(p.fg_muted),
                                        )
                                        .frame(false),
                                    )
                                    .on_hover_text("Remove tag")
                                    .clicked()
                            {
                                tag_to_remove = Some(tag.clone());
                            }
                        });
                    });
            }

            let tag_edit = ui.add(
                TextEdit::singleline(&mut self.new_tag_input)
                    .hint_text("+ add #tag…")
                    .font(TextStyle::Small)
                    .frame(false)
                    .desired_width(95.0),
            );
            let submitted = tag_edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            if submitted && !self.new_tag_input.trim().is_empty() {
                let added = self.notes[note_idx].add_tag(&self.new_tag_input);
                self.new_tag_input.clear();
                if added {
                    needs_save = true;
                }
                tag_edit.request_focus();
            }
        });

        if let Some(tag) = tag_to_remove {
            if self.notes[note_idx].remove_explicit_tag(&tag) {
                needs_save = true;
            }
        }
        if let Some(tag) = tag_to_filter {
            if self.active_tag.as_deref() == Some(tag.as_str()) {
                self.active_tag = None;
            } else {
                self.active_tag = Some(tag);
            }
        }

        // Formatting Bar (works on current selection + includes Glowing Highlight & Color Picker)
        if self.show_formatting_bar && self.view_mode != EditorViewMode::Preview {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(4.0, 3.0);
                for (glyph, tooltip, op) in [
                    (icon::H1, "Heading 1", EditorFormatOp::Heading1),
                    (icon::H2, "Heading 2", EditorFormatOp::Heading2),
                    (icon::BOLD, "Bold selection (Ctrl+B)", EditorFormatOp::Bold),
                    (
                        icon::ITALIC,
                        "Italic selection (Ctrl+I)",
                        EditorFormatOp::Italic,
                    ),
                    (
                        icon::UNDERLINE,
                        "Underline selection",
                        EditorFormatOp::Underline,
                    ),
                    (
                        icon::STRIKETHROUGH,
                        "Strikethrough selection",
                        EditorFormatOp::Strikethrough,
                    ),
                    (icon::CODE, "Inline Code", EditorFormatOp::InlineCode),
                    (icon::LIST, "Bullet List", EditorFormatOp::BulletItem),
                    (
                        icon::LIST_CHECK,
                        "Task Checkbox",
                        EditorFormatOp::ChecklistItem,
                    ),
                    (icon::QUOTE, "Blockquote", EditorFormatOp::Blockquote),
                    (
                        icon::LINK,
                        "Wrap in [[Wiki Link]]",
                        EditorFormatOp::WikiLink,
                    ),
                    (
                        icon::CLEAR_FORMATTING,
                        "Clear formatting on selection",
                        EditorFormatOp::ClearFormatting,
                    ),
                ] {
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(glyph).size(13.5).color(p.fg_secondary),
                            )
                            .fill(p.bg_elevated)
                            .corner_radius(CornerRadius::same(5)),
                        )
                        .on_hover_text(tooltip)
                        .clicked()
                    {
                        queued_format_op = Some(op);
                    }
                }

                if ui
                    .add(
                        egui::Button::new(
                            RichText::new(icon::SPARKLES)
                                .size(13.5)
                                .color(p.fg_secondary),
                        )
                        .fill(p.bg_elevated)
                        .corner_radius(CornerRadius::same(5)),
                    )
                    .on_hover_text("Insert a Tabler icon at the cursor")
                    .clicked()
                {
                    self.icon_picker_for_body = true;
                    self.icon_picker_open = true;
                    self.icon_picker_note_id = Some(self.notes[note_idx].id.clone());
                    self.icon_search.clear();
                }

                ui.add_space(4.0);
                ui.label(RichText::new("·").color(p.fg_muted));
                ui.add_space(2.0);

                // Text color: color picker + apply button wraps selection in {color:#HEX}...{/color}
                if ui
                    .color_edit_button_srgb(&mut self.text_color_rgb)
                    .on_hover_text("Pick text color")
                    .changed()
                {
                    self.persist_settings();
                }
                if ui
                    .add(
                        egui::Button::new(RichText::new("A Text Color").size(12.0).color(
                            Color32::from_rgb(
                                self.text_color_rgb[0],
                                self.text_color_rgb[1],
                                self.text_color_rgb[2],
                            ),
                        ))
                        .fill(p.bg_elevated)
                        .corner_radius(CornerRadius::same(6)),
                    )
                    .on_hover_text("Color the selected text with the chosen color")
                    .clicked()
                {
                    queued_format_op = Some(EditorFormatOp::TextColor(self.text_color_rgb));
                }

                ui.add_space(4.0);
                ui.label(RichText::new("·").color(p.fg_muted));
                ui.add_space(2.0);

                // Glow Highlight button + Color Picker + Quick Swatches
                let hl_col = Color32::from_rgb(
                    self.highlight_rgb[0],
                    self.highlight_rgb[1],
                    self.highlight_rgb[2],
                );
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new(format!("{} Glow Highlight", icon::HIGHLIGHT))
                                .size(12.0)
                                .strong()
                                .color(hl_col),
                        )
                        .fill(p.bg_elevated)
                        .stroke(Stroke::new(1.0_f32, hl_col))
                        .corner_radius(CornerRadius::same(6)),
                    )
                    .on_hover_text(
                        "Apply glowing highlight to selected text (Ctrl+Shift+H or Right-Click)",
                    )
                    .clicked()
                {
                    queued_format_op = Some(EditorFormatOp::GlowHighlight(self.highlight_rgb));
                }

                // Live Color Picker button
                if ui
                    .color_edit_button_srgb(&mut self.highlight_rgb)
                    .on_hover_text("Pick custom glow highlight color")
                    .changed()
                {
                    self.persist_settings();
                }

                // Preset Glow Color Swatches
                for (name, rgb) in HIGHLIGHT_PRESETS {
                    let is_sel = self.highlight_rgb == *rgb;
                    if color_swatch_button(
                        ui,
                        *rgb,
                        is_sel,
                        &format!(
                            "{name} ({}) — click to highlight selection",
                            rgb_to_hex(*rgb)
                        ),
                    )
                    .clicked()
                    {
                        self.highlight_rgb = *rgb;
                        queued_format_op = Some(EditorFormatOp::GlowHighlight(*rgb));
                    }
                }
            });
        }

        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);

        let backlinks = self.backlinks_for(note_idx);
        let mut clicked_wiki_link: Option<String> = None;
        let mut clicked_backlink_id: Option<String> = None;
        let mut toggled_task_idx: Option<usize> = None;
        let is_dark = self.theme.is_dark();

        match self.view_mode {
            EditorViewMode::Edit => {
                ScrollArea::vertical()
                    .id_salt("elora_editor_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let min_h = (ui.available_height() - 70.0).max(260.0);
                        self.render_multiline_markdown_editor(
                            ui,
                            note_idx,
                            min_h,
                            p,
                            &mut needs_save,
                            &mut queued_format_op,
                        );
                        render_backlinks_footer(ui, &backlinks, p, &mut clicked_backlink_id);
                    });
            }
            EditorViewMode::Preview => {
                ScrollArea::vertical()
                    .id_salt("elora_preview_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let body_snapshot = self.notes[note_idx].body.clone();
                        render_markdown_preview(
                            ui,
                            &body_snapshot,
                            p,
                            is_dark,
                            &mut clicked_wiki_link,
                            &mut toggled_task_idx,
                        );
                        render_backlinks_footer(ui, &backlinks, p, &mut clicked_backlink_id);
                    });
            }
            EditorViewMode::Split => {
                ui.columns(2, |cols| {
                    cols[0].vertical(|ui| {
                        ScrollArea::vertical()
                            .id_salt("elora_split_left")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                let min_h = ui.available_height().max(300.0);
                                self.render_multiline_markdown_editor(
                                    ui,
                                    note_idx,
                                    min_h,
                                    p,
                                    &mut needs_save,
                                    &mut queued_format_op,
                                );
                            });
                    });
                    cols[1].vertical(|ui| {
                        ScrollArea::vertical()
                            .id_salt("elora_split_right")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                let body_snapshot = self.notes[note_idx].body.clone();
                                render_markdown_preview(
                                    ui,
                                    &body_snapshot,
                                    p,
                                    is_dark,
                                    &mut clicked_wiki_link,
                                    &mut toggled_task_idx,
                                );
                                render_backlinks_footer(
                                    ui,
                                    &backlinks,
                                    p,
                                    &mut clicked_backlink_id,
                                );
                            });
                    });
                });
            }
        }

        if let Some(op) = queued_format_op {
            self.apply_editor_format(note_idx, op);
        }

        if let Some(task_i) = toggled_task_idx {
            if self.notes[note_idx].toggle_task_at(task_i) {
                needs_save = true;
            }
        }

        if needs_save {
            if let Err(e) = self.storage.save_note(&self.notes[note_idx]) {
                self.status_message = Some(format!("Auto-save error: {e}"));
            }
        }
        if needs_resort {
            self.resort();
        }
        if action_move_trash {
            self.confirm_delete_id = None;
            self.move_to_trash(&selected_id);
        }
        if action_restore_trash {
            self.restore_from_trash(&selected_id);
        }
        if action_perm_delete {
            self.permanently_delete_by_id(&selected_id);
        }
        if let Some(target) = clicked_wiki_link {
            self.open_or_create_wiki_link(&target);
        }
        if let Some(back_id) = clicked_backlink_id {
            self.selected_id = Some(back_id);
        }
    }
}

impl eframe::App for EloraApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_shortcuts(ctx);
        self.settings_window(ctx);
        self.graph_window(ctx);
        let p = self.theme.palette_with(&self.colors, self.glow_strength);

        egui::TopBottomPanel::bottom("elora_status_bar")
            .frame(
                Frame::new()
                    .fill(p.bg_sidebar)
                    .stroke(Stroke::new(1.0_f32, p.border))
                    .inner_margin(Margin::symmetric(14, 5)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let path_display = self.storage.root_dir().display().to_string();
                    ui.label(
                        RichText::new(format!("{} {path_display}", icon::FOLDER))
                            .size(11.0)
                            .color(p.fg_muted),
                    );

                    ui.label(RichText::new("·").size(11.0).color(p.fg_muted));
                    ui.label(
                        RichText::new(format!("{} {}", icon::PALETTE, self.theme.name()))
                            .size(11.0)
                            .color(p.fg_muted),
                    );

                    if let Some(msg) = &self.status_message {
                        ui.label(RichText::new("·").size(11.0).color(p.fg_muted));
                        ui.label(
                            RichText::new(format!("{} {msg}", icon::CHECK))
                                .size(11.0)
                                .color(p.accent),
                        );
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!(
                                "{} Right-Click Selection to Format/Glow  ·  Ctrl+B Bold  ·  Ctrl+Shift+H Highlight  ·  Ctrl+K Palette",
                                icon::FLARE
                            ))
                            .size(11.0)
                            .color(p.fg_muted),
                        );
                    });
                });
            });

        if !self.focus_mode {
            egui::SidePanel::left("elora_sidebar")
                .resizable(true)
                .default_width(305.0)
                .width_range(240.0..=450.0)
                .frame(
                    Frame::new()
                        .fill(p.bg_sidebar)
                        .stroke(Stroke::new(1.0_f32, p.border))
                        .inner_margin(Margin::symmetric(12, 10)),
                )
                .show(ctx, |ui| {
                    self.render_sidebar(ui, &p);
                });
        }

        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(p.bg_canvas)
                    .inner_margin(Margin::symmetric(28, 18)),
            )
            .show(ctx, |ui| {
                let max_width = if self.focus_mode {
                    820.0
                } else if self.view_mode == EditorViewMode::Split {
                    1320.0
                } else {
                    980.0
                };
                if ui.available_width() > max_width {
                    let pad = ((ui.available_width() - max_width) * 0.5).min(180.0);
                    Frame::new()
                        .inner_margin(Margin::symmetric(pad as i8, 0))
                        .show(ui, |ui| {
                            self.render_editor(ui, &p);
                        });
                } else {
                    self.render_editor(ui, &p);
                }
            });

        self.render_command_palette(ctx, &p);
    }
}

enum CardAction {
    SelectNote(String),
    FilterTag(String),
}

fn nav_pill(
    ui: &mut Ui,
    glyph: &str,
    label: &str,
    count: usize,
    active: bool,
    p: &Palette,
) -> egui::Response {
    let text = format!("{glyph} {label} ({count})");
    let fg = if active { p.accent } else { p.fg_secondary };
    let bg = if active { p.accent_soft } else { p.bg_elevated };
    let border = if active { p.accent } else { p.border };

    ui.add(
        egui::Button::new(RichText::new(text).size(11.5).color(fg))
            .fill(bg)
            .stroke(Stroke::new(1.0_f32, border))
            .corner_radius(CornerRadius::same(8)),
    )
}

fn filter_chip(
    ui: &mut Ui,
    label: &str,
    count: usize,
    active: bool,
    p: &Palette,
) -> egui::Response {
    let text = format!("{label} ({count})");
    let fg = if active { p.accent } else { p.fg_secondary };
    let bg = if active { p.accent_soft } else { p.bg_elevated };
    let border = if active { p.accent } else { p.border };

    ui.add(
        egui::Button::new(RichText::new(text).size(11.0).color(fg))
            .fill(bg)
            .stroke(Stroke::new(1.0_f32, border))
            .corner_radius(CornerRadius::same(12)),
    )
}

fn render_note_card(
    ui: &mut Ui,
    note: &Note,
    is_selected: bool,
    p: &Palette,
    query: &str,
) -> Option<CardAction> {
    let mut action = None;
    let bg = if is_selected {
        p.bg_selected
    } else {
        Color32::TRANSPARENT
    };
    let stroke = if is_selected {
        Stroke::new(1.0_f32, p.accent)
    } else {
        Stroke::new(1.0_f32, Color32::TRANSPARENT)
    };

    let frame_resp = Frame::new()
        .fill(bg)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(7))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                if note.pinned {
                    ui.label(RichText::new(icon::PINNED).size(13.0).color(p.pin_color));
                } else {
                    ui.label(RichText::new(icon::FILE_TEXT).size(13.0).color(p.fg_muted));
                }
                ui.label(
                    RichText::new(note.display_title())
                        .strong()
                        .size(14.0)
                        .color(p.fg_primary),
                );
            });

            ui.add_space(2.0);
            let excerpt = note.body_snippet(query).unwrap_or_else(|| note.snippet());
            ui.label(RichText::new(excerpt).size(12.0).color(p.fg_secondary));

            let tags = note.all_tags();
            let (done_tasks, total_tasks) = note.task_progress();
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(6.0, 2.0);
                ui.label(
                    RichText::new(note.formatted_updated())
                        .size(11.0)
                        .color(p.fg_muted),
                );
                if total_tasks > 0 {
                    ui.label(
                        RichText::new(format!("{} {done_tasks}/{total_tasks}", icon::LIST_CHECK))
                            .size(11.0)
                            .color(p.fg_muted),
                    );
                }
                for tag in tags.iter().take(3) {
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(format!("#{tag}")).size(11.0).color(p.accent),
                            )
                            .frame(false),
                        )
                        .clicked()
                    {
                        action = Some(CardAction::FilterTag(tag.clone()));
                    }
                }
            });
        });

    let card_click = ui.interact(
        frame_resp.response.rect,
        Id::new(("note_card", &note.id)),
        Sense::click(),
    );
    if card_click.clicked() && action.is_none() {
        action = Some(CardAction::SelectNote(note.id.clone()));
    }

    action
}

fn render_backlinks_footer(
    ui: &mut Ui,
    backlinks: &[(String, String)],
    p: &Palette,
    clicked_backlink_id: &mut Option<String>,
) {
    if backlinks.is_empty() {
        return;
    }
    ui.add_space(16.0);
    ui.separator();
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(format!("{} Linked from:", icon::LINK))
                .size(12.0)
                .color(p.fg_muted),
        );
        for (id, title) in backlinks {
            if ui
                .add(
                    egui::Button::new(
                        RichText::new(format!("{} {title}", icon::FILE_TEXT))
                            .size(12.0)
                            .color(p.accent),
                    )
                    .fill(p.bg_elevated)
                    .corner_radius(CornerRadius::same(6)),
                )
                .on_hover_text("Jump to referencing note")
                .clicked()
            {
                *clicked_backlink_id = Some(id.clone());
            }
        }
    });
}

/// Interactive Markdown Preview renderer with full multi-word inline formatting,
/// glowing highlights, clickable checkboxes, and `[[Wiki Links]]`.
fn render_markdown_preview(
    ui: &mut Ui,
    markdown: &str,
    p: &Palette,
    is_dark: bool,
    clicked_wiki_link: &mut Option<String>,
    toggled_task_idx: &mut Option<usize>,
) {
    let mut in_code_block = false;
    let mut code_buffer = String::new();
    let mut task_counter = 0usize;

    for line in markdown.lines() {
        let trimmed = line.trim_end();
        if trimmed.trim_start().starts_with("```") {
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
            continue;
        }

        if trimmed.is_empty() {
            ui.add_space(6.0);
            continue;
        }

        let trimmed_start = trimmed.trim_start();

        if trimmed_start == "---" || trimmed_start == "***" {
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(4.0);
        } else if let Some(h1) = trimmed_start.strip_prefix("# ") {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                render_inline_spans_ui(ui, h1, 21.5, p, is_dark, p.fg_primary, clicked_wiki_link);
            });
            ui.add_space(2.0);
        } else if let Some(h2) = trimmed_start.strip_prefix("## ") {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                render_inline_spans_ui(ui, h2, 18.0, p, is_dark, p.fg_primary, clicked_wiki_link);
            });
            ui.add_space(2.0);
        } else if let Some(h3) = trimmed_start.strip_prefix("### ") {
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                render_inline_spans_ui(ui, h3, 16.0, p, is_dark, p.fg_secondary, clicked_wiki_link);
            });
        } else if let Some(quote) = trimmed_start.strip_prefix("> ") {
            Frame::new()
                .fill(p.bg_sidebar)
                .stroke(Stroke::new(1.5_f32, p.accent))
                .corner_radius(CornerRadius::same(4))
                .inner_margin(Margin::symmetric(10, 6))
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        render_inline_spans_ui(
                            ui,
                            quote,
                            14.5,
                            p,
                            is_dark,
                            p.fg_secondary,
                            clicked_wiki_link,
                        );
                    });
                });
        } else if let Some(done) = trimmed_start
            .strip_prefix("- [x] ")
            .or_else(|| trimmed_start.strip_prefix("- [X] "))
        {
            let this_idx = task_counter;
            task_counter += 1;
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
                    *toggled_task_idx = Some(this_idx);
                }
                ui.add_space(4.0);
                ui.label(
                    RichText::new(done)
                        .strikethrough()
                        .color(p.fg_muted)
                        .size(14.5),
                );
            });
        } else if let Some(todo) = trimmed_start.strip_prefix("- [ ] ") {
            let this_idx = task_counter;
            task_counter += 1;
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
                    *toggled_task_idx = Some(this_idx);
                }
                ui.add_space(4.0);
                render_inline_spans_ui(ui, todo, 14.5, p, is_dark, p.fg_primary, clicked_wiki_link);
            });
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
                    is_dark,
                    p.fg_primary,
                    clicked_wiki_link,
                );
            });
        } else {
            ui.horizontal_wrapped(|ui| {
                render_inline_spans_ui(
                    ui,
                    trimmed_start,
                    14.5,
                    p,
                    is_dark,
                    p.fg_primary,
                    clicked_wiki_link,
                );
            });
        }
    }
}
