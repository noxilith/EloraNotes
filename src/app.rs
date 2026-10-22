use crate::icon_names::TABLER_ICON_NAMES;
use crate::icons::{icon, install_tabler_icons, tabler_glyph};
use crate::markdown::{
    apply_highlight_selection, build_editor_layout_job, clear_formatting_selection,
    color_swatch_button, insert_at_selection, markdown_to_html, render_inline_spans_ui, rgb_to_hex,
    wrap_selection, HIGHLIGHT_PRESETS,
};
use crate::storage::{
    extract_wiki_links, fuzzy_subsequence, normalize_folder, normalize_tag, sort_notes,
    AppSettings, Note, NoteStorage, NoteTemplate,
};
use crate::theme::{ColorOverrides, Palette, ThemeMode};
use crate::update::{self, Release, UpdateOffer};
use chrono::{Local, Utc};
use egui::{
    Align, Align2, Color32, CornerRadius, FontFamily, FontId, Frame, Id, Key, Layout, Margin, Pos2,
    RichText, ScrollArea, Sense, Stroke, TextEdit, TextStyle, Ui, Vec2,
};
use std::collections::{BTreeMap, BTreeSet};
use std::f32::consts::TAU;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// The sidebar keeps this width instead of being drag-resizable, so the note list
/// and the editor stop shifting around while the window is resized. It only has to fit
/// the logo, title and New button now that the other actions live in the footer.
const SIDEBAR_WIDTH: f32 = 210.0;

/// Height kept free at the bottom of the sidebar for the footer action row.
const SIDEBAR_FOOTER_HEIGHT: f32 = 34.0;

/// Smallest gap between the window edge (or the sidebar) and the note column. The column is
/// centred in wider windows and keeps at least this much air on both sides when it is not,
/// so resizing never pushes the text against one edge.
const NOTE_SIDE_MARGIN: f32 = 24.0;

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
    new_tag_input: String,
    /// Folder names folded shut in the sidebar.
    collapsed_folders: BTreeSet<String>,
    /// Text buffer for the "new folder" field in the sidebar folder controls.
    new_folder_input: String,
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
    /// Message currently shown in the transient toast, used to detect when a new one arrives.
    status_shown: Option<String>,
    /// Context time (seconds) at which the toast disappears.
    status_until: f64,
    /// Result channel for the update check and download running on worker threads.
    update_rx: Option<Receiver<UpdateEvent>>,
    update_state: UpdateState,
}

/// Messages from the update worker threads to the UI.
enum UpdateEvent {
    Checked(Result<Option<UpdateOffer>, String>),
    Staged(Result<PathBuf, String>),
}

/// What the update prompt is currently showing.
#[derive(Clone)]
enum UpdateState {
    Idle,
    Checking,
    Available(UpdateOffer),
    Downloading(Release),
    /// A newer release exists but a package manager owns this copy, so there is nothing to
    /// download; the prompt only explains how to get it.
    Managed {
        version: String,
        manager: &'static str,
    },
    Error(String),
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

        let notes = storage.load_all().unwrap_or_default();
        let selected_id = notes.iter().find(|n| !n.trashed).map(|n| n.id.clone());

        let mut app = Self {
            storage,
            notes,
            selected_id,
            search_query: String::new(),
            active_tag: None,
            sidebar_section: SidebarSection::AllNotes,
            new_tag_input: String::new(),
            collapsed_folders: settings.collapsed_folders.iter().cloned().collect(),
            new_folder_input: String::new(),
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
            status_shown: None,
            status_until: 0.0,
            update_rx: None,
            update_state: UpdateState::Idle,
        };
        app.start_update_check(&cc.egui_ctx);
        app
    }

    fn persist_settings(&self) {
        let settings = AppSettings {
            theme_name: self.theme.name().to_string(),
            show_formatting_bar: self.show_formatting_bar,
            highlight_rgb: self.highlight_rgb,
            text_color_rgb: self.text_color_rgb,
            font_size: self.font_size,
            glow_strength: self.glow_strength,
            colors: self.colors,
            collapsed_folders: self.collapsed_folders.iter().cloned().collect(),
        };
        self.storage.save_settings(&settings);
    }

    fn apply_look(&self, ctx: &egui::Context) {
        self.theme
            .apply(ctx, &self.colors, self.font_size, self.glow_strength);
    }

    /// Short-lived message in the lower-left corner, replacing the old permanent status bar.
    /// Import/export results and auto-save errors still need somewhere to appear.
    fn status_toast(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(message) = self.status_message.clone() else {
            return;
        };
        let now = ctx.input(|i| i.time);
        if self.status_shown.as_deref() != Some(message.as_str()) {
            self.status_shown = Some(message.clone());
            self.status_until = now + 4.0;
        }
        let remaining = self.status_until - now;
        if remaining <= 0.0 {
            self.status_message = None;
            self.status_shown = None;
            return;
        }

        let lower = message.to_lowercase();
        let is_problem =
            lower.contains("error") || lower.contains("failed") || lower.contains("fail");
        let toast_left = if self.focus_mode {
            16.0
        } else {
            SIDEBAR_WIDTH + 16.0
        };
        egui::Area::new(egui::Id::new("elora_status_toast"))
            .anchor(egui::Align2::LEFT_BOTTOM, Vec2::new(toast_left, -16.0))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                Frame::new()
                    .fill(p.bg_elevated)
                    .stroke(Stroke::new(1.0_f32, p.border))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        let glyph = if is_problem { icon::X } else { icon::CHECK };
                        ui.label(
                            RichText::new(format!("{glyph} {message}"))
                                .size(11.5)
                                .color(if is_problem { p.danger } else { p.fg_secondary }),
                        );
                    });
            });
        ctx.request_repaint_after(Duration::from_secs_f32(remaining as f32));
    }

    /// Kicks off the launch-time version check on a worker thread so the first frame does not
    /// wait for GitHub.
    fn start_update_check(&mut self, ctx: &egui::Context) {
        update::cleanup_stale_updates();
        let (tx, rx) = mpsc::channel();
        self.update_rx = Some(rx);
        self.update_state = UpdateState::Checking;
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = update::check_for_update();
            let _ = tx.send(UpdateEvent::Checked(result));
            ctx.request_repaint();
        });
    }

    /// Downloads the release asset in the background; the binary is swapped in on the UI
    /// thread once the file is ready.
    fn start_update_download(&mut self, ctx: &egui::Context, release: Release) {
        let (tx, rx) = mpsc::channel();
        self.update_rx = Some(rx);
        self.update_state = UpdateState::Downloading(release.clone());
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = update::stage_download(&release);
            let _ = tx.send(UpdateEvent::Staged(result));
            ctx.request_repaint();
        });
    }

    fn poll_update(&mut self) {
        let was_downloading = matches!(self.update_state, UpdateState::Downloading(_));
        let mut events = Vec::new();
        if let Some(rx) = &self.update_rx {
            loop {
                match rx.try_recv() {
                    Ok(event) => events.push(event),
                    Err(TryRecvError::Empty) => break,
                    // Each worker sends one message and then hangs up, so a disconnect is
                    // only a failure when nothing at all arrived.
                    Err(TryRecvError::Disconnected) => {
                        if events.is_empty() {
                            events.push(if was_downloading {
                                UpdateEvent::Staged(Err(
                                    "The update download stopped unexpectedly.".to_string(),
                                ))
                            } else {
                                UpdateEvent::Checked(Err(
                                    "The update check stopped unexpectedly.".to_string()
                                ))
                            });
                        }
                        break;
                    }
                }
            }
        }

        for event in events {
            match event {
                UpdateEvent::Checked(Ok(Some(offer))) => {
                    self.update_rx = None;
                    self.update_state = UpdateState::Available(offer);
                }
                // Offline, rate-limited, already current, or no binary for this platform:
                // there is nothing worth interrupting the user for.
                UpdateEvent::Checked(_) => {
                    self.update_rx = None;
                    self.update_state = UpdateState::Idle;
                }
                UpdateEvent::Staged(Ok(staged)) => {
                    self.update_rx = None;
                    match update::install_and_restart(&staged) {
                        Ok(()) => std::process::exit(0),
                        Err(error) => self.update_state = UpdateState::Error(error),
                    }
                }
                UpdateEvent::Staged(Err(error)) => {
                    self.update_rx = None;
                    self.update_state = UpdateState::Error(error);
                }
            }
        }
    }

    /// The launch-time prompt: update now (download, swap and restart) or dismiss it for this
    /// run. Nothing is offered when the check failed or the running build is current.
    fn update_window(&mut self, ctx: &egui::Context, p: &Palette) {
        let state = self.update_state.clone();
        match state {
            UpdateState::Idle | UpdateState::Checking => {}
            UpdateState::Available(offer) => match offer {
                UpdateOffer::SelfUpdate(release) => {
                    let mut choice = None;
                    egui::Window::new("Update available")
                        .id(Id::new("elora_update_window"))
                        .collapsible(false)
                        .resizable(false)
                        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                        .show(ctx, |ui| {
                            ui.set_min_width(320.0);
                            ui.label(
                                RichText::new(format!(
                                    "EloraNotes {} is available",
                                    release.version
                                ))
                                .size(14.0)
                                .strong(),
                            );
                            ui.label(
                                RichText::new(format!(
                                    "You are running {}. Update and restart now?",
                                    update::CURRENT_VERSION
                                ))
                                .size(12.0)
                                .color(p.fg_secondary),
                            );
                            ui.add_space(10.0);
                            ui.horizontal(|ui| {
                                if ui
                                    .add(
                                        egui::Button::new(RichText::new("Update now").size(12.5))
                                            .fill(p.accent),
                                    )
                                    .clicked()
                                {
                                    choice = Some(true);
                                }
                                if ui.button("Later").clicked() {
                                    choice = Some(false);
                                }
                            });
                        });
                    match choice {
                        Some(true) => self.start_update_download(ctx, release),
                        Some(false) => self.update_state = UpdateState::Idle,
                        None => {}
                    }
                }
                // Flatpak owns its own files: offering to replace them would fail after the
                // download, so the prompt just points at the command that can.
                UpdateOffer::Managed { version, manager } => {
                    self.update_state = UpdateState::Managed { version, manager };
                }
            },
            UpdateState::Managed { version, manager } => {
                let mut dismiss = false;
                egui::Window::new("Update available")
                    .id(Id::new("elora_update_window"))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                    .show(ctx, |ui| {
                        ui.set_min_width(320.0);
                        ui.label(
                            RichText::new(format!("EloraNotes {version} is available"))
                                .size(14.0)
                                .strong(),
                        );
                        ui.label(
                            RichText::new(format!(
                                "You are running {}.",
                                update::CURRENT_VERSION
                            ))
                            .size(12.0)
                            .color(p.fg_secondary),
                        );
                        ui.label(
                            RichText::new(format!(
                                "This copy was installed with {manager}, which owns its own files. Install the new bundle over it, in the folder where you saved it:"
                            ))
                            .size(11.5)
                            .color(p.fg_muted),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new(format!(
                                "flatpak install --reinstall ./EloraNotes-{version}-linux-x86_64.flatpak"
                            ))
                            .size(11.5)
                            .monospace(),
                        );
                        ui.add_space(8.0);
                        if ui.button("Close").clicked() {
                            dismiss = true;
                        }
                    });
                if dismiss {
                    self.update_state = UpdateState::Idle;
                }
            }
            UpdateState::Downloading(release) => {
                egui::Window::new("Updating EloraNotes")
                    .id(Id::new("elora_update_window"))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                    .show(ctx, |ui| {
                        ui.set_min_width(320.0);
                        ui.horizontal(|ui| {
                            ui.add(egui::Spinner::new().size(16.0));
                            ui.label(format!("Downloading EloraNotes {}…", release.version));
                        });
                        ui.label(
                            RichText::new("EloraNotes restarts itself when the download finishes.")
                                .size(11.5)
                                .color(p.fg_muted),
                        );
                    });
            }
            UpdateState::Error(message) => {
                let mut dismiss = false;
                egui::Window::new("Update failed")
                    .id(Id::new("elora_update_window"))
                    .collapsible(false)
                    .resizable(false)
                    .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
                    .show(ctx, |ui| {
                        ui.set_min_width(320.0);
                        ui.label(RichText::new(message).size(12.0).color(p.danger));
                        ui.add_space(8.0);
                        if ui.button("Close").clicked() {
                            dismiss = true;
                        }
                    });
                if dismiss {
                    self.update_state = UpdateState::Idle;
                }
            }
        }
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
            .default_width(380.0)
            .show(ctx, |ui| {
                // The window now carries the notes folder and shortcut reference too, so
                // it must scroll rather than run off the bottom of a short window.
                let max_height = (ctx.screen_rect().height() - 160.0).max(240.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height(max_height)
                    .show(ui, |ui| {
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
                            .add(
                                egui::Slider::new(&mut self.font_size, 11.0..=22.0)
                                    .text("Text size"),
                            )
                            .changed();
                        ui.separator();

                        ui.label(RichText::new("Colors").strong());
                        ui.label("Tick a color to override the theme's default.");
                        changed |= color_override_row(
                            ui,
                            "Background",
                            &mut self.colors.bg,
                            defaults.bg_canvas,
                        );
                        changed |= color_override_row(
                            ui,
                            "Sidebar",
                            &mut self.colors.sidebar,
                            defaults.bg_sidebar,
                        );
                        changed |= color_override_row(
                            ui,
                            "Text",
                            &mut self.colors.fg,
                            defaults.fg_primary,
                        );
                        changed |= color_override_row(
                            ui,
                            "Accent",
                            &mut self.colors.accent,
                            defaults.accent,
                        );
                        changed |=
                            color_override_row(ui, "Pin", &mut self.colors.pin, defaults.pin_color);
                        changed |= color_override_row(
                            ui,
                            "Muted text",
                            &mut self.colors.fg_muted,
                            defaults.fg_muted,
                        );
                        changed |= color_override_row(
                            ui,
                            "Borders",
                            &mut self.colors.border,
                            defaults.border,
                        );
                        changed |= color_override_row(
                            ui,
                            "Danger",
                            &mut self.colors.danger,
                            defaults.danger,
                        );
                        if ui.button("Reset colors to theme").clicked() {
                            self.colors = Default::default();
                            changed = true;
                        }
                        ui.separator();

                        ui.label(RichText::new("Glow highlights").strong());
                        changed |= ui
                            .add(
                                egui::Slider::new(&mut self.glow_strength, 0.0..=1.0)
                                    .text("Glow strength"),
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

                        ui.label(RichText::new("Notes folder").strong());
                        let notes_dir = self.storage.root_dir().display().to_string();
                        ui.add(
                            egui::Label::new(
                                RichText::new(notes_dir.clone())
                                    .monospace()
                                    .size(11.0)
                                    .color(defaults.fg_muted),
                            )
                            .wrap(),
                        );
                        if ui
                            .small_button(format!("{}  Copy path", icon::COPY))
                            .clicked()
                        {
                            ui.ctx().copy_text(notes_dir);
                            self.status_message = Some("Copied notes folder path".to_string());
                        }

                        ui.separator();

                        ui.label(RichText::new("Shortcuts").strong());
                        egui::Grid::new("elora_settings_shortcuts")
                            .num_columns(2)
                            .spacing([14.0, 4.0])
                            .show(ui, |ui| {
                                for (keys, action) in [
                                    ("Right-click selection", "Format / glow highlight"),
                                    ("Ctrl+B", "Bold"),
                                    ("Ctrl+Shift+H", "Glow highlight"),
                                    ("Ctrl+K", "Command palette & quick switcher"),
                                ] {
                                    ui.label(
                                        RichText::new(keys)
                                            .monospace()
                                            .size(11.0)
                                            .color(defaults.fg_secondary),
                                    );
                                    ui.label(
                                        RichText::new(action).size(11.5).color(defaults.fg_muted),
                                    );
                                    ui.end_row();
                                }
                            });
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
        sort_notes(&mut self.notes);
    }

    /// Every folder currently used by a live note, alphabetically.
    fn all_folders(&self) -> Vec<String> {
        let mut folders: BTreeSet<String> = BTreeSet::new();
        for note in self.notes.iter().filter(|n| !n.trashed) {
            if let Some(folder) = &note.folder {
                folders.insert(folder.clone());
            }
        }
        folders.into_iter().collect()
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
                self.notes = self.storage.load_all().unwrap_or_default();
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
        // A new note joins the folder of the note that was open.
        if note.folder.is_none() {
            note.folder = self
                .selected_id
                .as_ref()
                .and_then(|id| self.notes.iter().find(|n| &n.id == id))
                .and_then(|n| n.folder.clone());
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
                    let resp = ui
                        .add(
                            TextEdit::singleline(&mut self.search_query)
                                .id(search_id)
                                .hint_text("Search notes…")
                                .frame(false)
                                .desired_width(f32::INFINITY),
                        )
                        .on_hover_text("Search notes or #tags (Ctrl+F)");
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

        // One row that never wraps: Notes and Pinned keep their counts at the smaller pill
        // size, and the glyph-only trash button sits to the right of Pinned instead of
        // dropping onto a second line.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
            ui.spacing_mut().button_padding = Vec2::new(6.0, 3.0);
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
                && nav_icon_pill(
                    ui,
                    icon::TRASH,
                    self.sidebar_section == SidebarSection::Trash,
                    p,
                )
                .on_hover_text(format!("Trash ({trash_count})"))
                .clicked()
            {
                self.sidebar_section = SidebarSection::Trash;
                self.select_first_visible();
            }
        });

        let all_tags = self.all_tags_with_counts();
        if !all_tags.is_empty() && self.sidebar_section != SidebarSection::Trash {
            ui.add_space(4.0);
            ui.scope(|ui| {
                ui.set_max_width((ui.available_width() - 6.0).max(80.0));
                tag_filter_menu(ui, p, &all_tags, &mut self.active_tag);
            });
        }

        ui.add_space(6.0);
        ui.separator();
        ui.add_space(4.0);

        // Reserve room for the footer so the note list never scrolls under it.
        let list_height = (ui.available_height() - SIDEBAR_FOOTER_HEIGHT).max(80.0);
        self.render_sidebar_notes(ui, p, list_height);

        ui.add_space(2.0);
        ui.separator();
        ui.add_space(3.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
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
            });
        });
        ui.add_space(2.0);
    }

    /// The scrollable note list: pinned notes, folder groups, unfiled notes, then trash.
    fn render_sidebar_notes(&mut self, ui: &mut Ui, p: &Palette, max_height: f32) {
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

        // Unpinned live notes are grouped by folder; trashed notes are listed flat.
        let mut folder_groups: Vec<(String, Vec<usize>)> = Vec::new();
        let mut unfiled: Vec<usize> = Vec::new();
        let mut trashed_indices: Vec<usize> = Vec::new();
        for &i in &filtered {
            let note = &self.notes[i];
            if note.trashed {
                trashed_indices.push(i);
            } else if note.pinned {
                // Already listed in the pinned section above.
            } else if let Some(folder) = &note.folder {
                match folder_groups.iter_mut().find(|(name, _)| name == folder) {
                    Some((_, list)) => list.push(i),
                    None => folder_groups.push((folder.clone(), vec![i])),
                }
            } else {
                unfiled.push(i);
            }
        }
        folder_groups.sort_by_key(|(name, _)| name.to_lowercase());

        // Filtering a folder should reveal its matches instead of hiding them.
        let folders_open = !self.search_query.trim().is_empty() || self.active_tag.is_some();
        let has_folders = !folder_groups.is_empty();

        let mut picked = CardSelection::default();
        let mut toggled_folder: Option<String> = None;

        ScrollArea::vertical()
            .id_salt("notes_sidebar_scroll")
            .max_height(max_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if !pinned_indices.is_empty() {
                    section_heading(ui, icon::PINNED, "PINNED", p.pin_color);
                    render_note_rows(
                        ui,
                        &self.notes,
                        &pinned_indices,
                        self.selected_id.as_deref(),
                        p,
                        &self.search_query,
                        &mut picked,
                    );
                    ui.add_space(4.0);
                }

                for (name, indices) in &folder_groups {
                    let collapsed = !folders_open && self.collapsed_folders.contains(name);
                    if folder_header(ui, name, indices.len(), collapsed, p).clicked() {
                        toggled_folder = Some(name.clone());
                    }
                    if !collapsed {
                        ui.add_space(2.0);
                        render_note_rows(
                            ui,
                            &self.notes,
                            indices,
                            self.selected_id.as_deref(),
                            p,
                            &self.search_query,
                            &mut picked,
                        );
                        ui.add_space(3.0);
                    }
                }

                if !unfiled.is_empty() {
                    if has_folders || !pinned_indices.is_empty() {
                        section_heading(ui, icon::NOTES, "ALL NOTES", p.fg_muted);
                    }
                    render_note_rows(
                        ui,
                        &self.notes,
                        &unfiled,
                        self.selected_id.as_deref(),
                        p,
                        &self.search_query,
                        &mut picked,
                    );
                }

                if !trashed_indices.is_empty() {
                    section_heading(ui, icon::TRASH, "TRASH", p.fg_muted);
                    render_note_rows(
                        ui,
                        &self.notes,
                        &trashed_indices,
                        self.selected_id.as_deref(),
                        p,
                        &self.search_query,
                        &mut picked,
                    );
                }
            });

        if let Some(folder) = toggled_folder {
            if !self.collapsed_folders.remove(&folder) {
                self.collapsed_folders.insert(folder);
            }
            self.persist_settings();
        }

        if let Some(id) = picked.note_id.take() {
            self.selected_id = Some(id);
            self.confirm_delete_id = None;
        }
        if let Some(tag) = picked.tag.take() {
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
        let mut folder_choice: Option<Option<String>> = None;

        let is_trashed = self.notes[note_idx].trashed;

        // Top Minimal Action Bar with Tabler Icons
        ui.horizontal(|ui| {
            // A narrow window cannot hold the labels, the folder picker and every action on
            // one line, so below this width the buttons drop their text and the stats hide
            // instead of drawing over each other.
            let compact = ui.available_width() < 680.0;
            let (focus_icon, focus_label) = if self.focus_mode {
                (icon::SIDEBAR_EXPAND, "Sidebar")
            } else {
                (icon::SIDEBAR_COLLAPSE, "Focus")
            };
            let focus_text = if compact {
                focus_icon.to_string()
            } else {
                format!("{focus_icon} {focus_label}")
            };
            if ui
                .add(
                    egui::Button::new(RichText::new(focus_text).size(12.5).color(p.fg_secondary))
                        .frame(false),
                )
                .on_hover_text("Toggle distraction-free focus mode (Ctrl+\\)")
                .clicked()
            {
                self.focus_mode = !self.focus_mode;
            }

            if !compact {
                ui.label(RichText::new("·").color(p.fg_muted));
            }

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

            if !compact {
                ui.label(RichText::new("·").color(p.fg_muted));
            }

            // Folder chooser: file this note under a folder, or start a new one.
            let current_folder = self.notes[note_idx].folder.clone();
            let folders = self.all_folders();
            if let Some(choice) = folder_menu_ui(
                ui,
                p,
                current_folder.as_deref(),
                &folders,
                &mut self.new_folder_input,
                if compact { 104.0 } else { 148.0 },
            ) {
                folder_choice = Some(choice);
            }

            if !compact {
                ui.label(RichText::new("·").color(p.fg_muted));
            }

            // The word count has no room to spare in a narrow window.
            if !compact {
                let updated_text = self.notes[note_idx].formatted_updated();
                let words = self.notes[note_idx].word_count();
                let chars = self.notes[note_idx].char_count();
                let read_mins = self.notes[note_idx].reading_time_minutes();
                let (done_tasks, total_tasks) = self.notes[note_idx].task_progress();

                let mut extras = String::new();
                if read_mins > 0 {
                    extras.push_str(&format!(" (~{read_mins}m read)"));
                }
                if total_tasks > 0 {
                    extras.push_str(&format!(
                        "   ·   {} {done_tasks}/{total_tasks}",
                        icon::LIST_CHECK
                    ));
                }
                // Width the action buttons on the right take up, including the gap before them.
                // The stats must stay clear of it: a right-to-left group would otherwise draw
                // over the end of the text in a narrow window.
                let reserved = if is_trashed { 440.0 } else { 330.0 };
                let budget = (ui.available_width() - reserved).max(0.0);
                let measure = |text: &str| {
                    ui.fonts(|fonts| {
                        fonts
                            .layout_no_wrap(
                                text.to_owned(),
                                egui::FontId::proportional(12.0),
                                p.fg_muted,
                            )
                            .size()
                            .x
                    })
                };
                // Shorter and shorter versions of the same line, tried in order. Truncating
                // the full one left it ending in "(" the moment the window got a little
                // narrower, which read like it was running under the buttons.
                let candidates = [
                    format!(
                        "{} {}   ·   {} words ({} chars){extras}",
                        icon::CLOCK,
                        updated_text,
                        words,
                        chars
                    ),
                    format!("{words} words ({chars} chars){extras}"),
                    format!("{words} words"),
                ];
                if budget >= 40.0 {
                    if let Some(stats) = candidates
                        .iter()
                        .find(|text| measure(text) <= budget)
                        .or_else(|| candidates.last())
                    {
                        ui.scope(|ui| {
                            ui.set_max_width(budget);
                            ui.add(
                                egui::Label::new(RichText::new(stats).size(12.0).color(p.fg_muted))
                                    .truncate(),
                            );
                        });
                    }
                }
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if is_trashed {
                    let delete_text = if compact {
                        icon::TRASH_X.to_string()
                    } else {
                        format!("{} Delete Forever", icon::TRASH_X)
                    };
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(delete_text).size(12.0).color(Color32::WHITE),
                            )
                            .fill(p.danger),
                        )
                        .on_hover_text("Delete this note forever")
                        .clicked()
                    {
                        action_perm_delete = true;
                    }
                    let restore_text = if compact {
                        icon::RESTORE.to_string()
                    } else {
                        format!("{} Restore", icon::RESTORE)
                    };
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(restore_text).size(12.5).color(p.accent),
                            )
                            .frame(true),
                        )
                        .on_hover_text("Restore this note")
                        .clicked()
                    {
                        action_restore_trash = true;
                    }
                } else if self.confirm_delete_id.as_deref() == Some(selected_id.as_str()) {
                    let trash_text = if compact {
                        "Trash".to_string()
                    } else {
                        format!("{} Move to Trash", icon::TRASH)
                    };
                    if ui
                        .add(
                            egui::Button::new(
                                RichText::new(trash_text).size(12.0).color(Color32::WHITE),
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
                                RichText::new(if compact {
                                    glyph.to_string()
                                } else {
                                    format!("{glyph} {label}")
                                })
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
                                RichText::new(if compact {
                                    pin_glyph.to_string()
                                } else {
                                    format!("{pin_glyph} {pin_text}")
                                })
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

        if let Some(choice) = folder_choice {
            self.notes[note_idx].folder = choice.clone();
            self.notes[note_idx].updated_at = Utc::now();
            needs_save = true;
            match choice {
                Some(name) => {
                    // Filing a note into a folded folder should reveal it.
                    self.collapsed_folders.remove(&name);
                    self.status_message = Some(format!("Filed under {name}"));
                }
                None => self.status_message = Some("Removed from folder".to_string()),
            }
        }

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

                // The colour controls are the widest group in the bar, and a wrapped row cannot
                // move a group of widgets as a unit: at some widths the swatch next to a button
                // was left alone on a second line. Break the line before the group instead.
                let needed = color_controls_width(ui, p);
                // In a wrapping layout both `available_width` and `available_size_before_wrap`
                // report the whole row, so the space left on the line is measured from the
                // cursor instead.
                let row = ui.max_rect();
                let cursor = ui.cursor().min.x;
                let remaining = (row.right() - cursor).max(0.0);
                if needed > remaining && cursor > row.left() + 0.5 {
                    ui.add_space(remaining);
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

                // The preset glow swatches are not repeated here: on a laptop-width window
                // they wrapped onto a row of their own, so the bar now always fits its
                // controls on one line. The presets remain in the right-click menu.
            });
        }

        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);

        let backlinks = self.backlinks_for(note_idx);
        let mut clicked_wiki_link: Option<String> = None;
        let mut clicked_backlink_id: Option<String> = None;
        let mut toggled_task_idx: Option<usize> = None;

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
                            &mut clicked_wiki_link,
                            &mut toggled_task_idx,
                        );
                        render_backlinks_footer(ui, &backlinks, p, &mut clicked_backlink_id);
                    });
            }
            EditorViewMode::Split => {
                // Both panes are the same width and sit inside the same column as Edit and
                // Preview, so the outer margins do not change when the view is switched. The
                // gutter is the width of the window margin: `ui.columns` sized its gap from the
                // global item spacing (8 px), which left the two panes almost touching.
                let pane_height = ui.available_height();
                let pane_width = ((ui.available_width() - NOTE_SIDE_MARGIN) * 0.5).max(80.0);
                ui.horizontal_top(|ui| {
                    // The gutter below is the only space between the panes: the default item
                    // spacing would otherwise be added to it and push the right pane past the
                    // column's right edge.
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.allocate_ui_with_layout(
                        Vec2::new(pane_width, pane_height),
                        Layout::top_down(Align::Min),
                        |ui| {
                            // A pane owns its half of the column: nothing may be painted into
                            // the gutter or the window margin, even a chip too wide to fit.
                            ui.set_clip_rect(ui.max_rect());
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
                        },
                    );
                    ui.add_space(NOTE_SIDE_MARGIN);
                    ui.allocate_ui_with_layout(
                        Vec2::new(pane_width, pane_height),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.set_clip_rect(ui.max_rect());
                            ScrollArea::vertical()
                                .id_salt("elora_split_right")
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    let body_snapshot = self.notes[note_idx].body.clone();
                                    render_markdown_preview(
                                        ui,
                                        &body_snapshot,
                                        p,
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
                        },
                    );
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
    /// eframe clears the window with a translucent near-black by default, which shows up as a
    /// lighter strip wherever the panels have not painted yet. Clear with the theme's canvas
    /// colour instead so such places are indistinguishable from the note background.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.theme
            .palette_with(&self.colors, self.glow_strength)
            .bg_canvas
            .to_normalized_gamma_f32()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_shortcuts(ctx);
        self.poll_update();
        self.settings_window(ctx);
        self.graph_window(ctx);
        let p = self.theme.palette_with(&self.colors, self.glow_strength);

        if !self.focus_mode {
            let sidebar = egui::SidePanel::left("elora_sidebar")
                .resizable(false)
                .exact_width(SIDEBAR_WIDTH)
                // The divider is painted inside the panel below instead of as a frame stroke:
                // a stroke straddles the panel boundary, which at fractional display scales
                // leaves a canvas-coloured pixel between the fill and the line.
                .frame(
                    Frame::new()
                        .fill(p.bg_sidebar)
                        .inner_margin(Margin::symmetric(12, 10)),
                )
                .show(ctx, |ui| {
                    self.render_sidebar(ui, &p);
                });
            let rect = sidebar.response.rect;
            let ppp = ctx.pixels_per_point();
            let right = (rect.right() * ppp).round() / ppp;
            ctx.layer_painter(egui::LayerId::background()).rect_filled(
                egui::Rect::from_min_max(
                    Pos2::new(right - 1.0 / ppp, rect.top()),
                    Pos2::new(right, rect.bottom()),
                ),
                0.0,
                p.border,
            );
        }

        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(p.bg_canvas)
                    // The horizontal inset is applied to the centred column below, equally on
                    // both sides, instead of as a one-sided panel margin. A fixed right margin
                    // made the note sit off-centre at every window size.
                    .inner_margin(Margin {
                        left: 0,
                        right: 0,
                        top: 18,
                        bottom: 18,
                    }),
            )
            .show(ctx, |ui| {
                // Every view uses the same column width, so switching between Edit, Split
                // and Preview never shifts the text sideways. Focus mode keeps a slightly
                // narrower column because the sidebar is gone.
                let max_width = if self.focus_mode { 820.0 } else { 980.0 };
                let available = ui.available_width();
                // Centre the column when the window is wide, and keep an equal minimum inset
                // on both sides when it is not. Only the padding changes with the window size;
                // the column never clings to one edge.
                let pad = ((available - max_width) * 0.5).max(NOTE_SIDE_MARGIN);
                let width = (available - pad * 2.0).max(200.0);
                let height = ui.available_height();
                ui.horizontal_top(|ui| {
                    ui.add_space(pad);
                    ui.allocate_ui_with_layout(
                        Vec2::new(width, height),
                        Layout::top_down(Align::Min),
                        |ui| self.render_editor(ui, &p),
                    );
                });
            });

        self.render_command_palette(ctx, &p);
        self.update_window(ctx, &p);
        self.status_toast(ctx, &p);
    }
}

enum CardAction {
    SelectNote(String),
    FilterTag(String),
}

/// A section pill in the sidebar: glyph, label and count on one line. The pills are drawn at a
/// slightly smaller size than the rest of the sidebar so all three sections, including the
/// glyph-only trash button, fit across a 210 px sidebar without wrapping.
fn nav_pill(
    ui: &mut Ui,
    glyph: &str,
    label: &str,
    count: usize,
    active: bool,
    p: &Palette,
) -> egui::Response {
    let fg = if active { p.accent } else { p.fg_secondary };
    let bg = if active { p.accent_soft } else { p.bg_elevated };
    let border = if active { p.accent } else { p.border };

    pill_button(
        ui,
        RichText::new(format!("{glyph} {label} ({count})"))
            .size(11.0)
            .color(fg),
        bg,
        border,
    )
}

/// Trash is the quietest section, so it gets a glyph-only button that still fits beside the
/// two labelled pills.
fn nav_icon_pill(ui: &mut Ui, glyph: &str, active: bool, p: &Palette) -> egui::Response {
    let fg = if active { p.accent } else { p.fg_secondary };
    let bg = if active { p.accent_soft } else { p.bg_elevated };
    let border = if active { p.accent } else { p.border };

    pill_button(ui, RichText::new(glyph).size(11.5).color(fg), bg, border)
}

fn pill_button(ui: &mut Ui, text: RichText, bg: Color32, border: Color32) -> egui::Response {
    ui.add(
        egui::Button::new(text)
            .fill(bg)
            .stroke(Stroke::new(1.0_f32, border))
            .corner_radius(CornerRadius::same(8)),
    )
}

/// One dropdown that holds every tag filter. A wrapping row of tag chips grew with each new
/// tag, pushing the note list down and changing the sidebar's height as notes were edited.
/// This keeps the row at a single line no matter how many tags exist.
fn tag_filter_menu(
    ui: &mut Ui,
    p: &Palette,
    tags: &[(String, usize)],
    active_tag: &mut Option<String>,
) {
    let active = active_tag.is_some();
    let label = match active_tag.as_deref() {
        Some(tag) => format!("{} {}", icon::HASH, short_tag(tag, 16)),
        None => format!("{} Filter", icon::ADJUSTMENTS),
    };
    let fg = if active { p.accent } else { p.fg_secondary };

    egui::ComboBox::from_id_salt("sidebar_tag_filter")
        .selected_text(RichText::new(label).size(11.0).color(fg))
        .width(ui.available_width())
        // Keep the menu open while the list is scrolled; a click inside simply picks a tag.
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            let mut picked: Option<Option<String>> = None;
            if ui
                .selectable_label(!active, RichText::new("All notes").size(11.5))
                .clicked()
            {
                picked = Some(None);
            }
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .show(ui, |ui| {
                    for (tag, count) in tags {
                        let selected = active_tag.as_deref() == Some(tag.as_str());
                        let text = format!("{} {tag}  ({count})", icon::HASH);
                        if ui
                            .selectable_label(selected, RichText::new(text).size(11.5))
                            .clicked()
                        {
                            picked = Some(if selected { None } else { Some(tag.clone()) });
                        }
                    }
                });
            if let Some(choice) = picked {
                *active_tag = choice;
                ui.memory_mut(|mem| mem.close_popup());
            }
        });
}

/// A long tag is elided so the filter button always stays inside the sidebar width.
fn short_tag(tag: &str, max_chars: usize) -> String {
    if tag.chars().count() <= max_chars {
        return tag.to_string();
    }
    let mut elided: String = tag.chars().take(max_chars.saturating_sub(1)).collect();
    elided.push('…');
    elided
}

/// A small all-caps heading above a group of note cards.
fn section_heading(ui: &mut Ui, glyph: &str, label: &str, color: Color32) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(glyph).size(11.0).color(color));
        ui.label(RichText::new(label).size(10.5).strong().color(color));
    });
    ui.add_space(2.0);
}

/// A collapsible folder row: chevron, folder icon, name and note count.
fn folder_header(
    ui: &mut Ui,
    name: &str,
    count: usize,
    collapsed: bool,
    p: &Palette,
) -> egui::Response {
    let chevron = if collapsed {
        icon::CHEVRON_RIGHT
    } else {
        icon::CHEVRON_DOWN
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = Vec2::new(6.0, 2.0);
        let resp = ui
            .add(
                egui::Button::new(
                    RichText::new(format!("{chevron} {} {name}", icon::FOLDER))
                        .size(11.5)
                        .color(p.fg_secondary),
                )
                .frame(false),
            )
            .on_hover_text(if collapsed {
                "Expand folder"
            } else {
                "Collapse folder"
            });
        ui.label(
            RichText::new(count.to_string())
                .size(10.5)
                .color(p.fg_muted),
        );
        resp
    })
    .inner
}

/// What the user clicked in the note list this frame.
#[derive(Default)]
struct CardSelection {
    note_id: Option<String>,
    tag: Option<String>,
}

/// Renders the given note indices as cards, recording any click in `picked`.
fn render_note_rows(
    ui: &mut Ui,
    notes: &[Note],
    indices: &[usize],
    selected_id: Option<&str>,
    p: &Palette,
    query: &str,
    picked: &mut CardSelection,
) {
    for &idx in indices {
        let note = &notes[idx];
        let is_selected = selected_id == Some(note.id.as_str());
        if let Some(action) = render_note_card(ui, note, is_selected, p, query) {
            match action {
                CardAction::SelectNote(id) => picked.note_id = Some(id),
                CardAction::FilterTag(t) => picked.tag = Some(t),
            }
        }
        ui.add_space(3.0);
    }
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
        .inner_margin(Margin::symmetric(10, 6))
        .show(ui, |ui| {
            // Inside the frame `available_width` already excludes the frame's own margins,
            // so the card fills the list exactly instead of leaving a strip on its right.
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.horizontal(|ui| {
                // A note's own icon wins; pinned notes fall back to a pin, then a file glyph.
                let (glyph, color) = note
                    .icon
                    .as_deref()
                    .and_then(tabler_glyph)
                    .map(|g| (g, p.accent))
                    .unwrap_or_else(|| {
                        if note.pinned {
                            (icon::PINNED, p.pin_color)
                        } else {
                            (icon::FILE_TEXT, p.fg_muted)
                        }
                    });
                ui.label(RichText::new(glyph).size(13.0).color(color));
                ui.label(
                    RichText::new(note.display_title())
                        .strong()
                        .size(13.5)
                        .color(p.fg_primary),
                );
            });

            let excerpt = note.body_snippet(query).unwrap_or_else(|| note.snippet());
            ui.add(
                egui::Label::new(RichText::new(excerpt).size(11.5).color(p.fg_secondary))
                    .truncate(),
            );

            let tags = note.all_tags();
            let (done_tasks, total_tasks) = note.task_progress();
            ui.add_space(2.0);
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

/// Width the colour pickers and their two apply buttons need on one line.
///
/// The formatting bar is a wrapping row, and a wrapping row cannot move a group of widgets to
/// a new line on its own. Knowing this width lets the bar break before the group instead, so a
/// swatch is never left stranded on a line of its own.
fn color_controls_width(ui: &Ui, p: &Palette) -> f32 {
    let text_width = |text: &str, strong: bool| {
        let family = if strong {
            crate::icons::bold_family()
        } else {
            FontFamily::Proportional
        };
        ui.fonts(|fonts| {
            fonts
                .layout_no_wrap(text.to_owned(), FontId::new(12.0, family), p.fg_primary)
                .size()
                .x
        })
    };
    let button =
        |text: &str, strong: bool| text_width(text, strong) + 2.0 * ui.spacing().button_padding.x;
    // The two square colour buttons use the standard interaction height.
    let swatch = ui.spacing().interact_size.y;
    let text_button = button("A Text Color", false);
    let glow_button = button(&format!("{} Glow Highlight", icon::HIGHLIGHT), true);
    // Both swatches, both buttons, the two "·" separators and the gaps between them.
    swatch + text_button + glow_button + swatch + 40.0
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

/// Folder dropdown shown in the note header. Files the note under an existing folder,
/// removes it from its folder, or names a brand new one.
///
/// Returns `Some(None)` for "No folder", `Some(Some(name))` for a chosen or freshly
/// created folder, and `None` when nothing was picked.
///
/// The popup closes only on clicks *outside* it (egui's default closes on any click), because
/// otherwise clicking into the "New folder" field dismissed the menu before a name could be typed.
fn folder_menu_ui(
    ui: &mut Ui,
    p: &Palette,
    current_folder: Option<&str>,
    folders: &[String],
    new_folder_input: &mut String,
    width: f32,
) -> Option<Option<String>> {
    let mut choice: Option<Option<String>> = None;
    let folder_label = current_folder.unwrap_or("No folder");

    egui::ComboBox::from_id_salt("note_folder_choice")
        .selected_text(
            RichText::new(format!("{} {folder_label}", icon::FOLDER))
                .size(12.0)
                .color(if current_folder.is_some() {
                    p.accent
                } else {
                    p.fg_secondary
                }),
        )
        .width(width)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(current_folder.is_none(), "No folder")
                .clicked()
            {
                choice = Some(None);
                ui.memory_mut(|mem| mem.close_popup());
            }
            for folder in folders {
                let selected = current_folder == Some(folder.as_str());
                if ui
                    .selectable_label(selected, format!("{} {folder}", icon::FOLDER))
                    .clicked()
                {
                    choice = Some(Some(folder.clone()));
                    ui.memory_mut(|mem| mem.close_popup());
                }
            }
            ui.separator();
            let mut submit = false;
            ui.horizontal(|ui| {
                let field = ui.add(
                    TextEdit::singleline(new_folder_input)
                        .id(Id::new("new_folder_name_field"))
                        .hint_text("New folder…")
                        .desired_width(108.0),
                );
                if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    submit = true;
                }
                if ui.button("Add").clicked() {
                    submit = true;
                }
            });
            if submit {
                if let Some(name) = normalize_folder(new_folder_input) {
                    choice = Some(Some(name));
                    new_folder_input.clear();
                    ui.memory_mut(|mem| mem.close_popup());
                }
            }
        });

    choice
}

/// Interactive Markdown Preview renderer with full multi-word inline formatting,
/// glowing highlights, clickable checkboxes, and `[[Wiki Links]]`.
fn render_markdown_preview(
    ui: &mut Ui,
    markdown: &str,
    p: &Palette,
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
                render_inline_spans_ui(ui, h1, 21.5, p, p.fg_primary, clicked_wiki_link, true);
            });
            ui.add_space(2.0);
        } else if let Some(h2) = trimmed_start.strip_prefix("## ") {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                render_inline_spans_ui(ui, h2, 18.0, p, p.fg_primary, clicked_wiki_link, true);
            });
            ui.add_space(2.0);
        } else if let Some(h3) = trimmed_start.strip_prefix("### ") {
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                render_inline_spans_ui(ui, h3, 16.0, p, p.fg_secondary, clicked_wiki_link, true);
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
                            p.fg_secondary,
                            clicked_wiki_link,
                            false,
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
                render_inline_spans_ui(ui, todo, 14.5, p, p.fg_primary, clicked_wiki_link, false);
            });
        } else if let Some(bullet) = trimmed_start
            .strip_prefix("- ")
            .or_else(|| trimmed_start.strip_prefix("* "))
        {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("•").color(p.accent).size(15.0));
                ui.add_space(5.0);
                render_inline_spans_ui(ui, bullet, 14.5, p, p.fg_primary, clicked_wiki_link, false);
            });
        } else {
            ui.horizontal_wrapped(|ui| {
                render_inline_spans_ui(
                    ui,
                    trimmed_start,
                    14.5,
                    p,
                    p.fg_primary,
                    clicked_wiki_link,
                    false,
                );
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drives the header folder dropdown the way a person does: open it, click into the
    /// "New folder" field, type a name, press Enter.
    struct FolderMenuHarness {
        ctx: egui::Context,
        palette: Palette,
        folders: Vec<String>,
        input: String,
        choice: Option<Option<String>>,
        combo_id: Option<Id>,
    }

    impl FolderMenuHarness {
        fn new() -> Self {
            let ctx = egui::Context::default();
            install_tabler_icons(&ctx);
            Self {
                ctx,
                palette: ThemeMode::Obsidian.palette(),
                folders: Vec::new(),
                input: String::new(),
                choice: None,
                combo_id: None,
            }
        }

        fn frame(&mut self, events: Vec<egui::Event>) {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(640.0, 480.0),
                )),
                events,
                ..Default::default()
            };
            let p = self.palette;
            let folders = self.folders.clone();
            let input = &mut self.input;
            let choice = &mut self.choice;
            let combo_id = &mut self.combo_id;
            let _ = self.ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    *combo_id = Some(ui.make_persistent_id(Id::new("note_folder_choice")));
                    *choice = folder_menu_ui(ui, &p, None, &folders, input, 148.0);
                });
            });
        }

        fn combo_id(&self) -> Id {
            self.combo_id.expect("combo box should have been laid out")
        }

        fn click(&mut self, pos: egui::Pos2) {
            for pressed in [true, false] {
                self.frame(vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ]);
            }
        }
    }

    #[test]
    fn test_folder_menu_click_into_new_folder_field_keeps_menu_open() {
        let mut h = FolderMenuHarness::new();
        h.frame(vec![]);

        // Open the menu the way clicking the combo box does.
        let widget_id = h.combo_id();
        let button_rect = h.ctx.read_response(widget_id).expect("combo button").rect;
        h.click(button_rect.center());
        assert!(
            egui::ComboBox::is_open(&h.ctx, widget_id),
            "clicking the combo box should open the folder menu"
        );
        h.frame(vec![]);

        let field_rect = h
            .ctx
            .read_response(Id::new("new_folder_name_field"))
            .expect("new-folder field should be laid out while the menu is open")
            .rect;
        h.click(field_rect.center());

        assert!(
            egui::ComboBox::is_open(&h.ctx, widget_id),
            "clicking the new-folder field must not close the folder menu"
        );

        // Type a name and confirm it: the note is filed under the new folder.
        h.frame(vec![egui::Event::Text("Research".to_string())]);
        h.frame(vec![egui::Event::Key {
            key: Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }]);

        assert_eq!(h.choice, Some(Some("Research".to_string())));
        assert!(h.input.is_empty(), "the field should reset after use");
        assert!(
            !egui::ComboBox::is_open(&h.ctx, widget_id),
            "picking a folder should close the folder menu"
        );
    }
}
