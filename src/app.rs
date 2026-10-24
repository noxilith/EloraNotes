use crate::collab::{Collab, ConnectionState, Incoming, ServerProfile};
use crate::icon_names::TABLER_ICON_NAMES;
use crate::icons::{icon, install_tabler_icons, tabler_glyph};
use crate::images::NoteImages;
use crate::markdown::{
    apply_highlight_selection, build_editor_layout_job, clear_formatting_selection,
    color_swatch_button, insert_at_selection, markdown_to_html, render_inline_spans_ui, rgb_to_hex,
    set_image_width, split_images, wrap_selection, LinePiece, HIGHLIGHT_PRESETS,
};
use crate::net::{NoteMeta, PresenceUser};
use crate::people::MIN_IDENTIFIER;
use crate::storage::{
    extract_wiki_links, fuzzy_subsequence, normalize_folder, normalize_tag, sort_notes,
    AppSettings, Note, NoteSort, NoteStorage, NoteTemplate, WindowState,
};
use crate::theme::{ColorOverrides, Palette, ThemeMode};
use crate::update::{self, Release, UpdateOffer};
use chrono::{Local, Utc};
use egui::text::{CCursor, CCursorRange};
use egui::{
    Align, Align2, Color32, CornerRadius, FontFamily, FontId, Frame, Id, Key, Layout, Margin, Pos2,
    Rect, RichText, ScrollArea, Sense, Stroke, TextEdit, TextStyle, Ui, UiBuilder, Vec2,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::f32::consts::TAU;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// The sidebar keeps this width instead of being drag-resizable, so the note list
/// and the editor stop shifting around while the window is resized. It only has to fit
/// the logo, title and New button now that the other actions live in the footer.
const SIDEBAR_WIDTH: f32 = 210.0;
/// Where the footer's Ko-fi button sends people.
const KOFI_URL: &str = "https://ko-fi.com/noxilith";
/// The note title's text field. The sync code and the field itself agree on this id, so a copy
/// of the note arriving from a server can tell when its title is the one being typed.
const TITLE_FIELD_ID: &str = "elora_note_title_input";
/// How long the editor waits for a shared note's own text before trusting the copy it has. A
/// note that opens normally answers in a few milliseconds; this only covers one that never does.
const ROOM_BODY_WAIT: Duration = Duration::from_millis(800);
/// Widest a status notice gets, in points. A longer message wraps inside it instead of running
/// across the window in one line; in a narrow window it is held to the window instead.
const TOAST_MAX_WIDTH: f32 = 420.0;
/// The notice's frame keeps this much room for its padding and border, on both sides together.
const TOAST_PADDING: f32 = 24.0;
/// Inset of the sidebar's content, in points. Also used to place the divider at the panel edge.
const SIDEBAR_MARGIN_X: i8 = 12;
const SIDEBAR_MARGIN_Y: i8 = 10;

/// How far a note or folder sits inside the group that holds it, in points. Notes filed in a
/// folder are stepped in once, so a folder's contents read as its children; the shared
/// library steps its folders in, and their notes in once more.
const SIDEBAR_INDENT_STEP: i8 = 12;

/// Strip kept free on the right of the note list for its scroll bar, so the bar sits beside the
/// cards instead of painting over them. The bar is 9 points wide at its widest.
const SIDEBAR_SCROLL_STRIP: f32 = 10.0;

/// Space kept between the footer icons and the panel's right edge.
const FOOTER_ICON_INSET: f32 = 5.0;

/// Smallest gap between the window edge (or the sidebar) and the note column. The column is
/// centred in wider windows and keeps at least this much air on both sides when it is not,
/// so resizing never pushes the text against one edge. It matches the sidebar's own inner
/// margin closely enough that the divider sits between two even gaps instead of hugging one
/// side and leaving a wide empty band on the other.
const NOTE_SIDE_MARGIN: f32 = 16.0;

/// Space reserved for the colour controls in the formatting bar, measured at the current text
/// sizes with a few pixels of slack. They are drawn inside a child layout of this size so the
/// wrapping row treats the whole group as one item: it fits beside the markdown buttons when
/// there is room, and otherwise drops to the next line in one piece.
const COLOR_GROUP_WIDTH: f32 = 344.0;
const COLOR_GROUP_HEIGHT: f32 = 28.0;

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
    /// Whether notes in the sidebar are the tall cards — icon, title, date and tags — instead of
    /// the one-line rows. Off by default: the list then reads like a file list and fits twice as
    /// many notes in the same height.
    roomy_cards: bool,
    /// What the notes inside each sidebar group are ordered by.
    sort: NoteSort,
    /// Where the window is now, so the next launch opens in the same place and size.
    window_state: Option<WindowState>,
    /// Whether the find bar is open over the editor.
    find_open: bool,
    /// What is being looked for in the open note.
    find_query: String,
    /// Which match is the one being stepped through, counted from zero.
    find_current: usize,
    /// Set when the match changed, so the editor moves its caret there and scrolls it into view.
    find_pending_jump: bool,
    /// One frame's request to put the caret in the find field.
    focus_find_next_frame: bool,
    /// The report of wiki links that point at no note, and whether its window is open.
    broken_links_open: bool,
    broken_links: Vec<(String, Vec<String>)>,
    /// Whether note cards in the sidebar show a line of the note's text under the title.
    show_card_previews: bool,
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
    /// Caret position in the active note editor, in characters, for the presence cursor sent to
    /// the other people in a shared note.
    last_editor_caret: Option<usize>,
    /// True while the caret was on screen last frame, so a remote edit only scrolled into view
    /// when the words being worked on were already being looked at.
    caret_in_view: bool,
    /// Set by a remote edit that moved the caret: the editor pulls it back on screen this frame.
    reveal_caret: bool,
    /// The shared library: connection, server notes and who is editing what.
    collab: Collab,
    /// The server saved in Settings, and the copy the Settings fields edit.
    server: ServerProfile,
    server_form: ServerProfile,
    /// Ids of the notes that live on the server rather than on this disk.
    remote_ids: BTreeSet<String>,
    /// Bodies of the server notes as last received, so the list can be rebuilt without flicker.
    remote_bodies: BTreeMap<String, String>,
    /// The server note the connection currently mirrors.
    collab_open: Option<String>,
    /// A shared note whose own text has been asked for and not arrived yet. Until it does, the
    /// window is showing whatever it had for that note from earlier, and sending that would
    /// write a stale copy into the room.
    awaiting_room_body: Option<(String, Instant)>,
    /// A certificate the server presented that has not been pinned yet: `(fingerprint, address)`.
    trust_prompt: Option<(String, String)>,
    /// True while waiting for the server to answer a new-note request, so the title field can be
    /// focused once the note exists.
    pending_new_note_focus: bool,
    /// The local note being moved into the shared library. Once the server reports a note with
    /// that title, the body is shown straight away and the local file is removed, so the note
    /// exists in one place.
    pending_server_move: Option<PendingMove>,
    confirm_delete_id: Option<String>,
    command_palette_open: bool,
    /// The "who can see this note" panel, and the note it was opened for.
    share_picker_open: bool,
    share_picker_note_id: Option<String>,
    share_input: String,
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
    /// True while the check was started from Settings: the result belongs in that window
    /// rather than in a popup over the notes.
    quiet_update: bool,
    /// Pictures embedded in the preview, decoded once and kept as textures.
    note_images: NoteImages,
    /// Note and character index the caret belongs at after a picture was inserted, so the
    /// editor puts it after the new lines instead of leaving it where the picture was dropped in.
    caret_after_insert: Option<(String, usize)>,
    /// Pictures this computer has asked the library for, so a missing one is not requested on
    /// every frame. Cleared when a connection is made, so a reconnect tries again.
    requested_pictures: BTreeSet<String>,
    /// Pictures this computer sent to the library and has not heard about yet, so a refusal can
    /// be reported to the person who added the picture.
    shared_uploads: BTreeSet<String>,
    /// The name this client goes by: what others see next to its caret, and the word they add
    /// to a note to share it. It is public on purpose — the server sees it, and so does everyone
    /// else. Generated on first launch, and typed by the person when they want their own.
    display_name: String,
    /// The secret behind that name. Only the server ever reads it, to recognise this person
    /// again; no other client is told it, so a name cannot be taken by somebody who simply
    /// knows what it is. The same identifier on another computer is the same person.
    identifier: String,
    /// The colour this person chose to appear in on everybody else's screen. `None` lets the
    /// name pick one, as it does when nothing has been chosen.
    peer_color: Option<[u8; 3]>,
    /// The identifier the open connection was made with, so a pasted one can be noticed and
    /// acted on rather than waiting for the next reconnect.
    sent_identifier: String,
    /// A system file dialog running on its own thread, with the note its picture belongs to.
    picture_pick: Option<PicturePick>,
    /// Ko-fi's mark, painted in the theme's colours beside Settings.
    kofi: Option<KofiMark>,
}

/// A "choose a picture" dialog running on a thread of its own.
///
/// The desktop's dialog answers on the file portal's schedule, and the blocking call waits for
/// it. On the window's own thread that stops the event loop for as long as somebody browses
/// their pictures, which is long enough for the desktop to decide the app has hung and offer to
/// close it. Off the thread, the window keeps drawing and the desktop never asks.
struct PicturePick {
    /// The note the picture goes into; the person may have moved on by the time it arrives.
    note_id: String,
    result: Receiver<Option<PathBuf>>,
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
    /// A check started from Settings found nothing newer.
    UpToDate,
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
    BrokenLinks,
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
        let mut app = Self::with_context(&cc.egui_ctx, storage);
        app.start_update_check(&cc.egui_ctx, false);
        // A remote keystroke asks for a frame the moment it arrives; an idle window is never
        // asked to draw on its own, so without this the other person's words would only appear
        // the next time this window happened to repaint.
        let repaint = cc.egui_ctx.clone();
        app.collab
            .set_waker(std::sync::Arc::new(move || repaint.request_repaint()));
        // A configured shared library reconnects on its own at launch.
        if app.server.is_configured() {
            let profile = app.server.clone();
            let name = app.server_display_name();
            let identifier = app.identifier.clone();
            app.sent_identifier = identifier.clone();
            app.collab.set_color(app.peer_color);
            app.collab.connect(profile, name, identifier);
        }
        app
    }

    /// Builds the app against a context without the side effects a window has at startup: no
    /// update check and no automatic reconnect. Tests drive the same state machine with it.
    fn with_context(ctx: &egui::Context, storage: NoteStorage) -> Self {
        install_tabler_icons(ctx);

        let settings = storage.load_settings();
        let theme = ThemeMode::from_name(&settings.theme_name);
        theme.apply(
            ctx,
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
            roomy_cards: settings.roomy_cards,
            sort: settings.sort,
            window_state: settings.window,
            find_open: false,
            find_query: String::new(),
            find_current: 0,
            find_pending_jump: false,
            focus_find_next_frame: false,
            broken_links_open: false,
            broken_links: Vec::new(),
            show_card_previews: settings.show_card_previews,
            highlight_rgb: settings.highlight_rgb,
            colors: settings.colors,
            font_size: settings.font_size,
            glow_strength: settings.glow_strength,
            show_settings: false,
            graph: None,
            last_editor_selection: None,
            last_editor_caret: None,
            caret_in_view: false,
            reveal_caret: false,
            collab: Collab::new(),
            server: settings.server.clone(),
            server_form: settings.server.clone(),
            display_name: settings.display_name.clone(),
            identifier: settings.identifier.clone(),
            peer_color: settings.peer_color,
            sent_identifier: String::new(),
            remote_ids: BTreeSet::new(),
            remote_bodies: BTreeMap::new(),
            collab_open: None,
            awaiting_room_body: None,
            trust_prompt: None,
            pending_new_note_focus: false,
            pending_server_move: None,
            edit_history: BTreeMap::new(),
            last_seen_body: None,
            confirm_delete_id: None,
            command_palette_open: false,
            share_picker_open: false,
            share_picker_note_id: None,
            share_input: String::new(),
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
            quiet_update: false,
            note_images: NoteImages::default(),
            caret_after_insert: None,
            requested_pictures: BTreeSet::new(),
            shared_uploads: BTreeSet::new(),
            picture_pick: None,
            kofi: KofiMark::load(ctx),
        };
        // Every client needs a name others can see and a secret the server can recognise it by.
        // A fresh install gets both so nothing has to be filled in before the shared library
        // works; Settings lets the name be replaced with any word and the secret be copied to
        // another computer.
        if app.display_name.trim().is_empty() {
            app.display_name = default_display_name();
            app.persist_settings();
        }
        if app.identifier.trim().is_empty() {
            app.identifier = fresh_identifier();
            app.persist_settings();
        }
        app
    }

    fn persist_settings(&self) {
        let settings = AppSettings {
            theme_name: self.theme.name().to_string(),
            show_formatting_bar: self.show_formatting_bar,
            roomy_cards: self.roomy_cards,
            sort: self.sort,
            window: self.window_state,
            show_card_previews: self.show_card_previews,
            highlight_rgb: self.highlight_rgb,
            text_color_rgb: self.text_color_rgb,
            font_size: self.font_size,
            glow_strength: self.glow_strength,
            colors: self.colors,
            collapsed_folders: self.collapsed_folders.iter().cloned().collect(),
            server: self.server.clone(),
            display_name: self.display_name.clone(),
            identifier: self.identifier.clone(),
            peer_color: self.peer_color,
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

        // The text is laid out here, at a width the window really has, because an `egui::Area`
        // hands its content the width of the *previous* frame's area. A long message arriving
        // after a short one — "Connecting…", then "could not connect to …" — was squeezed into
        // that smaller width and came out as a column one word wide.
        let screen = ctx.screen_rect();
        let toast_width = (screen.width() - 32.0).clamp(120.0, TOAST_MAX_WIDTH);
        let wrap = (toast_width - TOAST_PADDING).max(80.0);
        // Normally the notice sits beside the sidebar; in a window too narrow for a readable
        // sentence it moves left so the words stay in lines instead of a strip.
        let toast_left = if self.focus_mode {
            16.0
        } else {
            SIDEBAR_WIDTH + 16.0
        };
        let toast_left = toast_left.min((screen.right() - 16.0 - toast_width).max(16.0));

        let glyph = if is_problem { icon::X } else { icon::CHECK };
        let color = if is_problem { p.danger } else { p.fg_secondary };
        let galley = ctx.fonts(|fonts| {
            fonts.layout(
                format!("{glyph} {message}"),
                egui::FontId::proportional(11.5),
                color,
                wrap,
            )
        });

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
                        let (rect, _) = ui.allocate_exact_size(galley.size(), Sense::hover());
                        ui.painter().galley(rect.min, galley.clone(), color);
                    });
            });
        ctx.request_repaint_after(Duration::from_secs_f32(remaining as f32));
    }

    /// Kicks off a version check on a worker thread so the frame does not wait for GitHub.
    /// `quiet` is set when Settings asked for it: the outcome is reported there instead of in a
    /// popup, and "already current" is worth saying because the user asked.
    fn start_update_check(&mut self, ctx: &egui::Context, quiet: bool) {
        update::cleanup_stale_updates();
        let (tx, rx) = mpsc::channel();
        self.update_rx = Some(rx);
        self.quiet_update = quiet;
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
                // there is nothing worth interrupting the user for unless they asked.
                UpdateEvent::Checked(Ok(None)) => {
                    self.update_rx = None;
                    self.update_state = if self.quiet_update {
                        UpdateState::UpToDate
                    } else {
                        UpdateState::Idle
                    };
                }
                UpdateEvent::Checked(Err(error)) => {
                    self.update_rx = None;
                    self.update_state = if self.quiet_update {
                        UpdateState::Error(error)
                    } else {
                        UpdateState::Idle
                    };
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
            UpdateState::Idle | UpdateState::UpToDate | UpdateState::Checking => {}
            // A check from Settings keeps its result in that window, so nothing pops up over the
            // note the user is reading.
            UpdateState::Available(_) if self.quiet_update => {}
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

    /// Follows the window as it moves and resizes, so what is written down at exit is the window
    /// as it was. A maximised window keeps the size and place it had before: restoring it with
    /// the maximised size would open it full-screen the next time it is un-maximised.
    fn track_window(&mut self, ctx: &egui::Context) {
        let (inner, outer, maximized) = ctx.input(|i| {
            let viewport = i.viewport();
            (
                viewport.inner_rect,
                viewport.outer_rect,
                viewport.maximized.unwrap_or(false),
            )
        });
        let Some(inner) = inner else {
            return;
        };
        let mut state = self.window_state.unwrap_or(WindowState {
            width: inner.width(),
            height: inner.height(),
            x: inner.min.x,
            y: inner.min.y,
            maximized,
        });
        state.maximized = maximized;
        if !maximized {
            state.width = inner.width();
            state.height = inner.height();
            if let Some(outer) = outer {
                state.x = outer.min.x;
                state.y = outer.min.y;
            }
        }
        self.window_state = Some(state);
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
                        changed |= ui
                            .checkbox(&mut self.roomy_cards, "Tall note cards")
                            .on_hover_text(
                                "Off, each note is one line — a small icon and its title — so about \
                                 twice as many fit in the list; hovering shows the date and tags. \
                                 On brings back the taller card with its date and tag row.",
                            )
                            .changed();
                        changed |= ui
                            .checkbox(
                                &mut self.show_card_previews,
                                "Show a preview line on note cards",
                            )
                            .on_hover_text(
                                "Off leaves just the title, date and tags, so the sidebar's \
                                 cards are shorter and more notes fit at once. On adds the first \
                                 lines of each note. Only the tall cards have room for it.",
                            )
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
                                    ("Tab / Shift+Tab", "Indent / unindent"),
                                    ("Enter", "Keep list & indentation"),
                                    ("Shift+Enter", "Stay inside quote / item"),
                                    ("Ctrl+F", "Find in the open note"),
                                    ("Ctrl+Shift+F", "Search all notes"),
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

                        ui.label(RichText::new("Shared library").strong());
                        self.shared_library_settings(ui, &defaults);
                        ui.separator();

                        ui.label(RichText::new("Updates").strong());
                        self.update_settings_row(ui, &defaults);
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
                            self.show_card_previews = d.show_card_previews;
                            self.roomy_cards = d.roomy_cards;
                            self.sort = d.sort;
                            self.peer_color = d.peer_color;
                            self.collab.set_color(self.peer_color);
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

    /// The shared library section of Settings: where the server is and whether we are on it.
    fn shared_library_settings(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let connected = self.collab.is_connected();
        let state_line = match self.collab.state() {
            ConnectionState::Offline => {
                if self.collab.is_active() {
                    "Reconnecting…".to_string()
                } else {
                    "Not connected".to_string()
                }
            }
            ConnectionState::Connecting => "Connecting…".to_string(),
            ConnectionState::Online { server, you } => format!("Connected to {server} as {you}"),
        };
        ui.label(RichText::new(state_line).size(11.5).color(if connected {
            p.accent
        } else {
            p.fg_secondary
        }));
        // Why it did not connect, kept in view instead of only flashing by in the toast: the
        // usual answers are a port that is not forwarded and a firewall that drops it.
        if !connected {
            if let Some(problem) = self.collab.last_error() {
                ui.add(
                    egui::Label::new(
                        RichText::new(format!("{} {problem}", icon::ALERT_TRIANGLE))
                            .size(11.0)
                            .color(p.fg_muted),
                    )
                    .wrap(),
                );
            }
        }
        let mut watchers: Vec<&str> = self
            .notes
            .iter()
            .filter(|n| self.is_remote(&n.id))
            .flat_map(|n| self.collab.watchers(&n.id).iter().map(|u| u.name.as_str()))
            .collect();
        watchers.sort_unstable();
        watchers.dedup();
        if !watchers.is_empty() {
            ui.label(
                RichText::new(format!(
                    "{} In a note right now: {}",
                    icon::USERS,
                    watchers.join(", ")
                ))
                .size(11.0)
                .color(p.fg_muted),
            );
        }
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            ui.label("Address");
            ui.add(
                TextEdit::singleline(&mut self.server_form.host)
                    .hint_text("192.168.1.20 or notes.example.com")
                    .desired_width(210.0),
            );
        });
        ui.horizontal(|ui| {
            ui.label("Port");
            let mut port = self.server_form.port.to_string();
            if ui
                .add(TextEdit::singleline(&mut port).desired_width(70.0))
                .changed()
            {
                if let Ok(parsed) = port.trim().parse::<u16>() {
                    self.server_form.port = parsed;
                }
            }
            ui.label("Password");
            ui.add(
                TextEdit::singleline(&mut self.server_form.password)
                    .password(true)
                    .desired_width(140.0),
            );
        });
        // The name is public: the server and everyone on it see it. The identifier is the
        // secret behind it, which only the server reads, so nobody can be somebody else just by
        // typing their name. The same identifier on another computer is the same person.
        let mut name_changed = false;
        let mut name_committed = false;
        let mut identifier_changed = false;
        let mut identifier_committed = false;
        ui.horizontal(|ui| {
            ui.label("Your name");
            let account = crate::net::os_username();
            let typed = ui
                .add(
                    TextEdit::singleline(&mut self.display_name)
                        .hint_text(&account)
                        .desired_width(170.0),
                )
                .on_hover_text(
                    "The name others see next to your caret, and the one they add to a note to \
                     share it with you. It has to be free on this server: the first person to use \
                     a name keeps it.",
                );
            name_changed = typed.changed();
            name_committed = typed.lost_focus();
        });
        // The identifier is a key, not a word: it is made once by the app, copied to another
        // computer to be the same person there, and never edited for fun. There is no "make a
        // new one" button because the server keeps a name bound to the key that claimed it —
        // a fresh key would be a stranger, locked out of the name and the notes behind it.
        ui.horizontal(|ui| {
            ui.label("Identifier");
            let typed = ui
                .add(
                    TextEdit::singleline(&mut self.identifier)
                        .password(true)
                        .desired_width(170.0),
                )
                .on_hover_text(
                    "The secret behind your name. Only the server reads it, to know it is really \
                     you; nobody else ever sees it, so nobody can connect as you. It is how your \
                     notes follow you to another computer: copy this value into the same box \
                     there. Treat it like a password.",
                );
            let long_enough = self.identifier.trim().len() >= MIN_IDENTIFIER;
            identifier_changed = typed.changed() && long_enough;
            // A pasted key only takes effect when the box is left, and only if it is a different
            // one from the connection already open: clicking Copy must not re-key.
            identifier_committed =
                typed.lost_focus() && long_enough && self.identifier != self.sent_identifier;
            if ui
                .small_button("Copy")
                .on_hover_text("Copy this identifier, to paste into EloraNotes on another computer")
                .clicked()
            {
                ui.ctx().copy_text(self.identifier.clone());
                self.status_message = Some(
                    "Identifier copied; paste it into EloraNotes on your other computer"
                        .to_string(),
                );
            }
        });
        if self.identifier.trim().len() < MIN_IDENTIFIER {
            ui.label(
                RichText::new(
                    "This identifier is too short to be a key: it needs at least 16 characters. \
                     Paste the one from your other computer here.",
                )
                .size(10.5)
                .color(p.danger),
            );
        }
        // The public half of the identifier: what a note records as its owner or as somebody it
        // was shared with. It cannot be turned back into the identifier, which is why it is safe
        // to show — and it is what `elora-server adopt` takes, so it needs to be copyable.
        let public_key = crate::people::public_key(self.identifier.trim());
        ui.horizontal(|ui| {
            ui.label("Your key");
            ui.label(
                RichText::new(crate::net::short_fingerprint(&public_key))
                    .size(10.5)
                    .color(p.fg_muted),
            )
            .on_hover_text(
                "The public half of your identifier. Notes record it, not your name, so they \
                 follow you even after you rename yourself. Give it to whoever runs the server \
                 when older notes have to be handed over.",
            );
            if ui
                .small_button("Copy")
                .on_hover_text("Copy your full key")
                .clicked()
            {
                ui.ctx().copy_text(public_key.clone());
                self.status_message = Some("Key copied".to_string());
            }
        });
        // How this person looks in somebody else's note: the colour of their caret and of the
        // name beside it. Nothing on this screen changes, so the hint says so in words.
        let mut color_changed = false;
        ui.horizontal(|ui| {
            ui.label("Your colour");
            let automatic = crate::collab::default_peer_color(&self.server_display_name());
            let mut chosen = self.peer_color.unwrap_or(automatic);
            if ui
                .color_edit_button_srgb(&mut chosen)
                .on_hover_text(
                    "The colour others see your name and caret in while a note is shared",
                )
                .changed()
            {
                self.peer_color = Some(chosen);
                color_changed = true;
            }
            if self.peer_color.is_some()
                && ui
                    .small_button("Automatic")
                    .on_hover_text("Go back to the colour picked from your name")
                    .clicked()
            {
                self.peer_color = None;
                color_changed = true;
            }
        });
        ui.label(
            RichText::new(
                "Your colour in a shared note: the name beside your caret and the caret itself, \
                 as it looks on everybody else's screen. Nothing here changes.",
            )
            .size(10.5)
            .color(p.fg_muted),
        );
        if color_changed {
            // Applies to the people already in a note with you, and is remembered for the next
            // connection, so it never has to be picked again.
            self.collab.set_color(self.peer_color);
            self.persist_settings();
        }
        if name_changed || identifier_changed {
            self.persist_settings();
        }
        if (name_committed || identifier_committed) && self.collab.is_connected() {
            // Which notes the server shows depends on who it thinks this client is, so this is a
            // new conversation with the server rather than a rename while it is open.
            self.disconnect_server();
            self.connect_server();
        }

        ui.horizontal(|ui| {
            if connected {
                if ui
                    .button(format!("{}  Disconnect", icon::WIFI_OFF))
                    .clicked()
                {
                    self.disconnect_server();
                }
            } else if ui.button(format!("{}  Connect", icon::CLOUD)).clicked() {
                self.connect_server();
            }
            if self.server.fingerprint.is_some()
                && ui
                    .small_button(format!("{}  Forget this server's key", icon::RESTORE))
                    .on_hover_text("Ask for the certificate code again on the next connection")
                    .clicked()
            {
                self.server.fingerprint = None;
                self.server_form.fingerprint = None;
                self.persist_settings();
            }
        });

        if self.server.fingerprint.is_some() {
            let short = crate::net::short_fingerprint(
                self.server.fingerprint.as_deref().unwrap_or_default(),
            );
            ui.label(
                RichText::new(format!("Trusted key {short}"))
                    .size(10.5)
                    .color(p.fg_muted),
            );
        }
        ui.label(
            RichText::new(
                "The server's notes appear in the sidebar under its name. A note can be closed to \
                 everybody but the people you name on it; they add your name, and the server \
                 knows it is you by the identifier only it can see.",
            )
            .size(10.5)
            .color(p.fg_muted),
        );
    }

    /// The update section of Settings: what is running, what the last check found, and the one
    /// button that acts on it. Everything the launch prompt can do is reachable from here too.
    fn update_settings_row(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let ctx = ui.ctx().clone();
        let mut check = false;
        let mut install: Option<Release> = None;

        ui.label(
            RichText::new(format!(
                "You are running EloraNotes {}.",
                update::CURRENT_VERSION
            ))
            .size(11.5)
            .color(p.fg_muted),
        );

        match self.update_state.clone() {
            UpdateState::Checking => {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(12.0));
                    ui.label(RichText::new("Checking for updates…").size(11.5));
                });
            }
            UpdateState::Downloading(release) => {
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(12.0));
                    ui.label(RichText::new(format!("Downloading {}…", release.version)).size(11.5));
                });
                ui.label(
                    RichText::new("EloraNotes restarts itself when the download finishes.")
                        .size(11.0)
                        .color(p.fg_muted),
                );
            }
            UpdateState::Available(UpdateOffer::SelfUpdate(release)) => {
                ui.label(
                    RichText::new(format!("EloraNotes {} is available.", release.version))
                        .size(12.0)
                        .strong(),
                );
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new(format!("{}  Update and restart", icon::DOWNLOAD))
                                .size(12.0),
                        )
                        .fill(p.accent),
                    )
                    .clicked()
                {
                    install = Some(release);
                }
            }
            UpdateState::Available(UpdateOffer::Managed { version, manager })
            | UpdateState::Managed { version, manager } => {
                ui.label(
                    RichText::new(format!("EloraNotes {version} is available."))
                        .size(12.0)
                        .strong(),
                );
                ui.label(
                    RichText::new(format!(
                        "This copy was installed with {manager}, which owns its own files."
                    ))
                    .size(11.0)
                    .color(p.fg_muted),
                );
                let command = format!(
                    "flatpak install --reinstall ./EloraNotes-{version}-linux-x86_64.flatpak"
                );
                ui.label(RichText::new(&command).size(11.0).monospace());
                if ui
                    .small_button(format!("{}  Copy command", icon::COPY))
                    .clicked()
                {
                    ui.ctx().copy_text(command);
                    self.status_message = Some("Copied the update command".to_string());
                }
            }
            UpdateState::UpToDate => {
                ui.label(
                    RichText::new(format!("{}  You're up to date.", icon::CHECK))
                        .size(11.5)
                        .color(p.fg_secondary),
                );
                if ui
                    .small_button(format!("{}  Check again", icon::REFRESH))
                    .clicked()
                {
                    check = true;
                }
            }
            UpdateState::Error(message) => {
                ui.label(RichText::new(message).size(11.5).color(p.danger));
                if ui
                    .small_button(format!("{}  Try again", icon::REFRESH))
                    .clicked()
                {
                    check = true;
                }
            }
            UpdateState::Idle => {
                if ui
                    .button(format!("{}  Check for updates", icon::REFRESH))
                    .clicked()
                {
                    check = true;
                }
            }
        }

        if check {
            self.start_update_check(&ctx, true);
        } else if let Some(release) = install {
            self.start_update_download(&ctx, release);
        }
    }

    fn resort(&mut self) {
        sort_notes(&mut self.notes);
    }

    /// True when the note lives on the server and must never be written to this disk.
    fn is_remote(&self, id: &str) -> bool {
        self.remote_ids.contains(id)
    }

    /// Saves a note where it belongs: to disk, or back to the server it came from.
    fn persist_note(&mut self, note_idx: usize) -> Result<(), String> {
        let note = self.notes[note_idx].clone();
        if !self.is_remote(&note.id) {
            return self.storage.save_note(&note).map_err(|e| e.to_string());
        }
        // Only the fields that live in the frontmatter; the body has its own live channel, and
        // sending the metadata on every keystroke would spam every connection.
        let unchanged = self
            .collab
            .notes()
            .iter()
            .find(|m| m.id == note.id)
            .is_some_and(|m| {
                m.title == note.title
                    && m.folder == note.folder
                    && m.tags == note.explicit_tags
                    && m.pinned == note.pinned
                    && m.trashed == note.trashed
                    && m.icon == note.icon
            });
        if !unchanged {
            self.collab.update_note(&NoteMeta {
                id: note.id.clone(),
                title: note.title,
                folder: note.folder,
                tags: note.explicit_tags,
                pinned: note.pinned,
                trashed: note.trashed,
                icon: note.icon,
                updated_at: note.updated_at.timestamp(),
                // Sharing has its own message, sent from the panel that changes it, so an
                // ordinary edit never carries it and can never undo somebody else's.
                owner: note.owner.clone(),
                owner_key: note.owner_key.clone(),
                private: note.private,
                shared_with: note.shared_with.clone(),
                shared_keys: note.shared_keys.clone(),
            });
        }
        if self.collab_open.as_deref() == Some(note.id.as_str())
            && !self.room_body_pending(&note.id)
        {
            self.collab.send_body(&note.id, &note.body);
        }
        Ok(())
    }

    /// Where pictures dropped or pasted into a note are kept: a folder beside the note files, so
    /// the Markdown can name them with a path relative to the note.
    fn attachments_dir(&self) -> PathBuf {
        self.storage.root_dir().join("attachments")
    }

    /// Files a picture away under a name that is safe to put in Markdown (no spaces to end the
    /// address early, no characters a file system dislikes) and returns the path to write into
    /// the note, with the plain file name beside it.
    ///
    /// The name carries a digest of the picture's bytes. That keeps two people's
    /// `screenshot.png` apart in a shared library, makes the same picture file once however
    /// often it is added, and lets a shared note ask the server for exactly the file it names.
    fn store_picture(
        &self,
        bytes: &[u8],
        name_hint: &str,
        extension: &str,
    ) -> Result<(String, String), String> {
        let dir = self.attachments_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
        let file = stored_picture_name(bytes, name_hint, extension);
        let path = dir.join(&file);
        // A name that already exists holds these very bytes, so there is nothing to write again.
        if !path.is_file() {
            std::fs::write(&path, bytes).map_err(|e| format!("Could not save the picture: {e}"))?;
        }
        Ok((format!("attachments/{file}"), file))
    }

    /// Puts a picture into a note at the caret: the file is copied into the vault's pictures
    /// folder and a `![name](attachments/…)` line is written where the user is typing.
    fn insert_picture(
        &mut self,
        note_idx: usize,
        bytes: &[u8],
        name_hint: &str,
        extension: &str,
    ) -> Result<(), String> {
        let (src, file) = self.store_picture(bytes, name_hint, extension)?;
        let alt = name_hint.trim();
        let alt = if alt.is_empty() {
            "picture".to_string()
        } else {
            alt.to_string()
        };
        let snippet = format!("![{alt}]({src})");
        let note_id = self.notes[note_idx].id.clone();
        self.insert_markdown_at_caret(note_idx, &snippet)?;

        // A shared note is looked at by other people, and they have no access to this disk, so
        // the picture itself goes to the library as well.
        if self.is_remote(&note_id) {
            if bytes.len() > crate::net::MAX_ATTACHMENT {
                self.status_message = Some(format!(
                    "Added {alt} — too large to share with the library (over 12 MB)"
                ));
            } else {
                self.shared_uploads.insert(file.clone());
                self.collab.upload_attachment(&file, bytes.to_vec());
                self.status_message = Some(format!("Added {alt} — sharing it with the library"));
            }
        } else {
            self.status_message = Some(format!(
                "Added {alt} — drag its corner in Preview to resize it"
            ));
        }
        Ok(())
    }

    /// Stores a picture the library sent, where the preview looks for it. Only a plain picture
    /// name is accepted, so a picture can never be written outside the pictures folder, and a
    /// file that is already here is left alone.
    fn save_shared_picture(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        if !crate::net::valid_attachment_name(name) {
            return Err(format!("{name} is not a picture name"));
        }
        if bytes.is_empty() || bytes.len() > crate::net::MAX_ATTACHMENT {
            return Err(format!("{name} is not a usable picture"));
        }
        let dir = self.attachments_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
        let path = dir.join(name);
        if path.is_file() {
            return Ok(());
        }
        std::fs::write(&path, bytes).map_err(|e| format!("Could not save {name}: {e}"))
    }

    /// Asks the library for the pictures a shared note names that this computer has not got.
    /// Each is asked for once per connection; when a picture arrives it is written into the
    /// vault and the preview draws it on the next frame.
    fn request_shared_pictures(&mut self, body: &str) {
        for name in picture_references(body) {
            if self.attachments_dir().join(&name).is_file() {
                continue;
            }
            if !self.requested_pictures.insert(name.clone()) {
                continue;
            }
            self.collab.fetch_attachment(&name);
        }
    }

    /// Sends the pictures a note names to the library, for when a local note is moved there.
    fn upload_pictures(&mut self, body: &str) {
        for name in picture_references(body) {
            let path = self.attachments_dir().join(&name);
            match std::fs::read(&path) {
                Ok(bytes) => self.collab.upload_attachment(&name, bytes),
                Err(e) => {
                    self.status_message = Some(format!("Could not share {}: {e}", path.display()));
                    return;
                }
            }
        }
    }

    /// Adds a picture that is already on this computer, chosen from the toolbar's picture
    /// button: the file is read, filed away next to the notes and written into the note.
    fn add_picture_file(&mut self, note_idx: usize, path: &Path) -> Result<(), String> {
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !is_picture_extension(&extension) {
            return Err(format!(
                "{} is not a picture EloraNotes can show (PNG, JPEG and GIF)",
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("That file")
            ));
        }
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(str::to_string)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "picture".to_string());
        let bytes =
            std::fs::read(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
        self.insert_picture(note_idx, &bytes, &name, &extension)
    }

    /// The toolbar's picture button: the system's own file dialog, starting where pictures
    /// usually are, then the chosen file goes into the note like a dropped one.
    fn pick_picture(&mut self, ctx: &egui::Context, note_idx: usize) {
        if self.picture_pick.is_some() {
            self.status_message = Some("A picture is already being chosen".to_string());
            return;
        }
        self.picture_pick = Some(PicturePick {
            note_id: self.notes[note_idx].id.clone(),
            result: choose_picture_file(ctx),
        });
    }

    /// Picks up the file dialog's answer, if it has one by now, and adds the picture to the note
    /// it was opened from. Called every frame, because the answer arrives between frames.
    fn poll_picture_pick(&mut self) {
        let Some(pick) = self.picture_pick.as_ref() else {
            return;
        };
        let note_id = pick.note_id.clone();
        let answer = pick.result.try_recv();
        match answer {
            Ok(Some(path)) => {
                self.picture_pick = None;
                match self.notes.iter().position(|note| note.id == note_id) {
                    Some(note_idx) => {
                        if let Err(e) = self.add_picture_file(note_idx, &path) {
                            self.status_message = Some(e);
                        }
                    }
                    // The note was closed, copied away or the server dropped it while the dialog
                    // was open; there is nowhere to put the picture now.
                    None => {
                        self.status_message =
                            Some("That note is gone, so the picture was not added".to_string())
                    }
                }
            }
            Ok(None) => self.picture_pick = None,
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.picture_pick = None,
        }
    }

    /// Inserts a block on lines of its own at the caret (replacing the selection, when there is
    /// one) and leaves the caret after it.
    fn insert_markdown_at_caret(&mut self, note_idx: usize, snippet: &str) -> Result<(), String> {
        let note_id = self.notes[note_idx].id.clone();
        let body = self.notes[note_idx].body.clone();
        let length = body.chars().count();
        // The caret only means something for the note the editor is actually showing; after a
        // switch (or from the list) the picture goes at the end instead of into an old position.
        let editor_showing_note = self
            .last_seen_body
            .as_ref()
            .is_some_and(|(seen, _)| seen == &note_id);
        let (start_char, end_char) = match editor_showing_note {
            true => self
                .last_editor_selection
                .map(|(start, end)| (start.min(length), end.min(length)))
                .unwrap_or_else(|| {
                    let caret = self.last_editor_caret.unwrap_or(length).min(length);
                    (caret, caret)
                }),
            false => (length, length),
        };
        let (start_char, end_char) = (start_char.min(end_char), end_char.max(start_char));

        let byte_at = |char_index: usize| {
            body.char_indices()
                .nth(char_index)
                .map(|(byte, _)| byte)
                .unwrap_or(body.len())
        };
        let (start_byte, end_byte) = (byte_at(start_char), byte_at(end_char));
        let before = &body[..start_byte];
        let after = &body[end_byte..];

        let mut inserted = String::new();
        if !before.is_empty() && !before.ends_with('\n') {
            inserted.push('\n');
        }
        inserted.push_str(snippet);
        let caret = start_char + inserted.chars().count();
        if !after.starts_with('\n') {
            inserted.push('\n');
        }

        self.notes[note_idx].body = format!("{before}{inserted}{after}");
        self.notes[note_idx].updated_at = Utc::now();
        self.last_editor_selection = Some((caret, caret));
        self.last_editor_caret = Some(caret);
        self.caret_after_insert = Some((note_id, caret));
        self.persist_note(note_idx)
    }

    /// Pictures dropped onto the window, added to the open note in the order they arrived.
    fn add_dropped_pictures(&mut self, files: Vec<egui::DroppedFile>) {
        let Some(note_idx) = self
            .selected_id
            .as_ref()
            .and_then(|id| self.notes.iter().position(|note| &note.id == id))
        else {
            self.status_message = Some("Open a note before dropping a picture into it".to_string());
            return;
        };

        let mut added = 0usize;
        let mut problems: Vec<String> = Vec::new();
        for file in files {
            let result = match (file.path.as_ref(), file.bytes) {
                // Some desktops hand over the file's contents instead of, or as well as, its
                // path; those do not need to be read again.
                (_, Some(bytes)) => {
                    let name = file
                        .path
                        .as_ref()
                        .and_then(|path| path.file_stem())
                        .and_then(|stem| stem.to_str())
                        .map(str::to_string)
                        .or_else(|| Some(file.name.clone()))
                        .filter(|name| !name.trim().is_empty())
                        .unwrap_or_else(|| "picture".to_string());
                    let extension = file
                        .path
                        .as_ref()
                        .and_then(|path| path.extension())
                        .and_then(|extension| extension.to_str())
                        .unwrap_or("png")
                        .to_ascii_lowercase();
                    match is_picture_extension(&extension) {
                        true => self.insert_picture(note_idx, &bytes, &name, &extension),
                        false => Err(format!(
                            "{}: {extension} pictures are not supported",
                            file.name
                        )),
                    }
                }
                (Some(path), None) => self.add_picture_file(note_idx, path),
                (None, None) => Err(format!("{}: could not be read", file.name)),
            };
            match result {
                Ok(()) => added += 1,
                Err(e) => problems.push(e),
            }
        }

        self.status_message = match (added, problems.first()) {
            (0, Some(problem)) => Some(problem.clone()),
            (0, None) => None,
            (count, None) => Some(format!(
                "Added {count} picture{}",
                if count == 1 { "" } else { "s" }
            )),
            (count, Some(problem)) => Some(format!(
                "Added {count} picture{}; {problem}",
                if count == 1 { "" } else { "s" }
            )),
        };
    }

    /// The identifier this client goes by: what was typed in Settings, or the account name when
    /// a settings file from before identifiers has nothing in it.
    fn server_display_name(&self) -> String {
        let typed = self.display_name.trim();
        if typed.is_empty() {
            crate::net::os_username()
        } else {
            typed.to_string()
        }
    }

    /// Connects to the server in Settings and remembers the details.
    fn connect_server(&mut self) {
        self.server = self.server_form.clone();
        self.persist_settings();
        if !self.server.is_configured() {
            self.status_message = Some("Enter the server address and password first".to_string());
            return;
        }
        // A short key is not a secret, so it is refused here with a hint rather than at the
        // server with a sentence about keys: this is the box it has to be fixed in.
        if self.identifier.trim().len() < MIN_IDENTIFIER {
            self.status_message = Some(format!(
                "The identifier needs at least {MIN_IDENTIFIER} characters; paste the one from \
                 your other computer into Settings → Shared library"
            ));
            return;
        }
        self.remote_bodies.clear();
        // A name is what the server binds this key to, so it is never left blank: an empty box
        // would otherwise fall back to the account name, which somebody else may already have.
        if self.display_name.trim().is_empty() {
            self.display_name = default_display_name();
            self.persist_settings();
        }
        let name = self.server_display_name();
        let identifier = self.identifier.clone();
        self.sent_identifier = identifier.clone();
        self.collab.set_color(self.peer_color);
        self.collab.connect(self.server.clone(), name, identifier);
        self.status_message = Some(format!("Connecting to {}…", self.server.address()));
    }

    fn disconnect_server(&mut self) {
        self.collab.disconnect();
        self.collab_open = None;
        self.awaiting_room_body = None;
        self.drop_remote_notes();
        self.status_message = Some("Left the shared library".to_string());
    }

    /// Removes the server's notes from the list after disconnecting.
    fn drop_remote_notes(&mut self) {
        if self.remote_ids.is_empty() {
            return;
        }
        let ids = std::mem::take(&mut self.remote_ids);
        self.notes.retain(|n| !ids.contains(&n.id));
        self.remote_bodies.clear();
        if self
            .selected_id
            .as_deref()
            .is_some_and(|id| ids.contains(id))
        {
            self.selected_id = None;
            self.select_first_visible();
        }
    }

    /// Moves a note from this computer into the shared library: the server gets a copy with the
    /// same title, body, folder, tags, pin and icon, and the local file goes away once it exists
    /// there. Called from the folder menu on a local note.
    fn move_note_to_server(&mut self, note_idx: usize) {
        let server = match self.collab.server_name() {
            Some(name) if self.collab.is_connected() => name.to_string(),
            _ => {
                self.status_message = Some("Connect to a server before moving a note there".into());
                return;
            }
        };
        let note = self.notes[note_idx].clone();
        if self.is_remote(&note.id) {
            return;
        }
        self.pending_server_move = Some(PendingMove {
            local_id: note.id.clone(),
            title: note.title.clone(),
            body: note.body.clone(),
        });
        self.collab.create_note(
            &note.title,
            note.folder.as_deref(),
            &note.explicit_tags,
            &note.body,
            note.pinned,
            note.icon.as_deref(),
        );
        self.status_message = Some(format!("Moving “{}” to {server}…", note.display_title()));
    }

    /// Copies a shared note onto this computer, keeping the shared one in place.
    fn copy_shared_note_to_computer(&mut self, note_idx: usize) {
        let shared = self.notes[note_idx].clone();
        let mut note = Note::new(
            shared.title.clone(),
            shared.body.clone(),
            self.storage.root_dir(),
        );
        note.folder = shared.folder.clone();
        note.explicit_tags = shared.explicit_tags.clone();
        note.icon = shared.icon.clone();
        note.pinned = shared.pinned;
        if let Err(e) = self.storage.save_note(&note) {
            self.status_message = Some(format!("Could not copy the note: {e}"));
            return;
        }
        let id = note.id.clone();
        self.notes.push(note);
        self.resort();
        self.selected_id = Some(id);
        // The copy names the same pictures; ones this computer has not got are fetched from the
        // library, so the copy is not left with holes.
        let body = shared.body.clone();
        self.request_shared_pictures(&body);
        self.status_message = Some(format!("Copied “{}” to this computer", shared.title));
    }

    /// Rebuilds the server's notes from what the connection last reported, keeping the body the
    /// editor is currently showing for the open note so typing is never yanked backwards.
    /// `editing_title` says the caret is in the title field of the note on screen: that title is
    /// being typed, so a copy arriving from the server does not replace it.
    fn apply_library(&mut self, editing_title: bool) {
        let metas = self.collab.notes().to_vec();
        let selected = self.selected_id.clone();
        let live: BTreeSet<String> = metas.iter().map(|m| m.id.clone()).collect();
        self.notes
            .retain(|n| !self.remote_ids.contains(&n.id) || live.contains(&n.id));
        self.remote_ids.retain(|id| live.contains(id));

        for meta in metas {
            let updated = chrono::TimeZone::timestamp_opt(&Utc, meta.updated_at, 0)
                .single()
                .unwrap_or_else(Utc::now);
            match self.notes.iter_mut().find(|n| n.id == meta.id) {
                Some(note) => {
                    if !(editing_title && selected.as_deref() == Some(meta.id.as_str())) {
                        note.title = meta.title;
                    }
                    note.folder = meta.folder;
                    note.explicit_tags = meta.tags;
                    note.pinned = meta.pinned;
                    note.trashed = meta.trashed;
                    note.icon = meta.icon;
                    note.owner = meta.owner.clone();
                    note.private = meta.private;
                    note.shared_with = meta.shared_with.clone();
                    note.updated_at = updated;
                    if self.collab_open.as_deref() != Some(meta.id.as_str()) {
                        if let Some(body) = self.remote_bodies.get(&meta.id) {
                            note.body = body.clone();
                        }
                    }
                }
                None => {
                    let body = self
                        .remote_bodies
                        .get(&meta.id)
                        .cloned()
                        .unwrap_or_default();
                    self.notes.push(Note {
                        id: meta.id.clone(),
                        title: meta.title,
                        body,
                        pinned: meta.pinned,
                        trashed: meta.trashed,
                        explicit_tags: meta.tags,
                        icon: meta.icon,
                        folder: meta.folder,
                        owner: meta.owner,
                        owner_key: meta.owner_key,
                        private: meta.private,
                        shared_with: meta.shared_with,
                        shared_keys: meta.shared_keys,
                        created_at: updated,
                        updated_at: updated,
                        file_path: PathBuf::new(),
                    });
                }
            }
            self.remote_ids.insert(meta.id);
        }
        self.resort();
        // A note that was closed to this person while they were reading it takes their
        // selection with it, so the window is not left showing a note that is no longer theirs.
        if self
            .selected_id
            .as_ref()
            .is_some_and(|id| !self.notes.iter().any(|note| &note.id == id))
        {
            self.select_first_visible();
        }
    }

    /// Drains the connection's messages and folds them into the app state.
    fn poll_collab(&mut self, ctx: &egui::Context) {
        let mut library_changed = false;
        for event in self.collab.poll() {
            match event {
                Incoming::Online { server, .. } => {
                    self.status_message = Some(format!("Connected to {server}"));
                    // A fresh connection starts with no note mirrored, so the selected shared
                    // note is reopened and re-synced.
                    self.collab_open = None;
                    self.awaiting_room_body = None;
                    // Pictures asked for over a connection that has since dropped were never
                    // answered, so they are asked for again.
                    self.requested_pictures.clear();
                    library_changed = true;
                }
                Incoming::Notes(_) => library_changed = true,
                // A note that is not this person's to change, or one that was closed to them
                // while they had it open. The connection stays up; the reason is worth saying.
                Incoming::Refused(message) => self.status_message = Some(message),
                Incoming::Created(note) => {
                    // Open it straight away: the library broadcast that follows makes it a
                    // normal shared note in the list.
                    if !self.notes.iter().any(|n| n.id == note.id) {
                        self.remote_ids.insert(note.id.clone());
                        let updated = chrono::TimeZone::timestamp_opt(&Utc, note.updated_at, 0)
                            .single()
                            .unwrap_or_else(Utc::now);
                        self.notes.push(Note {
                            id: note.id.clone(),
                            title: note.title.clone(),
                            body: self
                                .remote_bodies
                                .get(&note.id)
                                .cloned()
                                .unwrap_or_default(),
                            pinned: note.pinned,
                            trashed: note.trashed,
                            explicit_tags: note.tags.clone(),
                            icon: note.icon.clone(),
                            folder: note.folder.clone(),
                            owner: note.owner.clone(),
                            owner_key: note.owner_key.clone(),
                            private: note.private,
                            shared_with: note.shared_with.clone(),
                            shared_keys: note.shared_keys.clone(),
                            created_at: updated,
                            updated_at: updated,
                            file_path: PathBuf::new(),
                        });
                        self.resort();
                    }
                    self.selected_id = Some(note.id.clone());
                    self.sidebar_section = SidebarSection::AllNotes;

                    // A move finishes here: the server has the note, so the local file goes and
                    // the body that travelled with it is shown without waiting for the room.
                    let moved = self.pending_server_move.take().filter(|pending| {
                        pending.title == note.title && !self.remote_ids.contains(&pending.local_id)
                    });
                    if let Some(pending) = moved {
                        self.remote_bodies
                            .insert(note.id.clone(), pending.body.clone());
                        if let Some(local) =
                            self.notes.iter_mut().find(|n| n.id == pending.local_id)
                        {
                            local.body = pending.body.clone();
                        }
                        if let Some(pos) = self.notes.iter().position(|n| n.id == pending.local_id)
                        {
                            let removed = self.notes.remove(pos);
                            let _ = self.storage.delete_note(&removed);
                        }
                        if let Some(local) = self.notes.iter_mut().find(|n| n.id == note.id) {
                            local.body = pending.body.clone();
                        }
                        self.upload_pictures(&pending.body);
                        let server = self.collab.server_name().unwrap_or("the server");
                        self.status_message = Some(format!("Moved “{}” to {server}", note.title));
                    }

                    if self.pending_new_note_focus {
                        self.focus_title_next_frame = true;
                        self.pending_new_note_focus = false;
                    }
                    let server = self.collab.server_name().unwrap_or("the server");
                    if !self
                        .status_message
                        .as_deref()
                        .is_some_and(|m| m.starts_with("Moved"))
                    {
                        self.status_message = Some(format!("Created “{}” on {server}", note.title));
                    }
                    library_changed = true;
                }
                Incoming::Offline(reason) => {
                    if self.collab.is_active() {
                        self.status_message = Some(format!("Server: {reason}"));
                    }
                    self.collab_open = None;
                    self.awaiting_room_body = None;
                    // Nothing was moved, so the local note stays where it is.
                    self.pending_server_move = None;
                    library_changed = true;
                }
                Incoming::Body { id, text } => {
                    if self
                        .awaiting_room_body
                        .as_ref()
                        .is_some_and(|(pending, _)| *pending == id)
                    {
                        self.awaiting_room_body = None;
                    }
                    let previous = self
                        .notes
                        .iter()
                        .find(|n| n.id == id)
                        .map(|n| n.body.clone());
                    self.remote_bodies.insert(id.clone(), text.clone());
                    if let Some(note) = self.notes.iter_mut().find(|n| n.id == id) {
                        note.body = text.clone();
                        // The date is left alone on purpose: opening a note hands over the
                        // room's text, and that is not somebody editing it. The server stamps
                        // the note when its text really changes, and that date is what sorts
                        // the list.
                    }
                    if self.selected_id.as_deref() == Some(id.as_str()) {
                        // Somebody else's typing is not a local edit: no undo step, no save.
                        self.last_seen_body = Some((id.clone(), text.clone()));
                        // The caret belongs to the words around it, not to a character count, so
                        // it travels with them when somebody inserts a line above it.
                        if let Some(previous) = previous {
                            self.reveal_caret |= carry_caret_over(ctx, &id, &previous, &text);
                        }
                    }
                }
                Incoming::Presence(_) | Incoming::Peers(_) => {}
                Incoming::Attachment { name, bytes } => {
                    // The picture joins this vault, where the note's own relative address finds
                    // it and every later run of the app still has it.
                    match self.save_shared_picture(&name, &bytes) {
                        Ok(()) => {
                            self.requested_pictures.remove(&name);
                        }
                        Err(e) => self.status_message = Some(e),
                    }
                }
                Incoming::AttachmentMissing { name } => {
                    // The library does not have it (yet, or any more). It stays on the asked-for
                    // list so it is not demanded every frame; the connection is new or the note
                    // is opened again before it is asked for again.
                    if self.shared_uploads.remove(&name) {
                        self.status_message = Some(format!(
                            "Could not share {name} with the library; it stays on this computer"
                        ));
                    }
                }
                Incoming::AttachmentStored { name } => {
                    self.shared_uploads.remove(&name);
                }
                Incoming::AttachmentAdded { name } => {
                    // A picture that was missing a moment ago (its owner was still uploading
                    // it) is here now, so ask again.
                    if self.requested_pictures.contains(&name) {
                        self.collab.fetch_attachment(&name);
                    }
                }
            }
        }

        if let Some(fingerprint) = self.collab.take_fingerprint() {
            if self.server.fingerprint.is_none() {
                self.trust_prompt = Some((fingerprint, self.server.address()));
            }
        }

        if library_changed {
            let editing_title = ctx.memory(|m| m.has_focus(Id::new(TITLE_FIELD_ID)));
            self.apply_library(editing_title);
        }
    }

    /// Keeps the connection pointed at the selected note: shared notes are mirrored live, local
    /// notes leave the room.
    fn sync_open_note(&mut self) {
        let selected = self.selected_id.clone();
        let wanted = selected.filter(|id| self.is_remote(id));
        if wanted == self.collab_open {
            return;
        }
        if self.collab_open.is_some() {
            self.collab.close_note();
        }
        self.collab_open = wanted.clone();
        if let Some(id) = wanted {
            let body = self
                .notes
                .iter()
                .find(|n| n.id == id)
                .map(|n| n.body.clone())
                .unwrap_or_default();
            self.collab.open_note(&id, &body);
            // What the window holds for this note is only trusted again once the room has sent
            // its own text for it; until then the editor must not push that copy back.
            self.awaiting_room_body = Some((id, Instant::now()));
        } else {
            self.awaiting_room_body = None;
        }
    }

    /// True while the room's own text for `id` has not arrived yet. A request that goes
    /// unanswered stops holding the editor back after a moment, so a note that cannot be opened
    /// is still editable.
    fn room_body_pending(&self, id: &str) -> bool {
        self.awaiting_room_body
            .as_ref()
            .is_some_and(|(pending, since)| pending == id && since.elapsed() < ROOM_BODY_WAIT)
    }

    /// Asks the user to confirm a server's certificate the first time they connect to it.
    fn trust_prompt_window(&mut self, ctx: &egui::Context) {
        let Some((fingerprint, address)) = self.trust_prompt.clone() else {
            return;
        };
        let p = self.theme.palette_with(&self.colors, self.glow_strength);
        let short = crate::net::short_fingerprint(&fingerprint);
        let mut open = true;
        let mut accept = false;
        let mut reject = false;
        egui::Window::new("Trust this server?")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(380.0)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(format!("First connection to {address}"))
                        .strong()
                        .color(p.fg_primary),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "Check that this code matches the one the server printed while it installed:",
                    )
                    .size(11.5)
                    .color(p.fg_secondary),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new(short)
                        .monospace()
                        .size(19.0)
                        .strong()
                        .color(p.accent),
                );
                ui.add_space(6.0);
                ui.label(
                    RichText::new("EloraNotes remembers it, so nobody can pretend to be this server later.")
                        .size(11.0)
                        .color(p.fg_muted),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(format!("{}  Trust and connect", icon::CHECK)).clicked() {
                        accept = true;
                    }
                    if ui.button("Disconnect").clicked() {
                        reject = true;
                    }
                });
            });

        if accept {
            self.server.fingerprint = Some(fingerprint);
            self.server_form.fingerprint = self.server.fingerprint.clone();
            self.persist_settings();
            self.trust_prompt = None;
            self.status_message = Some("Server trusted".to_string());
        } else if reject || !open {
            self.trust_prompt = None;
            self.disconnect_server();
        }
    }

    /// Every folder currently used by a live note, alphabetically, either on this computer or on
    /// the server. The two lists are kept apart so a local note cannot be filed into a server
    /// folder (and the other way round) by accident.
    fn folders_for(&self, remote: bool) -> Vec<String> {
        let mut folders: BTreeSet<String> = BTreeSet::new();
        for note in self.notes.iter().filter(|n| !n.trashed) {
            if self.is_remote(&note.id) == remote {
                if let Some(folder) = &note.folder {
                    folders.insert(folder.clone());
                }
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

    /// Every `[[wiki link]]` in a live note that points at no note, with the titles of the notes
    /// that link to it. The same rule the link itself follows decides whether it is broken, so
    /// the report never lists something the link would have opened.
    fn collect_broken_links(&self) -> Vec<(String, Vec<String>)> {
        let live: Vec<&Note> = self.notes.iter().filter(|n| !n.trashed).collect();
        let mut broken: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for note in &live {
            for target in extract_wiki_links(&note.body) {
                let found = live.iter().any(|n| {
                    n.title.eq_ignore_ascii_case(&target)
                        || n.display_title().eq_ignore_ascii_case(&target)
                });
                if found {
                    continue;
                }
                let from = note.display_title();
                let entry = broken.entry(target).or_default();
                if !entry.iter().any(|title| title.eq_ignore_ascii_case(&from)) {
                    entry.push(from);
                }
            }
        }
        broken.into_iter().collect()
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
    /// The report of links that lead nowhere: one row per missing note, with where the links
    /// were found. Clicking one creates that note and opens it, which is the fix for most of them.
    fn broken_links_window(&mut self, ctx: &egui::Context) {
        if !self.broken_links_open {
            return;
        }
        let defaults = self.theme.palette();
        let mut open = true;
        let mut clicked: Option<String> = None;
        egui::Window::new("Links that lead nowhere")
            .id(Id::new("elora_broken_links"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(420.0)
            .show(ctx, |ui| {
                if self.broken_links.is_empty() {
                    ui.label(
                        RichText::new("Every [[link]] in your notes leads to a note that exists.")
                            .size(12.0)
                            .color(defaults.fg_secondary),
                    );
                    return;
                }
                ui.label(
                    RichText::new(format!(
                        "{} note{} linked to but not written yet. Click one to write it.",
                        self.broken_links.len(),
                        if self.broken_links.len() == 1 {
                            ""
                        } else {
                            "s"
                        }
                    ))
                    .size(11.5)
                    .color(defaults.fg_muted),
                );
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (target, from) in &self.broken_links {
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(format!("{} [[{target}]]", icon::LINK))
                                            .size(12.0)
                                            .color(defaults.accent),
                                    )
                                    .frame(false),
                                )
                                .on_hover_text("Create this note and open it")
                                .clicked()
                            {
                                clicked = Some(target.clone());
                            }
                            let mut list = String::from("linked from ");
                            list.push_str(&from.join(", "));
                            ui.label(RichText::new(list).size(10.5).color(defaults.fg_muted));
                            ui.add_space(3.0);
                        }
                    });
            });
        if let Some(target) = clicked {
            self.broken_links_open = false;
            self.open_or_create_wiki_link(&target);
        }
        if !open {
            self.broken_links_open = false;
        }
    }

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

        // With a shared note open, the new note belongs in that library too: it lands in the same
        // folder, with the same tag, and everyone connected sees it straight away.
        let shared = self
            .selected_id
            .as_deref()
            .is_some_and(|id| self.is_remote(id));
        if shared && self.collab.is_connected() {
            self.collab.create_note(
                &note.title,
                note.folder.as_deref(),
                &note.explicit_tags,
                &note.body,
                note.pinned,
                note.icon.as_deref(),
            );
            self.pending_new_note_focus = true;
            if self.view_mode == EditorViewMode::Preview {
                self.view_mode = EditorViewMode::Edit;
            }
            let server = self.collab.server_name().unwrap_or("the server");
            self.status_message = Some(format!("Adding “{}” to {server}…", note.title));
            return;
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

        let body = "Linked from another note.\n\n#linked";
        // A link made from a shared note creates the target in that library as well.
        let shared_folder = self
            .selected_id
            .as_ref()
            .filter(|id| self.is_remote(id))
            .and_then(|id| self.notes.iter().find(|n| &n.id == id))
            .and_then(|n| n.folder.clone());
        if self.collab.is_connected()
            && self
                .selected_id
                .as_deref()
                .is_some_and(|id| self.is_remote(id))
        {
            self.collab.create_note(
                target_trimmed,
                shared_folder.as_deref(),
                &["linked".to_string()],
                body,
                false,
                None,
            );
            self.pending_new_note_focus = false;
            let server = self.collab.server_name().unwrap_or("the server");
            self.status_message = Some(format!("Adding [[{target_trimmed}]] to {server}…"));
            return;
        }

        let note = Note::new(target_trimmed, body, self.storage.root_dir());
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
        if let Some(idx) = self.notes.iter().position(|n| n.id == id) {
            self.notes[idx].trashed = true;
            self.notes[idx].pinned = false;
            self.notes[idx].updated_at = Utc::now();
            let _ = self.persist_note(idx);
            self.resort();
            self.select_first_visible();
            self.status_message = Some("Moved note to Trash".to_string());
        }
    }

    fn restore_from_trash(&mut self, id: &str) {
        if let Some(idx) = self.notes.iter().position(|n| n.id == id) {
            self.notes[idx].trashed = false;
            self.notes[idx].updated_at = Utc::now();
            let _ = self.persist_note(idx);
            self.resort();
            self.status_message = Some("Restored note from Trash".to_string());
        }
    }

    fn permanently_delete_by_id(&mut self, id: &str) {
        if let Some(pos) = self.notes.iter().position(|n| n.id == id) {
            if self.is_remote(id) {
                self.collab.delete_note(id, true);
                self.remote_bodies.remove(id);
                self.remote_ids.remove(id);
                self.notes.remove(pos);
                if self.collab_open.as_deref() == Some(id) {
                    self.collab.close_note();
                    self.collab_open = None;
                    self.awaiting_room_body = None;
                }
            } else {
                let removed = self.notes.remove(pos);
                let _ = self.storage.delete_note(&removed);
            }
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
            if let Some(idx) = self.notes.iter().position(|n| n.id == id) {
                self.notes[idx].pinned = !self.notes[idx].pinned;
                self.notes[idx].updated_at = Utc::now();
                // Shared notes are pinned on the server, not on this disk.
                let _ = self.persist_note(idx);
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
        // A shared note has no file on this disk, so it goes back to the server instead.
        if let Err(e) = self.persist_note(note_idx) {
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
            PaletteCommand::BrokenLinks => {
                self.broken_links = self.collect_broken_links();
                self.broken_links_open = true;
            }
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
        let mut open_find = false;
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
            // Ctrl+F looks in the note in front of you, the way an editor does; the vault search
            // keeps the shifted version of the shortcut.
            if i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::F) {
                focus_search = true;
            } else if i.consume_key(egui::Modifiers::COMMAND, Key::F) {
                open_find = true;
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
            if self.find_open {
                self.find_open = false;
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
        if open_find {
            // A note has to be open for there to be anything to look in.
            if self.selected_id.is_some() {
                self.focus_mode = false;
                // The matches are drawn by the editor, so a preview-only window opens the split
                // view to give the find bar something to sit above.
                if self.view_mode == EditorViewMode::Preview {
                    self.view_mode = EditorViewMode::Split;
                }
                self.find_open = true;
                self.focus_find_next_frame = true;
                self.find_pending_jump = true;
            } else {
                self.focus_search_next_frame = true;
            }
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
                icon::BRUSH,
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
                "Show Links That Lead Nowhere",
                "broken wiki link missing note",
                PaletteCommand::BrokenLinks,
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

    /// Draws the sidebar and returns the rect the footer took at the bottom of it, so a test can
    /// check that the icons stay inside the panel.
    fn render_sidebar(&mut self, ui: &mut Ui, p: &Palette) -> Rect {
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
                    .on_hover_text(match self.collab.server_name() {
                        Some(server)
                            if self
                                .selected_id
                                .as_deref()
                                .is_some_and(|id| self.is_remote(id)) =>
                        {
                            format!("Create a note in the shared library on {server} (Ctrl+N)")
                        }
                        _ => "Create new note (Ctrl+N)".to_string(),
                    });
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
                        .on_hover_text("Search notes or #tags (Ctrl+Shift+F)");
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

        // The tag filter and the sort chooser share one row: the filter takes what is left after
        // the sort button, and the button stays put whether or not there are tags to filter.
        let all_tags = self.all_tags_with_counts();
        if self.sidebar_section != SidebarSection::Trash {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(5.0, 4.0);
                // What the sort button needs: its glyph and padding, the gap before it, and the
                // arrow the combo box puts on the right of its own field.
                const SORT_RESERVE: f32 = 52.0;
                let room = (ui.available_width() - SORT_RESERVE).max(80.0);
                if !all_tags.is_empty() {
                    ui.scope(|ui| {
                        ui.set_max_width(room);
                        tag_filter_menu(ui, p, &all_tags, &mut self.active_tag);
                    });
                } else {
                    ui.add_space((ui.available_width() - SORT_RESERVE).max(0.0));
                }
                let mut chosen = self.sort;
                sort_menu(ui, p, &mut chosen);
                if chosen != self.sort {
                    self.sort = chosen;
                    self.persist_settings();
                }
            });
        }

        ui.add_space(6.0);
        ui.separator();
        ui.add_space(4.0);

        // The footer is drawn first, from the bottom of the panel upwards, so it ends exactly at
        // the panel's edge and the list above it gets exactly what is left. A height reserved in
        // advance for it was a few points short, which pushed the icons past the bottom of the
        // panel and cut them off; the same shortfall let the list run underneath them.
        let content = ui.max_rect();
        let mut footer_ui = ui.new_child(
            UiBuilder::new()
                .id_salt("elora_sidebar_footer")
                .max_rect(content)
                .layout(Layout::bottom_up(Align::Min)),
        );
        self.sidebar_footer(&mut footer_ui, p);
        // The scroll area clips its content to the height it is given, so the list's own edge is
        // the footer's top: the cards can never be drawn over the icons.
        let list_height = (footer_ui.min_rect().top() - ui.next_widget_position().y).max(80.0);
        self.render_sidebar_notes(ui, p, list_height);
        footer_ui.min_rect()
    }

    /// The row of icons along the bottom of the sidebar, drawn upwards from the panel's edge.
    fn sidebar_footer(&mut self, ui: &mut Ui, p: &Palette) {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(6.0, 4.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // Pull the row off the panel edge a little, so the footer icons line up with
                // the note cards above them instead of hugging the divider.
                ui.add_space(FOOTER_ICON_INSET);
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
                } // Ko-fi's cup, beside Settings: leaving a tip is the one thing the app can
                  // offer besides its own features, so it has a place in the footer.
                if kofi_button(ui, p, self.kofi.as_ref())
                    .on_hover_text(format!("Support EloraNotes on Ko-fi\n{KOFI_URL}"))
                    .clicked()
                {
                    if let Err(e) = open_in_browser(KOFI_URL) {
                        self.status_message = Some(e);
                    }
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
        // Bottom-up, so this line sits above the icons and keeps them off the list.
        ui.add_space(3.0);
        ui.separator();
        ui.add_space(2.0);
    }

    /// The find bar: a field, how many matches the note holds, and the two arrows that step
    /// through them. The editor paints every match, so the count always matches what is seen.
    fn find_bar(&mut self, ui: &mut Ui, p: &Palette, note_idx: usize) {
        let body = &self.notes[note_idx].body;
        let hits = find_matches(body, &self.find_query);
        if self.find_current >= hits.len() {
            self.find_current = hits.len().saturating_sub(1);
        }
        let field_id = Id::new("elora_find_input");

        Frame::new()
            .fill(p.bg_elevated)
            .stroke(Stroke::new(1.0_f32, p.border))
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(9, 4))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(6.0, 3.0);
                    ui.label(RichText::new(icon::SEARCH).size(13.0).color(p.fg_muted));
                    let field = ui.add(
                        TextEdit::singleline(&mut self.find_query)
                            .id(field_id)
                            .hint_text("Find in this note…")
                            .frame(false)
                            .desired_width(180.0),
                    );
                    if self.focus_find_next_frame {
                        field.request_focus();
                        self.focus_find_next_frame = false;
                    }
                    if field.changed() {
                        self.find_current = 0;
                        self.find_pending_jump = true;
                    }
                    // Enter steps to the next match and shift+Enter back, then the field takes
                    // the caret again so the next press works too.
                    if field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        let backwards = ui.input(|i| i.modifiers.shift);
                        self.step_find(backwards, hits.len());
                        self.focus_find_next_frame = true;
                    }

                    let count = if self.find_query.is_empty() {
                        String::new()
                    } else if hits.is_empty() {
                        "No matches".to_string()
                    } else {
                        format!("{} of {}", self.find_current + 1, hits.len())
                    };
                    ui.label(RichText::new(count).size(10.5).color(p.fg_muted));

                    if !hits.is_empty() {
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(icon::CHEVRON_UP)
                                        .size(12.5)
                                        .color(p.fg_secondary),
                                )
                                .frame(false),
                            )
                            .on_hover_text("Previous match (Shift+Enter)")
                            .clicked()
                        {
                            self.step_find(true, hits.len());
                        }
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(icon::CHEVRON_DOWN)
                                        .size(12.5)
                                        .color(p.fg_secondary),
                                )
                                .frame(false),
                            )
                            .on_hover_text("Next match (Enter)")
                            .clicked()
                        {
                            self.step_find(false, hits.len());
                        }
                    }
                    if ui
                        .add(
                            egui::Button::new(RichText::new(icon::X).size(12.0).color(p.fg_muted))
                                .frame(false),
                        )
                        .on_hover_text("Close find (Esc)")
                        .clicked()
                    {
                        self.find_open = false;
                    }
                });
            });
    }

    /// Moves to the next or previous match, wrapping around at either end.
    fn step_find(&mut self, backwards: bool, total: usize) {
        if total == 0 {
            return;
        }
        self.find_current = step_match(self.find_current, total, backwards);
        self.find_pending_jump = true;
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

        let mut pinned_indices: Vec<usize> = filtered
            .iter()
            .copied()
            .filter(|&i| self.notes[i].pinned && !self.notes[i].trashed)
            .collect();
        self.sort_group(&mut pinned_indices);

        // Unpinned live notes are grouped by folder; trashed notes are listed flat. The
        // server's notes get their own group, titled with the server's name.
        let mut folder_groups: Vec<(String, Vec<usize>)> = Vec::new();
        let mut remote_indices: Vec<usize> = Vec::new();
        let mut unfiled: Vec<usize> = Vec::new();
        let mut trashed_indices: Vec<usize> = Vec::new();
        let server_label = self.collab.server_name().unwrap_or("Shared").to_string();
        for &i in &filtered {
            let note = &self.notes[i];
            if self.remote_ids.contains(&note.id) {
                remote_indices.push(i);
            } else if note.trashed {
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
        // Each group follows the sort the sidebar is set to; the groups themselves keep their
        // own order, so folders stay alphabetical and pinned notes stay on top.
        self.sort_group(&mut unfiled);
        self.sort_group(&mut trashed_indices);
        for (_, list) in &mut folder_groups {
            self.sort_group(list);
        }

        // Filtering a folder should reveal its matches instead of hiding them.
        let folders_open = !self.search_query.trim().is_empty() || self.active_tag.is_some();
        let has_folders = !folder_groups.is_empty();

        let mut picked = CardSelection::default();
        let mut toggled_folder: Option<String> = None;

        // egui puts a floating scroll bar against the right edge of its scroll area, which
        // inside the panel's margin is the very edge the divider is drawn on. Inset the bar by
        // the panel margin so it reads as a scrollbar in the note column, not a line on the seam.
        ui.scope(|ui| {
            note_list_scroll(&mut ui.spacing_mut().scroll);
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
                            Some(&self.collab),
                            &mut picked,
                            0,
                            self.show_card_previews,
                            self.roomy_cards,
                        );
                        ui.add_space(4.0);
                    }

                    if !remote_indices.is_empty() {
                        let key = format!("server:{server_label}");
                        let collapsed = !folders_open && self.collapsed_folders.contains(&key);
                        // The shared library is marked by its cloud alone; a folder glyph in
                        // front of it would read as a folder on this computer.
                        if folder_header_with(
                            ui,
                            icon::CLOUD,
                            &server_label,
                            remote_indices.len(),
                            collapsed,
                            p,
                        )
                        .clicked()
                        {
                            toggled_folder = Some(key);
                        }
                        if !collapsed {
                            ui.add_space(2.0);
                            // The shared library keeps folders of its own, laid out the same
                            // way as local ones.
                            let (mut remote_unfiled, mut remote_folders) =
                                split_by_folder(&self.notes, &remote_indices);
                            self.sort_group(&mut remote_unfiled);
                            for (_, list) in &mut remote_folders {
                                self.sort_group(list);
                            }
                            if !remote_unfiled.is_empty() {
                                render_note_rows(
                                    ui,
                                    &self.notes,
                                    &remote_unfiled,
                                    self.selected_id.as_deref(),
                                    p,
                                    &self.search_query,
                                    Some(&self.collab),
                                    &mut picked,
                                    SIDEBAR_INDENT_STEP,
                                    self.show_card_previews,
                                    self.roomy_cards,
                                );
                            }
                            for (name, indices) in &remote_folders {
                                let folder_key = format!("server:{server_label}/{name}");
                                let folder_collapsed =
                                    !folders_open && self.collapsed_folders.contains(&folder_key);
                                let header = indented(ui, SIDEBAR_INDENT_STEP, |ui| {
                                    folder_header(ui, name, indices.len(), folder_collapsed, p)
                                });
                                if header.clicked() {
                                    toggled_folder = Some(folder_key);
                                }
                                if !folder_collapsed {
                                    render_note_rows(
                                        ui,
                                        &self.notes,
                                        indices,
                                        self.selected_id.as_deref(),
                                        p,
                                        &self.search_query,
                                        Some(&self.collab),
                                        &mut picked,
                                        SIDEBAR_INDENT_STEP * 2,
                                        self.show_card_previews,
                                        self.roomy_cards,
                                    );
                                }
                            }
                            ui.add_space(3.0);
                        }
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
                                Some(&self.collab),
                                &mut picked,
                                SIDEBAR_INDENT_STEP,
                                self.show_card_previews,
                                self.roomy_cards,
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
                            Some(&self.collab),
                            &mut picked,
                            0,
                            self.show_card_previews,
                            self.roomy_cards,
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
                            Some(&self.collab),
                            &mut picked,
                            0,
                            self.show_card_previews,
                            self.roomy_cards,
                        );
                    }
                });
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

    /// Orders one group's notes the way the sidebar is set to. A search keeps its relevance
    /// order: the best match first is what somebody searching is looking for, not the newest.
    fn sort_group(&self, indices: &mut [usize]) {
        if !self.search_query.trim().is_empty() {
            return;
        }
        let notes = &self.notes;
        match self.sort {
            NoteSort::Updated => indices.sort_by(|&a, &b| {
                notes[b]
                    .updated_at
                    .cmp(&notes[a].updated_at)
                    .then_with(|| notes[a].title.cmp(&notes[b].title))
            }),
            NoteSort::Created => indices.sort_by(|&a, &b| {
                notes[b]
                    .created_at
                    .cmp(&notes[a].created_at)
                    .then_with(|| notes[a].title.cmp(&notes[b].title))
            }),
            NoteSort::Title => indices.sort_by(|&a, &b| {
                notes[a]
                    .title
                    .to_lowercase()
                    .cmp(&notes[b].title.to_lowercase())
            }),
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

        self.last_seen_body = Some((note_id.clone(), self.notes[note_idx].body.clone()));

        // Tab steps the caret's line in by one level and Shift+Tab takes a level away; a
        // selection moves every line it touches. The key is taken here, before the field reads
        // its events, so the editor never leaves a literal tab in the Markdown (the preview
        // would show it as an uneven gap) and the focus stays in the note.
        let mut indented = false;
        if ui.memory(|m| m.has_focus(editor_id)) {
            let dedent = ui.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, Key::Tab));
            let indent =
                !dedent && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Tab));
            if indent || dedent {
                let body = self.notes[note_idx].body.clone();
                let mut state = TextEdit::load_state(ui.ctx(), editor_id).unwrap_or_default();
                // The live cursor is the one in the field's state, not the one written down at
                // the end of the last frame: a character typed since then has moved it on.
                let (start, end, forward) = caret_range(&state, body.chars().count());
                let (new_body, new_start, new_end) = indent_selection(&body, start, end, dedent);
                if new_body != body {
                    let range = if forward {
                        CCursorRange::two(CCursor::new(new_start), CCursor::new(new_end))
                    } else {
                        CCursorRange::two(CCursor::new(new_end), CCursor::new(new_start))
                    };
                    state.cursor.set_char_range(Some(range));
                    state.store(ui.ctx(), editor_id);
                    self.notes[note_idx].body = new_body;
                    self.notes[note_idx].updated_at = Utc::now();
                    self.last_editor_selection = Some((new_start, new_end));
                    self.last_editor_caret = Some(if forward { new_end } else { new_start });
                    *needs_save = true;
                    indented = true;
                }
            }
        }

        // Enter starts the next line the way the current one is laid out: the same indentation,
        // and the same list marker so a list carries on. Shift+Enter stays inside the item and
        // lines up under its words instead of starting a new one. The key is taken before the
        // field reads its events, so this is the only line break that gets inserted.
        if ui.memory(|m| m.has_focus(editor_id)) {
            // Shift+Enter is asked for first, because the plain check matches it as well.
            let soft = ui.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, Key::Enter));
            let hard = !soft && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Enter));
            if soft || hard {
                let body = self.notes[note_idx].body.clone();
                let mut state = TextEdit::load_state(ui.ctx(), editor_id).unwrap_or_default();
                let (start, end, _) = caret_range(&state, body.chars().count());
                let (new_body, caret) = if soft {
                    newline_continuing_item(&body, start, end)
                } else {
                    newline_with_prefix(&body, start, end)
                };
                state
                    .cursor
                    .set_char_range(Some(CCursorRange::one(CCursor::new(caret))));
                state.store(ui.ctx(), editor_id);
                self.notes[note_idx].body = new_body;
                self.notes[note_idx].updated_at = Utc::now();
                self.last_editor_selection = Some((caret, caret));
                self.last_editor_caret = Some(caret);
                *needs_save = true;
            }
        }

        // Ctrl+V with a picture on the clipboard puts the picture into the note, the way a word
        // processor pastes a screenshot. The key is not consumed: when the clipboard holds text
        // instead, the editor pastes it exactly as before.
        if ui.memory(|m| m.has_focus(editor_id))
            && ui.input(|i| i.modifiers.command && i.key_pressed(Key::V))
        {
            if let Some((bytes, name)) = clipboard_picture() {
                if let Err(e) = self.insert_picture(note_idx, &bytes, &name, "png") {
                    self.status_message = Some(e);
                }
            }
        }

        // Who else has this note open, with the colour of their caret.
        let peers: Vec<crate::collab::RemotePeer> = if self.is_remote(&note_id) {
            self.collab.peers().to_vec()
        } else {
            Vec::new()
        };
        if !peers.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = Vec2::new(5.0, 2.0);
                ui.label(
                    RichText::new(format!("{} Editing with", icon::USERS))
                        .size(11.0)
                        .color(p.fg_muted),
                );
                for peer in &peers {
                    let [r, g, b] = peer.color;
                    // The same brush the sidebar cards use. The filled circle this used to
                    // draw has no glyph in the bundled fonts, so it showed as a
                    // missing-character box beside every name.
                    ui.label(
                        RichText::new(format!("{} {}", icon::BRUSH, peer.name))
                            .size(11.0)
                            .color(Color32::from_rgb(r, g, b)),
                    );
                }
            });
            ui.add_space(2.0);
        }

        let is_dark = self.theme.is_dark();
        let p_copy = *p;
        let hl_rgb = self.highlight_rgb;

        let mut layouter =
            move |ui: &Ui, text: &str, wrap_width: f32| -> std::sync::Arc<egui::Galley> {
                let job = build_editor_layout_job(text, wrap_width, &p_copy, is_dark, hl_rgb);
                ui.fonts(|f| f.layout_job(job))
            };

        // Stepping to a match moves the caret there before the field is drawn, so the editor
        // knows where it is when it decides what to reveal.
        if self.find_open && self.find_pending_jump {
            self.find_pending_jump = false;
            let hits = find_matches(&self.notes[note_idx].body, &self.find_query);
            if let Some(&byte) = hits.get(self.find_current) {
                let char_index = self.notes[note_idx].body[..byte].chars().count();
                // A note that has not been drawn yet has no state of its own; a fresh one is
                // made so the caret is already at the match when the field is first shown.
                let mut state = TextEdit::load_state(ui.ctx(), editor_id).unwrap_or_default();
                state
                    .cursor
                    .set_char_range(Some(CCursorRange::one(CCursor::new(char_index))));
                state.store(ui.ctx(), editor_id);
            }
        }

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
            // Tab indents inside the note instead of moving to the next widget.
            .lock_focus(true)
            .show(ui);

        // The first Tab after the note is clicked is spent before the field has told egui to
        // keep the key for itself, so egui may have moved on to the next widget while the line
        // was stepping in. The caret stays in the note: that is the whole point of the key.
        if indented {
            output.response.request_focus();
        }

        if let Some(cursor_range) = output.cursor_range {
            let a = cursor_range.primary.ccursor.index;
            let b = cursor_range.secondary.ccursor.index;
            self.last_editor_selection = Some((a.min(b), a.max(b)));
            self.last_editor_caret = Some(a);
        }

        // A picture just inserted sits behind the caret, so the caret moves past it instead of
        // being left in front of the picture it was dropped on.
        if let Some((target, caret)) = self.caret_after_insert.clone() {
            if target == note_id {
                self.caret_after_insert = None;
                if let Some(mut state) = TextEdit::load_state(ui.ctx(), editor_id) {
                    state
                        .cursor
                        .set_char_range(Some(CCursorRange::one(CCursor::new(caret))));
                    state.store(ui.ctx(), editor_id);
                }
                self.last_editor_selection = Some((caret, caret));
                self.last_editor_caret = Some(caret);
            }
        }

        // Where the caret is on screen. egui scrolls to the caret when it moves it itself, but a
        // remote edit moves it from the connection, so if those words were pushed out of view
        // while the user was working in them, the editor brings them back this frame.
        if let Some(rect) = output.cursor_range.map(|range| {
            let caret = output.galley.pos_from_ccursor(range.primary.ccursor);
            Rect::from_min_size(
                output.galley_pos + caret.left_top().to_vec2(),
                caret.size().max(Vec2::new(1.0, self.font_size)),
            )
        }) {
            if self.reveal_caret
                && self.caret_in_view
                && ui.memory(|m| m.has_focus(editor_id))
                && !ui.clip_rect().intersects(rect)
            {
                ui.scroll_to_rect(rect.expand(6.0), None);
            }
            self.caret_in_view = ui.clip_rect().contains_rect(rect);
        } else {
            self.caret_in_view = false;
        }
        self.reveal_caret = false;

        // Find in note: every match gets a wash, and the one being stepped through a stronger
        // one. The editor has no focus of its own while somebody types in the find field, so the
        // matches are painted here rather than left to a text selection.
        if self.find_open && !self.find_query.is_empty() {
            let hits = find_matches(&self.notes[note_idx].body, &self.find_query);
            if !hits.is_empty() {
                let current = self.find_current.min(hits.len() - 1);
                let chars = self.find_query.chars().count();
                let painter = ui.painter();
                let mut current_rect = Rect::NOTHING;
                for (index, &byte) in hits.iter().enumerate() {
                    let start = self.notes[note_idx].body[..byte].chars().count();
                    let strong = index == current;
                    let alpha = if strong { 110 } else { 45 };
                    let rect = paint_find_match(
                        painter,
                        &output.galley,
                        output.galley_pos,
                        start,
                        chars,
                        Color32::from_rgba_unmultiplied(
                            p.accent.r(),
                            p.accent.g(),
                            p.accent.b(),
                            alpha,
                        ),
                    );
                    if strong {
                        current_rect = rect;
                    }
                }
                // Stepping past the edge of the view brings that match into it.
                if current_rect != Rect::NOTHING && !ui.clip_rect().contains_rect(current_rect) {
                    ui.scroll_to_rect(current_rect.expand(40.0), Some(Align::Center));
                }
            }
        }

        // Other people's carets, drawn where their cursor sits in the text.
        if !peers.is_empty() {
            let painter = ui.painter();
            let body = &self.notes[note_idx].body;
            for peer in &peers {
                let Some(byte) = peer.cursor else {
                    continue;
                };
                let byte = byte.min(body.len());
                let byte = (0..=byte)
                    .rev()
                    .find(|i| body.is_char_boundary(*i))
                    .unwrap_or(0);
                let char_index = body[..byte].chars().count();
                let caret = output
                    .galley
                    .pos_from_ccursor(egui::text::CCursor::new(char_index));
                let top = output.galley_pos + caret.left_top().to_vec2();
                let height = caret.height().max(self.font_size + 2.0);
                let [r, g, b] = peer.color;
                let color = Color32::from_rgb(r, g, b);
                painter.vline(top.x, top.y..=top.y + height, Stroke::new(1.8_f32, color));
                // The name sits in a filled chip rather than as bare text: it very often lands
                // on the line above, where glyphs underneath would otherwise show through it.
                // An opaque pill drawn after the text stays legible there, and when the caret is
                // too close to the top of the text to fit a chip above it, it moves below.
                let galley = ui.fonts(|f| {
                    f.layout_no_wrap(
                        peer.name.clone(),
                        FontId::proportional(10.0),
                        readable_on(color),
                    )
                });
                let pad = Vec2::new(4.0, 1.5);
                let size = galley.size() + pad * 2.0;
                let chip = caret_chip(top, height, size, ui.clip_rect());
                if chip.is_positive() {
                    painter.rect_filled(chip, CornerRadius::same(3), color);
                    painter.galley(chip.min + pad, galley, readable_on(color));
                }
            }
        }

        if output.response.changed() {
            self.notes[note_idx].updated_at = Utc::now();
            *needs_save = true;
        }

        output.response.context_menu(|ui| {
            self.render_editor_context_menu(ui, p, queued_op);
        });
    }

    /// The panel behind the header's lock button: open the note to the whole server, close it to
    /// everybody but its owner, or name the people who may read it. Only the owner may change it;
    /// everybody else sees which note they were let into and by whom.
    fn render_share_picker(&mut self, ctx: &egui::Context, p: &Palette) {
        if !self.share_picker_open {
            return;
        }
        let Some(note_id) = self.share_picker_note_id.clone() else {
            self.share_picker_open = false;
            return;
        };
        // The library is the only place that knows who may see a note; a note that is not in it
        // (deleted, or moved off the server) has nothing to show.
        let Some(meta) = self
            .collab
            .notes()
            .iter()
            .find(|meta| meta.id == note_id)
            .cloned()
        else {
            self.share_picker_open = false;
            self.share_picker_note_id = None;
            return;
        };
        let me = self.server_display_name();
        let owner = meta.owner.clone().unwrap_or_else(|| me.clone());
        // Whether this note is yours is the key's business, never the name's: renaming yourself
        // to somebody else's word leaves you with your own key, and their notes stay theirs.
        let my_key = crate::people::public_key(&self.identifier);
        let mine = meta
            .owner_key
            .as_deref()
            .is_some_and(|owner_key| owner_key.eq_ignore_ascii_case(&my_key));
        let server = self.collab.server_name().map(str::to_string);
        let mut open = true;
        let mut change: Option<(bool, Vec<String>)> = None;
        let mut remove: Option<String> = None;

        egui::Window::new("Who can see this note")
            .id(Id::new("elora_share_picker"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_TOP, Vec2::new(0.0, 68.0))
            .frame(
                Frame::new()
                    .fill(p.bg_sidebar)
                    .stroke(Stroke::new(1.5_f32, p.accent))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::symmetric(14, 12)),
            )
            .show(ctx, |ui| {
                ui.set_width(330.0);
                ui.label(
                    RichText::new(meta.title.clone())
                        .size(13.5)
                        .strong()
                        .color(p.fg_primary),
                );
                ui.label(
                    RichText::new(if mine {
                        "You decide who can read this note.".to_string()
                    } else {
                        format!("{owner} decides who can read this note.")
                    })
                    .size(11.0)
                    .color(p.fg_muted),
                );
                if meta.private && meta.owner_key.is_none() {
                    ui.label(
                        RichText::new(
                            "It was locked before notes carried keys, so nobody can prove they own \
                             it: whoever runs the server has to give it one with \
                             `elora-server people adopt --write`.",
                        )
                        .size(10.0)
                        .color(p.danger),
                    );
                }
                ui.add_space(6.0);

                let mut private = meta.private;
                ui.add_enabled_ui(mine, |ui| {
                    if ui
                        .radio_value(
                            &mut private,
                            false,
                            format!(
                                "{} Everyone{}",
                                icon::WORLD,
                                server.map(|s| format!(" on {s}")).unwrap_or_default()
                            ),
                        )
                        .clicked()
                    {
                        change = Some((false, Vec::new()));
                    }
                    let only = if meta.shared_with.is_empty() {
                        format!("{} Only me", icon::LOCK)
                    } else {
                        format!("{} Only these people", icon::LOCK)
                    };
                    if ui.radio_value(&mut private, true, only).clicked() {
                        change = Some((true, meta.shared_with.clone()));
                    }
                });

                if meta.private {
                    ui.add_space(4.0);
                    if meta.shared_with.is_empty() {
                        ui.label(
                            RichText::new("Nobody else can open it.")
                                .size(11.0)
                                .color(p.fg_muted),
                        );
                    } else {
                        ui.horizontal_wrapped(|ui| {
                            for name in &meta.shared_with {
                                let chip = format!("{name} {}", icon::X);
                                if ui
                                    .add_enabled(
                                        mine,
                                        egui::Button::new(RichText::new(chip).size(11.5)),
                                    )
                                    .on_hover_text("Take this person off the note")
                                    .clicked()
                                {
                                    remove = Some(name.clone());
                                }
                            }
                        });
                    }
                    ui.horizontal(|ui| {
                        let field_id = Id::new("elora_share_name_input");
                        // The hint says what the field is for, so it goes away as soon as the
                        // caret is in it instead of sitting under the name being typed.
                        let hint = match ui.memory(|m| m.has_focus(field_id)) {
                            true => "",
                            false => "Their name, e.g. Goku",
                        };
                        let field = ui.add_enabled(
                            mine,
                            TextEdit::singleline(&mut self.share_input)
                                .id(field_id)
                                .hint_text(hint)
                                .desired_width(200.0),
                        );
                        let entered = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                        let added = ui
                            .add_enabled(
                                mine,
                                egui::Button::new(format!("{} Add", icon::USER_PLUS)),
                            )
                            .clicked();
                        if mine && (entered || added) {
                            let name = crate::net::normalize_identifier(&self.share_input);
                            if !name.is_empty()
                                && !meta
                                    .shared_with
                                    .iter()
                                    .any(|kept| kept.eq_ignore_ascii_case(&name))
                            {
                                let mut list = meta.shared_with.clone();
                                list.push(name);
                                change = Some((true, list));
                            }
                            self.share_input.clear();
                        }
                    });
                    if let Some(name) = remove.take() {
                        let kept = meta
                            .shared_with
                            .iter()
                            .filter(|other| **other != name)
                            .cloned()
                            .collect();
                        change = Some((true, kept));
                    }
                }

                ui.add_space(8.0);
                ui.label(
                    RichText::new(format!("You are “{me}”."))
                        .size(10.5)
                        .color(p.fg_secondary),
                );
                ui.label(
                    RichText::new(
                        "Add the name somebody uses as you see it next to their caret; the server \
                         checks it is really them. The server password is still what decides who \
                         may connect.",
                    )
                    .size(10.0)
                    .color(p.fg_muted),
                );
            });

        if let Some((private, shared_with)) = change {
            self.collab.set_share(&note_id, private, shared_with);
        }
        if !open {
            self.share_picker_open = false;
            self.share_picker_note_id = None;
        }
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

    /// `content_width` is the note column's text width and `right_extra` the empty strip between
    /// it and the window's right edge.
    fn render_editor(&mut self, ui: &mut Ui, p: &Palette, content_width: f32, right_extra: f32) {
        // Everything the editor draws keeps the column's width; only the scroll areas below are
        // allowed to reach the window edge.
        ui.set_max_width(content_width);
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
        let mut folder_choice: Option<FolderChoice> = None;

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

            // Folder chooser: file this note under a folder, start a new one, or move the note
            // into the shared library.
            let current_folder = self.notes[note_idx].folder.clone();
            let on_server = self.is_remote(&selected_id);
            let folders = self.folders_for(on_server);
            let server = if !on_server && self.collab.is_connected() {
                self.collab.server_name().map(str::to_string)
            } else {
                None
            };
            let extras = MenuExtras {
                server: server.as_deref(),
                on_server,
            };
            if let Some(choice) = folder_menu_ui(
                ui,
                p,
                current_folder.as_deref(),
                &folders,
                &mut self.new_folder_input,
                if compact { 104.0 } else { 148.0 },
                &extras,
            ) {
                folder_choice = Some(choice);
            }

            // Who may read this note. Only the server's notes can be closed to people: a note
            // on this computer has no server to be private on.
            if on_server {
                if !compact {
                    ui.label(RichText::new("·").color(p.fg_muted));
                }
                let my_key = crate::people::public_key(&self.identifier);
                let owner = self.notes[note_idx]
                    .owner
                    .clone()
                    .unwrap_or_else(|| self.server_display_name());
                let mine = self.notes[note_idx]
                    .owner_key
                    .as_deref()
                    .is_some_and(|key| key.eq_ignore_ascii_case(&my_key));
                let (glyph, summary) = share_look(
                    self.notes[note_idx].private,
                    &self.notes[note_idx].shared_with,
                    &owner,
                    mine,
                    self.collab.server_name(),
                );
                let showing = self.share_picker_open
                    && self.share_picker_note_id.as_deref() == Some(selected_id.as_str());
                let tint = if self.notes[note_idx].private {
                    p.accent
                } else {
                    p.fg_secondary
                };
                if ui
                    .add(
                        egui::Button::new(RichText::new(glyph).size(13.0).color(tint))
                            .frame(showing),
                    )
                    .on_hover_text(summary)
                    .clicked()
                {
                    self.share_picker_note_id = Some(self.notes[note_idx].id.clone());
                    self.share_picker_open = true;
                    self.share_input.clear();
                }
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
                    (EditorViewMode::Edit, icon::BRUSH, "Edit", "Markdown Editor"),
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
            match choice {
                FolderChoice::Unfiled => {
                    self.notes[note_idx].folder = None;
                    self.notes[note_idx].updated_at = Utc::now();
                    needs_save = true;
                    self.status_message = Some("Removed from folder".to_string());
                }
                FolderChoice::Folder(name) => {
                    self.notes[note_idx].folder = Some(name.clone());
                    self.notes[note_idx].updated_at = Utc::now();
                    needs_save = true;
                    // Filing a note into a folded folder should reveal it — including a folder
                    // of the shared library, which is folded under its own key.
                    self.collapsed_folders.remove(&name);
                    if self.is_remote(&self.notes[note_idx].id) {
                        if let Some(server) = self.collab.server_name().map(str::to_string) {
                            self.collapsed_folders
                                .remove(&format!("server:{server}/{name}"));
                        }
                    }
                    self.status_message = Some(format!("Filed under {name}"));
                }
                FolderChoice::ToServer => self.move_note_to_server(note_idx),
                FolderChoice::ToComputer => self.copy_shared_note_to_computer(note_idx),
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
        let title_id = Id::new(TITLE_FIELD_ID);
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

        // Find in note, above the editor it belongs to.
        if self.find_open && self.view_mode != EditorViewMode::Preview {
            self.find_bar(ui, p, note_idx);
            ui.add_space(2.0);
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

                // A picture from this computer. Dropping one on the window or pasting one works
                // too, but the button is where people look for it.
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new(icon::PHOTO).size(13.5).color(p.fg_secondary),
                        )
                        .fill(p.bg_elevated)
                        .corner_radius(CornerRadius::same(5)),
                    )
                    .on_hover_text("Add a picture to this note (or drop one on the window)")
                    .clicked()
                {
                    self.pick_picture(ui.ctx(), note_idx);
                }

                // The colour controls are their own fixed-size child layout, so this wrapping
                // row moves the whole group down as one piece. Letting them wrap one by one
                // left the glow swatch stranded under the markdown buttons at narrow widths.
                ui.add_space(4.0);
                ui.allocate_ui_with_layout(
                    Vec2::new(COLOR_GROUP_WIDTH, COLOR_GROUP_HEIGHT),
                    Layout::left_to_right(Align::Center),
                    |ui| {
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

                        // Glow Highlight button + Color Picker
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
                    },
                );
            });
        }

        ui.add_space(6.0);
        ui.separator();
        ui.add_space(6.0);

        let backlinks = self.backlinks_for(note_idx);
        // Relative picture paths resolve from the note's own folder, the way a file in a GitHub
        // repository does. A shared note has no file on this disk, so the library folder is the
        // closest thing it has to one.
        let note_dir: Option<PathBuf> = {
            let note = &self.notes[note_idx];
            if self.is_remote(&note.id) {
                Some(self.storage.root_dir().to_path_buf())
            } else {
                note.file_path.parent().map(Path::to_path_buf)
            }
        };
        // A shared note's pictures live on the server; whoever opens it without them asks for
        // those it does not have yet, and they are drawn a frame later.
        if self.is_remote(&selected_id) {
            let body = self.notes[note_idx].body.clone();
            self.request_shared_pictures(&body);
        }

        let mut preview = PreviewActions::default();
        let mut clicked_backlink_id: Option<String> = None;

        // Two panes need roughly this much room each before their text stops wrapping every
        // other word; in a narrow window the split view shows the editor alone instead of two
        // cramped columns (the chosen mode is kept, so widening the window brings it back).
        const MIN_SPLIT_PANE_WIDTH: f32 = 240.0;
        let view_mode = match self.view_mode {
            EditorViewMode::Split
                if ((ui.available_width() - NOTE_SIDE_MARGIN) * 0.5).max(80.0)
                    < MIN_SPLIT_PANE_WIDTH =>
            {
                EditorViewMode::Edit
            }
            other => other,
        };

        match view_mode {
            EditorViewMode::Edit => {
                note_scroll(
                    ui,
                    "elora_editor_scroll",
                    content_width,
                    right_extra,
                    |ui| {
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
                    },
                );
            }
            EditorViewMode::Preview => {
                note_scroll(
                    ui,
                    "elora_preview_scroll",
                    content_width,
                    right_extra,
                    |ui| {
                        let body_snapshot = self.notes[note_idx].body.clone();
                        render_markdown_preview(
                            ui,
                            &body_snapshot,
                            p,
                            note_dir.as_deref(),
                            &mut self.note_images,
                            &mut preview,
                        );
                        render_backlinks_footer(ui, &backlinks, p, &mut clicked_backlink_id);
                    },
                );
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
                            // the gutter or the window margin, even a chip too wide to fit.                            ui.set_clip_rect(ui.max_rect());
                            note_scroll(ui, "elora_split_left", pane_width, 0.0, |ui| {
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
                    // The preview pane reaches the window edge, so its bar does too.
                    ui.allocate_ui_with_layout(
                        Vec2::new(pane_width + right_extra, pane_height),
                        Layout::top_down(Align::Min),
                        |ui| {
                            ui.set_clip_rect(ui.max_rect());
                            note_scroll(ui, "elora_split_right", pane_width, right_extra, |ui| {
                                let body_snapshot = self.notes[note_idx].body.clone();
                                render_markdown_preview(
                                    ui,
                                    &body_snapshot,
                                    p,
                                    note_dir.as_deref(),
                                    &mut self.note_images,
                                    &mut preview,
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

        // A picture that was dragged to a new size keeps that size: it is written into the note
        // as `{width=…}` after the picture, so it is there again next time the note is opened.
        if let Some((index, width)) = preview.image {
            if let Some(resized) = set_image_width(&self.notes[note_idx].body, index, width) {
                self.notes[note_idx].body = resized;
                self.notes[note_idx].updated_at = Utc::now();
                needs_save = true;
            }
        }

        if let Some(op) = queued_format_op {
            self.apply_editor_format(note_idx, op);
        }

        if let Some(task_i) = preview.toggled_task {
            if self.notes[note_idx].toggle_task_at(task_i) {
                needs_save = true;
            }
        }

        // A shared note is mirrored to everyone with it open as it is typed. A note whose text
        // the connection has not handed over yet is left alone: the editor is still showing an
        // earlier copy of it.
        if self.is_remote(&selected_id) && !self.room_body_pending(&selected_id) {
            let body = self.notes[note_idx].body.clone();
            self.collab.send_body(&selected_id, &body);
            if let Some(caret) = self.last_editor_caret {
                let char_index = caret.min(body.chars().count());
                let byte = body
                    .char_indices()
                    .nth(char_index)
                    .map(|(i, _)| i)
                    .unwrap_or(body.len());
                self.collab.send_cursor(&selected_id, byte);
            }
        }

        if needs_save {
            if let Err(e) = self.persist_note(note_idx) {
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
        if let Some(target) = preview.wiki_link {
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

    /// The window's place and size are written down as it is closed, so the next launch opens
    /// where this one was left instead of at the size the app was first built with.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.persist_settings();
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.track_window(ctx);
        self.handle_shortcuts(ctx);
        self.poll_update();
        self.poll_collab(ctx);
        self.poll_picture_pick();
        self.sync_open_note();
        self.settings_window(ctx);
        self.graph_window(ctx);
        self.broken_links_window(ctx);
        self.trust_prompt_window(ctx);
        let p = self.theme.palette_with(&self.colors, self.glow_strength);

        if !self.focus_mode {
            egui::SidePanel::left("elora_sidebar")
                .resizable(false)
                .exact_width(SIDEBAR_WIDTH)
                // The panel's own separator line is a full border-coloured stroke at the very
                // edge; the divider drawn below replaces it with one mixed towards the sidebar,
                // so the two do not stack into a thick, bright seam.
                .show_separator_line(false)
                // The divider is painted inside the panel below instead of as a frame stroke:
                // a stroke straddles the panel boundary, which at fractional display scales
                // leaves a canvas-coloured pixel between the fill and the line.
                .frame(
                    Frame::new()
                        .fill(p.bg_sidebar)
                        .inner_margin(Margin::symmetric(SIDEBAR_MARGIN_X, SIDEBAR_MARGIN_Y)),
                )
                .show(ctx, |ui| {
                    self.render_sidebar(ui, &p);

                    // The panel's fill is painted before this closure runs, so a divider drawn
                    // here stays on top of it. The content ui clips to the panel, which is the
                    // rect the line belongs to: full height, on the panel's right edge.
                    let ppp = ctx.pixels_per_point();
                    let panel = ui.clip_rect();
                    let right = (panel.right() * ppp).round() / ppp;
                    // A full-strength border line between two dark fills reads as a bright
                    // seam, so it is mixed towards the sidebar side of it.
                    ui.painter().with_clip_rect(panel).rect_filled(
                        egui::Rect::from_min_max(
                            Pos2::new(right - 1.0 / ppp, panel.top()),
                            Pos2::new(right, panel.bottom()),
                        ),
                        0.0,
                        mix_color(p.border, p.bg_sidebar, 0.55),
                    );
                });
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
                // and Preview never shifts the text sideways. The column fills the window
                // rather than stopping at a fixed cap: on a large or fullscreen monitor a
                // capped column left wide empty bands beside the note, and the toolbar above
                // it spanned the whole window while the text sat in the middle. A proportional
                // inset keeps a little air at the edges instead.
                let (pad, width) = note_column(ui.available_width());
                // The column runs to the window's right edge, so a note's scroll bar can sit
                // against it instead of floating inside the inset. What the editor draws is held
                // to `width`, the text column, within.
                let column_rect = Rect::from_min_max(
                    Pos2::new(ui.max_rect().left() + pad, ui.max_rect().top()),
                    ui.max_rect().right_bottom(),
                );
                ui.allocate_new_ui(UiBuilder::new().max_rect(column_rect), |ui| {
                    self.render_editor(ui, &p, width, pad)
                });
            });

        // Pictures dragged onto the window become part of the open note, and while they are
        // being dragged the window says so instead of leaving the drop a guess.
        let hovering = ctx.input(|i| i.raw.hovered_files.clone());
        if !hovering.is_empty() {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                Id::new("elora_drop_hint"),
            ));
            painter.rect_filled(ctx.screen_rect(), 0.0, Color32::from_black_alpha(150));
            painter.text(
                ctx.screen_rect().center(),
                Align2::CENTER_CENTER,
                format!("{} Drop to add the picture to this note", icon::PHOTO),
                FontId::proportional(17.0),
                p.fg_primary,
            );
        }
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if !dropped.is_empty() {
            self.add_dropped_pictures(dropped);
        }

        self.render_command_palette(ctx, &p);
        self.render_share_picker(ctx, &p);
        self.update_window(ctx, &p);
        self.status_toast(ctx, &p);
    }
}

enum CardAction {
    SelectNote(String),
    FilterTag(String),
}

/// Moves the open note's caret and selection across somebody else's edit. The caret belongs to
/// the words around it: when they insert a line above, it has to travel down with the text the
/// user is in the middle of instead of staying behind in what they just wrote. Returns true when
/// the caret actually moved.
fn carry_caret_over(ctx: &egui::Context, note_id: &str, before: &str, after: &str) -> bool {
    let editor_id = Id::new(("md_editor", note_id.to_string()));
    let Some(mut state) = TextEdit::load_state(ctx, editor_id) else {
        return false;
    };
    let Some(range) = state.cursor.char_range() else {
        return false;
    };

    let (start, removed, inserted) = crate::collab::diff(before, after);
    let chars = |text: &str, bytes: usize| text[..bytes].chars().count();
    let start_chars = chars(before, start);
    let removed_chars = chars(&before[start..], removed);
    let inserted_chars = chars(&after[start..], inserted);
    let carried = |index: usize| {
        crate::collab::carry_caret(start_chars, removed_chars, inserted_chars, index)
    };

    let primary = carried(range.primary.index);
    let secondary = carried(range.secondary.index);
    if primary == range.primary.index && secondary == range.secondary.index {
        return false;
    }
    state.cursor.set_char_range(Some(CCursorRange::two(
        CCursor::new(primary),
        CCursor::new(secondary),
    )));
    state.store(ctx, editor_id);
    true
}

/// Where another person's name chip goes: just above their caret, below it when the caret is
/// too near the top of the visible text to fit one above, and pulled back inside the text when
/// the caret sits at the right edge of a wrapped line.
fn caret_chip(top: Pos2, line_height: f32, size: Vec2, clip: Rect) -> Rect {
    let mut chip = Rect::from_min_size(Pos2::new(top.x, top.y - size.y - 1.0), size);
    if chip.top() < clip.top() {
        chip = Rect::from_min_size(Pos2::new(top.x, top.y + line_height + 1.0), size);
    }
    if chip.right() > clip.right() {
        chip = chip.translate(Vec2::new(clip.right() - chip.right(), 0.0));
    }
    chip
}

/// A note's vertical scroll area. The area itself is allowed to reach `bar_extra` points further
/// right than the text it holds, so the scroll bar sits against the window's edge while the text
/// keeps the column's inset.
fn note_scroll(
    ui: &mut Ui,
    id: &str,
    content_width: f32,
    bar_extra: f32,
    add: impl FnOnce(&mut Ui),
) -> Rect {
    ui.scope(|ui| {
        ui.set_max_width(ui.available_width() + bar_extra);
        ScrollArea::vertical()
            .id_salt(id)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_width(content_width);
                add(ui);
            })
    })
    .inner
    .inner_rect
}

/// The inset on each side of the note column and the width of the column itself. A wide window is
/// filled rather than capped at a fixed 980 points, which used to leave wide empty bands beside
/// the note on a large or fullscreen monitor.
fn note_column(available: f32) -> (f32, f32) {
    let pad = (available * 0.04).clamp(NOTE_SIDE_MARGIN, 64.0);
    (pad, (available - pad * 2.0).max(200.0))
}

/// Text colour that stays readable on a filled background: white on dark fills, near-black on
/// light ones, so a pale caret colour does not get white-on-pale text.
/// A file name for a stored picture: lower case letters and digits, with everything else folded
/// into single dashes, so the Markdown address never ends at a space.
fn picture_name(hint: &str) -> String {
    let mut name = String::new();
    for ch in hint.chars() {
        if ch.is_ascii_alphanumeric() {
            name.push(ch.to_ascii_lowercase());
        } else if !name.is_empty() && !name.ends_with('-') {
            name.push('-');
        }
        if name.len() >= 48 {
            break;
        }
    }
    let name = name.trim_matches('-').to_string();
    if name.is_empty() {
        "picture".to_string()
    } else {
        name
    }
}

/// The formats the preview can decode; anything else is stored as PNG instead.
fn picture_extension(extension: &str) -> String {
    let extension = extension
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    match extension.as_str() {
        "png" | "jpg" | "jpeg" | "gif" => extension,
        _ => "png".to_string(),
    }
}

/// The name a picture is filed under: the name it was given, folded down to safe characters,
/// with a short digest of its bytes, so the same picture always lands on the same file and two
/// different pictures never can.
fn stored_picture_name(bytes: &[u8], name_hint: &str, extension: &str) -> String {
    let stem = picture_name(name_hint);
    let extension = picture_extension(extension);
    let digest = Sha256::digest(bytes);
    let tag: String = digest
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{stem}-{tag}.{extension}")
}

/// The pictures a note names with `![alt](attachments/…)`, as plain file names. Anything that is
/// not a plain picture name is ignored: a note can only ask the library for what it could keep.
fn picture_references(body: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for line in body.lines() {
        for piece in split_images(line) {
            let LinePiece::Image { src, .. } = piece else {
                continue;
            };
            let Some(name) = src.trim().strip_prefix("attachments/") else {
                continue;
            };
            if crate::net::valid_attachment_name(name) {
                names.insert(name.to_string());
            }
        }
    }
    names
}

fn is_picture_extension(extension: &str) -> bool {
    matches!(
        extension
            .trim_start_matches('.')
            .to_ascii_lowercase()
            .as_str(),
        "png" | "jpg" | "jpeg" | "gif"
    )
}

/// The picture on the clipboard, encoded as PNG, when this desktop offers one. Wayland sessions
/// that do not bridge the clipboard to X11 simply report nothing here, and text pasting is left
/// to the editor as usual.
fn clipboard_picture() -> Option<(Vec<u8>, String)> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    let image = clipboard.get_image().ok()?;
    let rgba = image::RgbaImage::from_raw(
        u32::try_from(image.width).ok()?,
        u32::try_from(image.height).ok()?,
        image.bytes.into_owned(),
    )?;
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some((png, "screenshot".to_string()))
}

fn readable_on(background: Color32) -> Color32 {
    let luma = 0.299 * background.r() as f32
        + 0.587 * background.g() as f32
        + 0.114 * background.b() as f32;
    if luma > 150.0 {
        Color32::from_rgb(24, 24, 24)
    } else {
        Color32::WHITE
    }
}

/// Blends `b` into `a`: `0.0` keeps `a`, `1.0` gives `b`.
fn mix_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
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

/// The sort chooser at the right of the filter row: a small glyph that opens the three orders,
/// with the current one ticked.
fn sort_menu(ui: &mut Ui, p: &Palette, sort: &mut NoteSort) {
    ui.menu_button(
        RichText::new(icon::ARROWS_SORT)
            .size(13.0)
            .color(p.fg_secondary),
        |ui| {
            ui.set_min_width(150.0);
            for order in [NoteSort::Updated, NoteSort::Created, NoteSort::Title] {
                if ui.selectable_label(*sort == order, order.name()).clicked() {
                    *sort = order;
                    ui.close_menu();
                }
            }
        },
    )
    .response
    .on_hover_text(format!("Sort notes — now by {}", sort.name()));
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

/// Gives the note list's scroll bar a strip of its own at the right edge. A floating bar is
/// painted over the content by default, which here means over the note cards themselves.
fn note_list_scroll(style: &mut egui::style::ScrollStyle) {
    style.floating_allocated_width = SIDEBAR_SCROLL_STRIP;
    style.bar_outer_margin = 0.0;
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
    folder_header_with(ui, icon::FOLDER, name, count, collapsed, p)
}

/// The same row with a glyph of the caller's choosing: the shared library is headed by a cloud,
/// and the folders inside it keep the folder glyph.
fn folder_header_with(
    ui: &mut Ui,
    glyph: &str,
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
        // The count that follows is measured first, so a long folder or server name is elided
        // into the width that is actually left instead of pushing the count out of the panel.
        let count_text = count.to_string();
        let count_width = ui.fonts(|fonts| {
            fonts
                .layout_no_wrap(count_text.clone(), FontId::proportional(10.5), p.fg_muted)
                .size()
                .x
        });
        let resp = ui
            .scope(|ui| {
                ui.set_max_width((ui.available_width() - count_width - 8.0).max(48.0));
                ui.add(
                    egui::Button::new(
                        RichText::new(format!("{chevron} {glyph} {name}"))
                            .size(11.5)
                            .color(p.fg_secondary),
                    )
                    .truncate()
                    .frame(false),
                )
                .on_hover_text(if collapsed {
                    "Expand folder"
                } else {
                    "Collapse folder"
                })
            })
            .inner;

        ui.label(RichText::new(count_text).size(10.5).color(p.fg_muted));
        resp
    })
    .inner
}

/// The entries the folder menu offers outside the folders themselves: the connected server's
/// shared library, and a way back from it onto this computer.
#[derive(Clone, Copy, Default)]
struct MenuExtras<'a> {
    /// Name of the connected server, when this note could be moved into its library.
    server: Option<&'a str>,
    /// True when the note lives on the server and could be copied onto this computer.
    on_server: bool,
}

/// A local note on its way into the shared library, remembered until the server answers.
#[derive(Clone, Debug)]
struct PendingMove {
    local_id: String,
    title: String,
    body: String,
}

/// What the folder menu on a note's header was used for.
#[derive(Clone, Debug, PartialEq)]
enum FolderChoice {
    /// Take the note out of any folder.
    Unfiled,
    Folder(String),
    /// Move the note into the shared library.
    ToServer,
    /// Copy a shared note onto this computer.
    ToComputer,
}

/// What the user clicked in the note list this frame.
#[derive(Default)]
struct CardSelection {
    note_id: Option<String>,
    tag: Option<String>,
}

/// Ko-fi's own symbol, baked into two alpha masks by `scripts/make-kofi-mark.py` from
/// `assets/kofi-symbol.svg` (their brand asset). The mark is drawn with a dark structure — the
/// cup's outline and its handle loop — and an orange heart, so it needs two masks: each is one
/// alpha byte per pixel, and the app paints it in whichever theme colour that part takes.
const KOFI_MASK_WIDTH: usize = 96;
const KOFI_MASK_HEIGHT: usize = 77;
const KOFI_CUP_ALPHA: &[u8] = include_bytes!("../assets/icons/kofi-cup.alpha");
const KOFI_HEART_ALPHA: &[u8] = include_bytes!("../assets/icons/kofi-heart.alpha");

/// The two tinted textures the footer's Ko-fi button draws, loaded once at startup.
struct KofiMark {
    cup: egui::TextureHandle,
    heart: egui::TextureHandle,
}

impl KofiMark {
    fn load(ctx: &egui::Context) -> Option<Self> {
        Some(Self {
            cup: mask_texture(ctx, "kofi-cup", KOFI_CUP_ALPHA)?,
            heart: mask_texture(ctx, "kofi-heart", KOFI_HEART_ALPHA)?,
        })
    }
}

/// Turns one alpha mask into a white texture, so the painter's tint decides its colour and the
/// two halves of the mark can be different colours.
fn mask_texture(ctx: &egui::Context, name: &str, alpha: &[u8]) -> Option<egui::TextureHandle> {
    if alpha.len() != KOFI_MASK_WIDTH * KOFI_MASK_HEIGHT {
        return None;
    }
    let mut rgba = Vec::with_capacity(alpha.len() * 4);
    for &value in alpha {
        rgba.extend_from_slice(&[255, 255, 255, value]);
    }
    let image =
        egui::ColorImage::from_rgba_unmultiplied([KOFI_MASK_WIDTH, KOFI_MASK_HEIGHT], &rgba);
    Some(ctx.load_texture(name, image, egui::TextureOptions::LINEAR))
}

/// Where the mark goes inside the space the footer gives it: as wide as the button, in the
/// symbol's own proportions.
fn kofi_mark_rect(rect: Rect) -> Rect {
    let size = Vec2::new(
        rect.width().min(19.0),
        rect.width().min(19.0) * KOFI_MASK_HEIGHT as f32 / KOFI_MASK_WIDTH as f32,
    );
    Rect::from_center_size(rect.center(), size)
}

/// The footer's Ko-fi button: the cup and its handle in the colour of the icons beside it, and
/// the heart in the theme's accent, with the whole mark taking the accent under the pointer.
fn kofi_button(ui: &mut Ui, p: &Palette, mark: Option<&KofiMark>) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(20.0, 18.0), Sense::click());
    let ink = if response.hovered() {
        p.accent
    } else {
        p.fg_secondary
    };
    match mark {
        Some(mark) => {
            let box_ = kofi_mark_rect(rect);
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
            let painter = ui.painter();
            // The white face of the cup is not painted at all, so the panel shows through it
            // and the mark reads as an outline with a heart in it whatever theme is on.
            painter.image(mark.cup.id(), box_, uv, ink);
            painter.image(mark.heart.id(), box_, uv, p.accent);
        }
        // A build that lost its masks still leaves a working button: the icon font's coffee
        // cup is not Ko-fi's mark, but it is not an empty square either.
        None => {
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                tabler_glyph("coffee").unwrap_or_default(),
                FontId::new(15.0, FontFamily::Name("tabler".into())),
                ink,
            );
        }
    }
    response
}

/// Hands a web address to the desktop's browser. Started and let go: what the browser does with
/// it is none of the app's business, and nothing waits for it to come back.
fn open_in_browser(url: &str) -> Result<(), String> {
    browser_command(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not open the browser: {e}"))
}

/// The desktop's own "open this address" command, per platform. Kept apart from the spawn so a
/// test can read what would be run without starting a browser.
fn browser_command(url: &str) -> std::process::Command {
    #[cfg(target_os = "windows")]
    let command = {
        let mut command = std::process::Command::new("cmd");
        // The empty title matters: `start` takes the first quoted word as the window title.
        command.args(["/C", "start", ""]).arg(url);
        command
    };
    #[cfg(target_os = "macos")]
    let command = {
        let mut command = std::process::Command::new("open");
        command.arg(url);
        command
    };
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(url);
        command
    };
    command
}

/// The glyph and the one-line answer for who can read a note, used by the header's lock button.
fn share_look(
    private: bool,
    shared_with: &[String],
    owner: &str,
    mine: bool,
    server: Option<&str>,
) -> (&'static str, String) {
    if !private {
        let where_ = server.map(|name| format!(" on {name}")).unwrap_or_default();
        return (icon::WORLD, format!("Everyone{where_} can see this note"));
    }
    if mine {
        if shared_with.is_empty() {
            (icon::LOCK, "Only you can see this note".to_string())
        } else {
            (
                icon::LOCK,
                format!("Shared with {}", shared_with.join(", ")),
            )
        }
    } else {
        (icon::LOCK, format!("{owner} shared this note with you"))
    }
}

/// A first public name for this computer: the account name with a short random tail, so it
/// reads as a person rather than a number while still not colliding with anybody else on a
/// server. Settings replaces it with any name the person prefers.
fn default_display_name() -> String {
    let base: String = crate::net::os_username()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        .take(24)
        .collect();
    let suffix = format!("{:04x}", rand::random::<u16>());
    if base.is_empty() {
        format!("guest-{suffix}")
    } else {
        format!("{base}-{suffix}")
    }
}

/// A fresh secret: 128 random bits written as hex. Long enough that guessing one out of the
/// blue is not worth thinking about, and short enough to paste between computers.
fn fresh_identifier() -> String {
    let bytes: [u8; 16] = rand::random();
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Opens the system's "choose a picture" dialog on a thread of its own and hands back the
/// channel its answer arrives on, so the window is never blocked waiting for it.
fn choose_picture_file(ctx: &egui::Context) -> Receiver<Option<PathBuf>> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let wake = ctx.clone();
    std::thread::spawn(move || {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Add a picture to this note")
            .add_filter("Pictures", &["png", "jpg", "jpeg", "gif"]);
        // Start where pictures usually are, but only if that folder really exists: a portal
        // asked for a missing folder is left showing nothing useful.
        if let Some(start) = dirs::picture_dir()
            .filter(|dir| dir.is_dir())
            .or_else(|| dirs::home_dir().filter(|dir| dir.is_dir()))
        {
            dialog = dialog.set_directory(start);
        }
        let chosen = dialog.pick_file();
        let _ = sender.send(chosen);
        // A window that is not asked to draw would not see the answer until something else
        // happened to repaint it.
        wake.request_repaint();
    });
    receiver
}

/// Runs `add` shifted right by `indent`, so a nested group reads as inside its parent.
fn indented<R>(ui: &mut Ui, indent: i8, add: impl FnOnce(&mut Ui) -> R) -> R {
    if indent <= 0 {
        return add(ui);
    }
    Frame::new()
        .inner_margin(Margin {
            left: indent,
            right: 0,
            top: 0,
            bottom: 0,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// Splits note indices by the folder each note is filed in: the unfiled ones, and one group per
/// folder in alphabetical order. Both the sidebar's local folder groups and the shared library's
/// follow the same shape.
fn split_by_folder(notes: &[Note], indices: &[usize]) -> (Vec<usize>, Vec<(String, Vec<usize>)>) {
    let mut unfiled: Vec<usize> = Vec::new();
    let mut folders: Vec<(String, Vec<usize>)> = Vec::new();
    for &idx in indices {
        match &notes[idx].folder {
            Some(folder) => match folders.iter_mut().find(|(name, _)| name == folder) {
                Some((_, list)) => list.push(idx),
                None => folders.push((folder.clone(), vec![idx])),
            },
            None => unfiled.push(idx),
        }
    }
    folders.sort_by_key(|(name, _)| name.to_lowercase());
    (unfiled, folders)
}

/// Renders the given note indices as cards, recording any click in `picked`.
#[allow(clippy::too_many_arguments)]
fn render_note_rows(
    ui: &mut Ui,
    notes: &[Note],
    indices: &[usize],
    selected_id: Option<&str>,
    p: &Palette,
    query: &str,
    collab: Option<&Collab>,
    picked: &mut CardSelection,
    indent: i8,
    show_preview: bool,
    roomy: bool,
) {
    indented(ui, indent, |ui| {
        for &idx in indices {
            let note = &notes[idx];
            let is_selected = selected_id == Some(note.id.as_str());
            let watchers: &[PresenceUser] =
                collab.map(|c| c.watchers(&note.id)).unwrap_or_default();
            let action = if roomy {
                render_note_card(ui, note, is_selected, p, query, watchers, show_preview)
            } else {
                render_compact_note_row(ui, note, is_selected, p, watchers)
            };
            if let Some(action) = action {
                match action {
                    CardAction::SelectNote(id) => picked.note_id = Some(id),
                    CardAction::FilterTag(t) => picked.tag = Some(t),
                }
            }
            ui.add_space(if roomy { 3.0 } else { 1.0 });
        }
    });
}

fn render_note_card(
    ui: &mut Ui,
    note: &Note,
    is_selected: bool,
    p: &Palette,
    query: &str,
    watchers: &[PresenceUser],
    show_preview: bool,
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
                // Truncated rather than allowed to run on: a long title in a one-line row used
                // to widen the whole card past the sidebar, cutting its right edge and every
                // card below it off at the panel's border.
                ui.add(
                    egui::Label::new(
                        RichText::new(note.display_title())
                            .strong()
                            .size(13.5)
                            .color(p.fg_primary),
                    )
                    .truncate(),
                );
                // A brush per person with this note open, in the colour of their caret.
                if !watchers.is_empty() {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        for user in watchers.iter().rev().take(4) {
                            let [r, g, b] = user.color;
                            ui.label(
                                RichText::new(icon::BRUSH)
                                    .size(11.0)
                                    .color(Color32::from_rgb(r, g, b)),
                            )
                            .on_hover_text(format!("{} is editing this note", user.name));
                        }
                    });
                }
            });

            // The preview line is what makes a card tall; without it the title, date and tags
            // are all that is left, so more notes fit in the list.
            if show_preview {
                let excerpt = note.body_snippet(query).unwrap_or_else(|| note.snippet());
                ui.add(
                    egui::Label::new(RichText::new(excerpt).size(11.5).color(p.fg_secondary))
                        .truncate(),
                );
            }

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

/// The one-line row the sidebar shows by default: a small glyph and the title, the way a file
/// list reads. Two rows fit in the height one tall card takes, and the date and tags move into
/// the row's hover text so nothing competes with the title for attention.
fn render_compact_note_row(
    ui: &mut Ui,
    note: &Note,
    is_selected: bool,
    p: &Palette,
    watchers: &[PresenceUser],
) -> Option<CardAction> {
    let height = ui.fonts(|fonts| fonts.row_height(&FontId::proportional(12.5))) + 6.0;
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::click());
    let fill = if is_selected {
        p.bg_selected
    } else if response.hovered() {
        p.bg_hover
    } else {
        Color32::TRANSPARENT
    };
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, CornerRadius::same(4), fill);
    }

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

    let brushes = watchers.len().min(3);
    let mut row = ui.new_child(
        UiBuilder::new()
            .max_rect(rect.shrink2(Vec2::new(7.0, 0.0)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    row.spacing_mut().item_spacing = Vec2::new(6.0, 0.0);
    row.label(RichText::new(glyph).size(12.0).color(color));
    // The title is elided into the room the brushes do not need, so a long name never pushes
    // them out of the row.
    let reserve = if brushes > 0 {
        brushes as f32 * 12.0 + 4.0
    } else {
        0.0
    };
    let title_width = (row.available_width() - reserve).max(24.0);
    row.scope(|ui| {
        ui.set_max_width(title_width);
        ui.add(
            egui::Label::new(
                RichText::new(note.display_title())
                    .size(12.5)
                    .color(if is_selected { p.accent } else { p.fg_primary }),
            )
            .truncate(),
        );
    });
    if brushes > 0 {
        row.with_layout(Layout::right_to_left(Align::Center), |ui| {
            for user in watchers.iter().take(brushes) {
                let [r, g, b] = user.color;
                ui.label(
                    RichText::new(icon::BRUSH)
                        .size(10.0)
                        .color(Color32::from_rgb(r, g, b)),
                )
                .on_hover_text(format!("{} is editing this note", user.name));
            }
        });
    }

    // What the tall card shows in its own rows: when it was last edited, and its tags.
    let tags = note.all_tags();
    let mut hint = note.formatted_updated();
    if !tags.is_empty() {
        hint.push_str("  ·  ");
        hint.push_str(
            &tags
                .iter()
                .take(4)
                .map(|tag| format!("#{tag}"))
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    if response.on_hover_text(hint).clicked() {
        Some(CardAction::SelectNote(note.id.clone()))
    } else {
        None
    }
}

/// One level of indentation, in spaces. Markdown nests lists and code with spaces, and a literal
/// tab would show up in the preview as an uneven gap.
pub const INDENT: &str = "    ";

/// The byte offset of a character index, or the end of the text when it is past that.
fn char_to_byte(text: &str, index: usize) -> usize {
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
fn caret_range(state: &egui::text_edit::TextEditState, fallback: usize) -> (usize, usize, bool) {
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

/// Fills the box of one match, character by character, so a match that wraps across a line break
/// is highlighted on both lines. `start` and `len` count characters, not bytes. Returns the union
/// of what it painted, which the caller uses to bring the match into view.
fn paint_find_match(
    painter: &egui::Painter,
    galley: &egui::Galley,
    origin: Pos2,
    start: usize,
    len: usize,
    color: Color32,
) -> Rect {
    let mut union = Rect::NOTHING;
    for index in start..start + len {
        let from = galley.pos_from_ccursor(CCursor::new(index));
        let to = galley.pos_from_ccursor(CCursor::new(index + 1));
        let box_ = if (from.top() - to.top()).abs() < 0.5 {
            Rect::from_min_max(from.min, to.max)
        } else {
            // This character ends the row, so the caret for the next one is already on the row
            // below: the wash runs to the end of the row it started on.
            let right = galley
                .rows
                .iter()
                .find(|row| row.rect.y_range().contains(from.center().y))
                .map(|row| row.rect.right())
                .unwrap_or(to.right());
            Rect::from_min_max(from.min, Pos2::new(right, from.max.y))
        };
        let rect = box_
            .translate(origin.to_vec2())
            .expand2(Vec2::new(0.5, 0.5));
        painter.rect_filled(rect, 1.0, color);
        union = union.union(rect);
    }
    union
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
    extras: &MenuExtras<'_>,
) -> Option<FolderChoice> {
    let mut choice: Option<FolderChoice> = None;
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
                choice = Some(FolderChoice::Unfiled);
                ui.memory_mut(|mem| mem.close_popup());
            }
            for folder in folders {
                let selected = current_folder == Some(folder.as_str());
                if ui
                    .selectable_label(selected, format!("{} {folder}", icon::FOLDER))
                    .clicked()
                {
                    choice = Some(FolderChoice::Folder(folder.clone()));
                    ui.memory_mut(|mem| mem.close_popup());
                }
            }
            if let Some(server) = extras.server {
                ui.separator();
                ui.label(
                    RichText::new("Shared libraries")
                        .size(10.5)
                        .color(p.fg_muted),
                );
                if ui
                    .selectable_label(false, format!("{} {server}", icon::CLOUD))
                    .on_hover_text(format!(
                        "Move this note into the shared library on {server}"
                    ))
                    .clicked()
                {
                    choice = Some(FolderChoice::ToServer);
                    ui.memory_mut(|mem| mem.close_popup());
                }
            }
            if extras.on_server {
                ui.separator();
                if ui
                    .selectable_label(false, format!("{} Copy to this computer", icon::SERVER))
                    .on_hover_text("Keep a local copy; the shared note stays on the server")
                    .clicked()
                {
                    choice = Some(FolderChoice::ToComputer);
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
                    choice = Some(FolderChoice::Folder(name));
                    new_folder_input.clear();
                    ui.memory_mut(|mem| mem.close_popup());
                }
            }
        });

    choice
}

/// Interactive Markdown Preview renderer with full multi-word inline formatting,
/// glowing highlights, clickable checkboxes, and `[[Wiki Links]]`.
/// What the preview hands back to the editor: the things a reader can do to the text while it is
/// being shown.
#[derive(Default)]
struct PreviewActions {
    /// A `[[wiki link]]` that was clicked.
    wiki_link: Option<String>,
    /// The task list item that was ticked or unticked, counted over the whole note.
    toggled_task: Option<usize>,
    /// A picture dragged to a new size: which picture (counted over the whole note), and its
    /// width in points.
    image: Option<(usize, u32)>,
}

fn render_markdown_preview(
    ui: &mut Ui,
    markdown: &str,
    p: &Palette,
    note_dir: Option<&Path>,
    images: &mut NoteImages,
    actions: &mut PreviewActions,
) {
    let mut in_code_block = false;
    let mut code_buffer = String::new();
    let mut task_counter = 0usize;
    // Pictures are counted across the whole note, so a width the user drags is written back to
    // the picture they dragged and not to one that merely looks like it (a code block also
    // counts, because the note's own text is scanned the same way).
    let mut image_count = 0usize;

    for line in markdown.lines() {
        let trimmed = line.trim_end();
        let pictures_here = split_images(line)
            .iter()
            .filter(|piece| matches!(piece, LinePiece::Image { .. }))
            .count();
        if trimmed.trim_start().starts_with("```") {
            image_count += pictures_here;
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
            image_count += pictures_here;
            continue;
        }

        if trimmed.is_empty() {
            ui.add_space(6.0);
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
                                p.fg_primary,
                                &mut actions.wiki_link,
                                false,
                            );
                        });
                    }
                    LinePiece::Image { alt, src, width } => {
                        let index = image_count;
                        image_count += 1;
                        if let Some(kept) = images.show(ui, &src, &alt, width, note_dir, p) {
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
                            &mut actions.wiki_link,
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
                    actions.toggled_task = Some(this_idx);
                }
                ui.add_space(4.0);
                render_inline_spans_ui(
                    ui,
                    todo,
                    14.5,
                    p,
                    p.fg_primary,
                    &mut actions.wiki_link,
                    false,
                );
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
                    p.fg_primary,
                    &mut actions.wiki_link,
                    false,
                );
            });
        } else {
            ui.horizontal_wrapped(|ui| {
                render_inline_spans_ui(
                    ui,
                    trimmed_start,
                    14.5,
                    p,
                    p.fg_primary,
                    &mut actions.wiki_link,
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
        choice: Option<FolderChoice>,
        server: Option<String>,
        on_server: bool,
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
                server: None,
                on_server: false,
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
            let server = self.server.clone();
            let on_server = self.on_server;
            let input = &mut self.input;
            let choice = &mut self.choice;
            let combo_id = &mut self.combo_id;
            let _ = self.ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    *combo_id = Some(ui.make_persistent_id(Id::new("note_folder_choice")));
                    let extras = MenuExtras {
                        server: server.as_deref(),
                        on_server,
                    };
                    *choice = folder_menu_ui(ui, &p, None, &folders, input, 148.0, &extras);
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

        assert_eq!(h.choice, Some(FolderChoice::Folder("Research".to_string())));
        assert!(h.input.is_empty(), "the field should reset after use");
        assert!(
            !egui::ComboBox::is_open(&h.ctx, widget_id),
            "picking a folder should close the folder menu"
        );
    }

    /// The shared library's notes are split into folders the same way local ones are: the
    /// unfiled ones stay in order and each folder becomes an alphabetical group.
    #[test]
    fn test_shared_library_notes_are_grouped_by_their_folder() {
        let now = Utc::now();
        let note = |id: &str, folder: Option<&str>| Note {
            id: id.to_string(),
            title: id.to_string(),
            body: String::new(),
            pinned: false,
            trashed: false,
            explicit_tags: Vec::new(),
            icon: None,
            folder: folder.map(str::to_string),
            owner: None,
            owner_key: None,
            private: false,
            shared_with: Vec::new(),
            shared_keys: Vec::new(),
            created_at: now,
            updated_at: now,
            file_path: PathBuf::new(),
        };
        let notes = vec![
            note("loose", None),
            note("b", Some("Second")),
            note("a", Some("Alpha")),
            note("b2", Some("Second")),
            note("loose2", None),
        ];

        let (unfiled, folders) = split_by_folder(&notes, &[0, 1, 2, 3, 4]);
        assert_eq!(unfiled, vec![0, 4]);
        assert_eq!(folders.len(), 2);
        assert_eq!(folders[0].0, "Alpha");
        assert_eq!(folders[0].1, vec![2]);
        assert_eq!(folders[1].0, "Second");
        assert_eq!(folders[1].1, vec![1, 3]);
    }

    /// A folder's notes, and everything inside the shared library, sit in from the left edge so
    /// the group they belong to reads as their parent.
    #[test]
    fn test_folder_contents_are_stepped_in() {
        let ctx = egui::Context::default();
        install_tabler_icons(&ctx);
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(240.0, 200.0))),
            ..Default::default()
        };
        let mut rects: Vec<Rect> = Vec::new();
        let mut edges: Vec<f32> = Vec::new();
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default()
                .frame(Frame::new().inner_margin(Margin::ZERO))
                .show(ctx, |ui| {
                    rects.push(indented(ui, 0, |ui| ui.button("flush").rect));
                    rects.push(indented(ui, SIDEBAR_INDENT_STEP, |ui| {
                        ui.button("nested").rect
                    }));
                    rects.push(indented(ui, SIDEBAR_INDENT_STEP * 2, |ui| {
                        ui.button("deep").rect
                    }));
                    // A full-width row inside the indent still ends at the panel's edge, so
                    // stepping a note in narrows it instead of pushing it off the sidebar.
                    edges.push(
                        indented(ui, SIDEBAR_INDENT_STEP, |ui| {
                            ui.allocate_response(
                                egui::vec2(ui.available_width(), 8.0),
                                Sense::hover(),
                            )
                            .rect
                        })
                        .right(),
                    );
                });
        });

        assert!(
            (edges[0] - 240.0).abs() < 0.5,
            "an indented row should still reach the panel edge: {}",
            edges[0]
        );

        let step = f32::from(SIDEBAR_INDENT_STEP);
        assert!(
            rects[0].left().abs() < 0.5,
            "the top level starts at the edge"
        );
        assert!(
            (rects[1].left() - step).abs() < 0.5,
            "a nested row should start {step} points in, not at {}",
            rects[1].left()
        );
        assert!(
            (rects[2].left() - step * 2.0).abs() < 0.5,
            "a twice-nested row should start {} points in, not at {}",
            step * 2.0,
            rects[2].left()
        );
    }

    /// The note list's scroll bar sits in a strip of its own on the right, so it never covers
    /// the cards. It used to float over them, right where their text is.
    #[test]
    fn test_the_note_lists_scroll_bar_sits_beside_the_cards() {
        let ctx = egui::Context::default();
        let mut outer = Rect::NOTHING;
        let mut inner = Rect::NOTHING;
        let mut card_right = 0.0f32;

        // Several frames: the strip the bar reserves widens as the bar fades in.
        for frame in 0..10 {
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(210.0, 400.0))),
                time: Some(frame as f64 * 0.1),
                predicted_dt: 0.1,
                ..Default::default()
            };
            let _ = ctx.run(raw, |ctx| {
                egui::CentralPanel::default()
                    .frame(Frame::new().inner_margin(Margin::ZERO))
                    .show(ctx, |ui| {
                        ui.scope(|ui| {
                            note_list_scroll(&mut ui.spacing_mut().scroll);
                            outer = ui.available_rect_before_wrap();
                            let out = ScrollArea::vertical()
                                .id_salt("note_list_strip")
                                .max_height(120.0)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    for _ in 0..40 {
                                        ui.allocate_response(
                                            egui::vec2(ui.available_width(), 12.0),
                                            Sense::hover(),
                                        );
                                    }
                                    card_right = ui.min_rect().right();
                                });
                            inner = out.inner_rect;
                        });
                    });
            });
        }

        assert!(
            (card_right - inner.right()).abs() < 0.5,
            "the cards should fill the scrolling area: {card_right} against {}",
            inner.right()
        );
        let strip = outer.right() - inner.right();
        assert!(
            strip >= 9.0,
            "the bar needs a strip of its own at the right, but only {strip} points are kept free"
        );
    }

    /// Turning the preview line off leaves a shorter card, so more notes fit in the sidebar.
    #[test]
    fn test_hiding_the_preview_line_makes_note_cards_shorter() {
        let ctx = egui::Context::default();
        install_tabler_icons(&ctx);
        let p = ThemeMode::Obsidian.palette();
        let now = Utc::now();
        let note = Note {
            id: "card".into(),
            title: "Card".into(),
            body: "A line of the note that the card preview would show.".into(),
            pinned: false,
            trashed: false,
            explicit_tags: vec!["tag".into()],
            icon: None,
            folder: None,
            owner: None,
            owner_key: None,
            private: false,
            shared_with: Vec::new(),
            shared_keys: Vec::new(),
            created_at: now,
            updated_at: now,
            file_path: PathBuf::new(),
        };
        let height = |show_preview: bool| {
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(210.0, 300.0))),
                ..Default::default()
            };
            let mut drawn = 0.0;
            let _ = ctx.run(raw, |ctx| {
                egui::CentralPanel::default()
                    .frame(Frame::new().inner_margin(Margin::ZERO))
                    .show(ctx, |ui| {
                        render_note_card(ui, &note, false, &p, "", &[], show_preview);
                        drawn = ctx
                            .read_response(Id::new(("note_card", &note.id)))
                            .map(|response| response.rect.height())
                            .unwrap_or(0.0);
                    });
            });
            drawn
        };

        let with = height(true);
        let without = height(false);
        assert!(
            with - without > 8.0,
            "the preview line should account for a line of the card: {with} against {without}"
        );
    }

    #[test]
    fn test_remote_edit_carries_the_caret_with_its_text() {
        let ctx = egui::Context::default();
        let note_id = "shared-note";
        let editor_id = Id::new(("md_editor", note_id.to_string()));
        let frame = |ctx: &egui::Context, body: &mut String| {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(640.0, 480.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    TextEdit::multiline(body).id(editor_id).show(ui);
                });
            });
        };

        let before = "one\ntwo";
        let mut body = before.to_string();
        frame(&ctx, &mut body);

        // The caret sits inside "two", as it would after somebody typed there.
        let mut state = TextEdit::load_state(&ctx, editor_id).expect("editor state");
        state
            .cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(6))));
        state.store(&ctx, editor_id);

        // Somebody else inserts a line above it; the caret travels down with "two".
        assert!(carry_caret_over(&ctx, note_id, before, "zero\none\ntwo"));

        let state = TextEdit::load_state(&ctx, editor_id).expect("editor state");
        let range = state.cursor.char_range().expect("caret");
        assert_eq!(range.primary.index, 11, "caret should still sit after 'tw'");

        // An edit after the caret leaves it exactly where it is, and nothing jumps.
        assert!(!carry_caret_over(
            &ctx,
            note_id,
            "zero\none\ntwo",
            "zero\none\ntwo!"
        ));
        let state = TextEdit::load_state(&ctx, editor_id).expect("editor state");
        let range = state.cursor.char_range().expect("caret");
        assert_eq!(range.primary.index, 11);
    }

    #[test]
    fn test_caret_chip_sits_above_and_stays_visible() {
        let clip = Rect::from_min_size(Pos2::ZERO, egui::vec2(500.0, 400.0));
        let size = Vec2::new(60.0, 14.0);

        // Usually the name sits just above the caret, on the line above.
        let chip = caret_chip(Pos2::new(100.0, 120.0), 18.0, size, clip);
        assert_eq!(chip.bottom(), 119.0);
        assert_eq!(chip.left(), 100.0);

        // On the first visible line there is no room above, so it drops below instead of being
        // clipped away at the top of the editor.
        let chip = caret_chip(Pos2::new(100.0, 4.0), 18.0, size, clip);
        assert_eq!(chip.top(), 23.0);

        // At the end of a wrapped line it is pulled back inside the text.
        let chip = caret_chip(Pos2::new(470.0, 120.0), 18.0, size, clip);
        assert_eq!(chip.right(), clip.right());
    }

    #[test]
    fn test_caret_chip_text_contrast() {
        // A pale caret colour needs dark text, a deep one white text.
        assert_eq!(
            readable_on(Color32::from_rgb(255, 226, 130)),
            Color32::from_rgb(24, 24, 24)
        );
        assert_eq!(readable_on(Color32::from_rgb(40, 70, 140)), Color32::WHITE);
    }

    #[test]
    fn test_note_column_fills_a_wide_window() {
        // The column used to stop at 980 points, leaving 600-point bands empty on either side of
        // a maximised window. It now takes the width it is given, keeping only a small inset.
        let (pad, width) = note_column(2400.0);
        assert_eq!(pad, 64.0);
        assert_eq!(width, 2400.0 - 128.0);
        assert!(
            width > 2000.0,
            "a wide window should not be left mostly empty"
        );

        // A narrow window keeps its small margin and never collapses the text away.
        let (pad, width) = note_column(320.0);
        assert_eq!(pad, NOTE_SIDE_MARGIN);
        assert_eq!(width, 320.0 - NOTE_SIDE_MARGIN * 2.0);
        assert!(note_column(100.0).1 >= 200.0);
    }

    /// The note's scroll bar belongs against the window's edge, not out in the middle of the
    /// inset the note leaves beside itself — while the text it holds keeps the column's width.
    #[test]
    fn test_a_notes_scroll_bar_reaches_the_window_edge() {
        let ctx = egui::Context::default();
        install_tabler_icons(&ctx);
        let window = egui::vec2(900.0, 600.0);
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, window)),
            ..Default::default()
        };
        let mut area: Option<Rect> = None;
        let mut column = 0.0f32;
        let mut inset = 0.0f32;
        let _ = ctx.run(raw, |ctx| {
            // The same frame the app uses: full width, no side margin of its own.
            egui::CentralPanel::default()
                .frame(Frame::new().inner_margin(Margin {
                    left: 0,
                    right: 0,
                    top: 18,
                    bottom: 18,
                }))
                .show(ctx, |ui| {
                    let (pad, width) = note_column(ui.available_width());
                    column = width;
                    inset = pad;
                    let column_rect = Rect::from_min_max(
                        Pos2::new(ui.max_rect().left() + pad, ui.max_rect().top()),
                        ui.max_rect().right_bottom(),
                    );
                    ui.allocate_new_ui(UiBuilder::new().max_rect(column_rect), |ui| {
                        ui.set_max_width(width);
                        area = Some(note_scroll(ui, "edge_test", width, pad, |ui| {
                            for line in 0..200 {
                                ui.label(format!("line {line}"));
                            }
                        }));
                    });
                });
        });

        let area = area.expect("the scroll area was drawn");
        assert!(
            (area.right() - window.x).abs() < 0.5,
            "the scroll bar should sit at the window edge: area ends at {}, window at {}",
            area.right(),
            window.x
        );
        assert!(
            (area.left() - inset).abs() < 0.5 && (area.width() - (column + inset)).abs() < 0.5,
            "the area is the column plus the empty strip beside it: {}..{} for a column of {column}",
            area.left(),
            area.right()
        );
    }

    /// A fresh install needs both halves of who it is: a public name others can see and type,
    /// and a secret the server alone reads so nobody can join as that name.
    #[test]
    fn test_a_fresh_install_gets_a_public_name_and_a_secret() {
        let dir = std::env::temp_dir().join(format!(
            "elora-name-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let storage = NoteStorage::new(dir.join("notes")).unwrap();
        let ctx = egui::Context::default();
        let mut app = EloraApp::with_context(&ctx, storage);

        // A fresh install gets a name of its own, so the shared library works before anything
        // is typed. It reads as a person, not as a number.
        let generated = app.server_display_name();
        assert!(!generated.trim().is_empty(), "a name is made up front");
        assert_eq!(
            generated, app.display_name,
            "it is saved, not made per launch"
        );
        assert!(
            generated.len() > 4 && generated.len() <= 64,
            "a usable name: {generated}"
        );

        // The secret behind it is a long random string of hex, not the name and not something
        // that could be guessed from it.
        let secret = app.identifier.clone();
        assert_eq!(
            secret.len(),
            32,
            "128 random bits, written as hex: {secret}"
        );
        assert!(
            secret.chars().all(|ch| ch.is_ascii_hexdigit()),
            "only hex digits: {secret}"
        );
        assert_ne!(secret, app.server_display_name(), "not the public name");
        assert!(
            secret.len() >= MIN_IDENTIFIER,
            "long enough for the server to accept it"
        );

        // And both are still there after a restart, so a private note found on another computer
        // is found again here, and the secret can be copied to it.
        let reopened = EloraApp::with_context(&ctx, NoteStorage::new(dir.join("notes")).unwrap());
        assert_eq!(reopened.server_display_name(), generated);
        assert_eq!(reopened.identifier, secret);

        // A name the person picks is used exactly, minus the space they typed around it, and
        // leaves the secret alone: renaming is not re-keying.
        app.display_name = "  SuperSaiyan  ".to_string();
        assert_eq!(app.server_display_name(), "SuperSaiyan");
        app.persist_settings();
        let reopened = EloraApp::with_context(&ctx, NoteStorage::new(dir.join("notes")).unwrap());
        assert_eq!(reopened.server_display_name(), "SuperSaiyan");
        assert_eq!(reopened.identifier, secret);

        // A key somebody types themselves is kept exactly: that is how a second computer is
        // given the same identity as the first.
        app.identifier = "0123456789abcdef0123456789abcdef".to_string();
        app.persist_settings();
        let reopened = EloraApp::with_context(&ctx, NoteStorage::new(dir.join("notes")).unwrap());
        assert_eq!(reopened.identifier, app.identifier);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The colour picked for the caret is a setting like any other: nothing chosen means the
    /// name picks one, and a chosen colour survives a restart so it is picked once.
    #[test]
    fn test_a_colour_picked_for_the_caret_is_remembered() {
        let dir = std::env::temp_dir().join(format!(
            "elora-colour-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let storage = NoteStorage::new(dir.join("notes")).unwrap();
        let ctx = egui::Context::default();
        let mut app = EloraApp::with_context(&ctx, storage);

        assert_eq!(app.peer_color, None, "nothing is chosen on a fresh install");

        app.peer_color = Some([240, 10, 20]);
        app.persist_settings();
        let reopened = EloraApp::with_context(&ctx, NoteStorage::new(dir.join("notes")).unwrap());
        assert_eq!(reopened.peer_color, Some([240, 10, 20]));

        // The button beside the picker puts it back to the colour the name picks, and that
        // cleared state is what is saved: going back to automatic survives a restart too.
        app.peer_color = None;
        app.persist_settings();
        let reopened = EloraApp::with_context(&ctx, NoteStorage::new(dir.join("notes")).unwrap());
        assert_eq!(reopened.peer_color, None);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A key that is too short is caught in Settings, before a connection is attempted. The
    /// server refuses it too, but that refusal would arrive as an outage notice instead of
    /// pointing at the box that needs the key pasted into it.
    #[test]
    fn test_a_short_identifier_never_reaches_the_server() {
        let dir = std::env::temp_dir().join(format!(
            "elora-short-key-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let storage = NoteStorage::new(dir.join("notes")).unwrap();
        let ctx = egui::Context::default();
        let mut app = EloraApp::with_context(&ctx, storage);
        app.server_form.host = "127.0.0.1".into();
        app.server_form.port = 1;
        app.server_form.password = "letmein".into();
        app.identifier = "abc".to_string();

        app.connect_server();
        let message = app.status_message.clone().expect("a message saying why");
        assert!(
            message.contains(&format!("at least {MIN_IDENTIFIER}")),
            "the reason should name the length: {message}"
        );
        assert!(!app.collab.is_active(), "nothing should be dialled yet");
        assert!(
            app.sent_identifier.is_empty(),
            "a key that was never sent cannot be the one the connection uses"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Everything a frame draws, as text, so a test can say what a person can read on screen.
    fn drawn_text(shapes: &[egui::epaint::ClippedShape]) -> String {
        fn walk(shape: &egui::Shape, out: &mut String) {
            match shape {
                egui::Shape::Text(text) => {
                    out.push_str(text.galley.text());
                    out.push('\n');
                }
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| walk(shape, out)),
                _ => {}
            }
        }
        let mut out = String::new();
        for clipped in shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    /// A notice is not laid out at the width of the one before it. "Connecting…" followed by
    /// the reason it failed used to squeeze the long message into a column one word wide.
    #[test]
    fn test_a_long_notice_after_a_short_one_stays_a_line() {
        let (mut app, _dir, _root) = app_with_note("");
        let ctx = egui::Context::default();
        ThemeMode::Obsidian.apply(&ctx, &ColorOverrides::default(), 15.0, 0.0);
        let p = ThemeMode::Obsidian.palette();

        let frame = |app: &mut EloraApp, window: f32| -> (Vec2, Vec2) {
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(window, 700.0))),
                ..Default::default()
            };
            let out = ctx.run(raw, |ctx| {
                egui::SidePanel::left("side")
                    .exact_width(SIDEBAR_WIDTH)
                    .show(ctx, |_ui| {});
                egui::CentralPanel::default().show(ctx, |_ui| {});
                app.status_toast(ctx, &p);
            });
            let mut text = Vec2::ZERO;
            for clipped in &out.shapes {
                if let egui::Shape::Text(drawn) = &clipped.shape {
                    text = drawn.galley.size();
                }
            }
            let area = ctx
                .memory(|m| m.area_rect(Id::new("elora_status_toast")))
                .map(|rect| rect.size())
                .unwrap_or_default();
            (text, area)
        };

        let long = "could not connect to 192.168.1.20: connection refused (os error 111)";
        for window in [1200.0_f32, 720.0, 640.0] {
            // A short notice first, the way a connection starts, then the long one.
            app.status_message = Some("Connecting…".to_string());
            app.status_shown = None;
            frame(&mut app, window);
            frame(&mut app, window);

            app.status_message = Some(long.to_string());
            app.status_shown = None;
            frame(&mut app, window);
            let (text, area) = frame(&mut app, window);

            assert!(
                text.x > 300.0,
                "in a {window}-wide window the notice is still a column: {text:?}"
            );
            assert!(
                text.y <= 40.0,
                "in a {window}-wide window the notice wrapped into {} rows",
                (text.y / 16.0).round()
            );
            assert!(
                area.x > 300.0 && area.x <= TOAST_MAX_WIDTH + 1.0,
                "in a {window}-wide window the notice box is {area:?}"
            );
            assert!(
                area.y < 60.0,
                "in a {window}-wide window the notice box is {area:?}"
            );
            app.status_message = None;
        }
    }

    /// The name is on screen because it is public; the identifier never is, even in Settings, so
    /// nobody reading over a shoulder gets the key. The two also have to fit side by side in the
    /// settings column.
    #[test]
    fn test_settings_show_the_name_but_never_the_identifier() {
        let dir = std::env::temp_dir().join(format!(
            "elora-key-on-screen-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let storage = NoteStorage::new(dir.join("notes")).unwrap();
        let ctx = egui::Context::default();
        install_tabler_icons(&ctx);
        let mut app = EloraApp::with_context(&ctx, storage);
        app.display_name = "SuperSaiyan".to_string();
        app.identifier = "9f3a1c2b4d5e6f708192a3b4c5d6e7f8".to_string();
        let p = ThemeMode::Obsidian.palette();
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(420.0, 900.0))),
            ..Default::default()
        };
        let output = ctx.run(raw, |ctx| {
            egui::CentralPanel::default()
                .frame(Frame::new().inner_margin(Margin::symmetric(12, 12)))
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        app.shared_library_settings(ui, &p);
                    });
                });
        });

        let text = drawn_text(&output.shapes);
        assert!(
            text.contains("Your name"),
            "the name row is missing: {text}"
        );
        assert!(
            text.contains("Identifier"),
            "the key row is missing: {text}"
        );
        assert!(
            text.contains("SuperSaiyan"),
            "the public name should be readable: {text}"
        );
        assert!(
            !text.contains("9f3a"),
            "the key must not be drawn anywhere: {text}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two launches must not share a secret: it is made once and then kept.
    #[test]
    fn test_two_computers_get_different_secrets() {
        let keep: Vec<String> = (0..2)
            .map(|tag| {
                let dir = std::env::temp_dir().join(format!(
                    "elora-key-{tag}-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
                std::fs::create_dir_all(&dir).unwrap();
                let storage = NoteStorage::new(dir.join("notes")).unwrap();
                let ctx = egui::Context::default();
                let app = EloraApp::with_context(&ctx, storage);
                let _ = std::fs::remove_dir_all(&dir);
                app.identifier
            })
            .collect();
        assert_ne!(keep[0], keep[1], "each install needs its own secret");
    }

    #[test]
    fn test_preview_draws_an_embedded_picture() {
        let ctx = egui::Context::default();
        install_tabler_icons(&ctx);
        let dir = std::env::temp_dir().join(format!(
            "elora-preview-image-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let pixels = image::RgbaImage::from_pixel(6, 4, image::Rgba([90, 150, 210, 255]));
        pixels.save(dir.join("dot.png")).unwrap();

        let markdown = "Before the picture.\n\n![a blue dot](dot.png \"A dot\")\n\nAfter it.";
        let p = ThemeMode::Obsidian.palette();
        let mut images = NoteImages::default();
        let mut preview = PreviewActions::default();
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))),
            ..Default::default()
        };
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render_markdown_preview(ui, markdown, &p, Some(&dir), &mut images, &mut preview);
            });
        });

        assert_eq!(images.ready_pictures(), 1, "the picture should be decoded");
        // Nothing was dragged, so the note's own width is untouched.
        assert_eq!(preview.image, None);
    }

    fn png_bytes() -> Vec<u8> {
        let pixels = image::RgbaImage::from_pixel(4, 3, image::Rgba([10, 200, 40, 255]));
        let mut bytes = Vec::new();
        pixels
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    /// An app over an empty vault holding one note with the given text open in the editor.
    fn app_with_note(body: &str) -> (EloraApp, PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "elora-pictures-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let storage = NoteStorage::new(dir.join("notes")).unwrap();
        let root = storage.root_dir().to_path_buf();
        let ctx = egui::Context::default();
        let mut app = EloraApp::with_context(&ctx, storage);

        let mut note = Note::new("Pictures", body, &root);
        note.body = body.to_string();
        app.selected_id = Some(note.id.clone());
        // As if the editor had drawn this note last frame: that is what makes the caret the
        // insertion point.
        app.last_seen_body = Some((note.id.clone(), note.body.clone()));
        app.notes.push(note);
        (app, dir, root)
    }

    /// The dialog answers between frames, so its result is collected later and goes into the
    /// note it was opened from — not into whatever note happens to be selected by then.
    #[test]
    fn test_the_chosen_picture_is_added_to_the_note_the_dialog_was_opened_for() {
        let (mut app, dir, root) = app_with_note("First line");
        let wanted = app.notes.len() - 1;
        let note_id = app.notes[wanted].id.clone();
        // A second note gets selected while the dialog is still open.
        let other = Note::new("Other", "Somewhere else", &root);
        app.notes.push(other);
        app.selected_id = app.notes.last().map(|note| note.id.clone());

        let picture = dir.join("chosen.png");
        std::fs::write(&picture, png_bytes()).unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        sender.send(Some(picture.clone())).unwrap();
        app.picture_pick = Some(PicturePick {
            note_id: note_id.clone(),
            result: receiver,
        });

        app.poll_picture_pick();

        assert!(app.picture_pick.is_none(), "the answer was collected");
        assert!(
            app.notes[wanted]
                .body
                .contains("![chosen](attachments/chosen-"),
            "the picture went into the note it was picked for: {:?}",
            app.notes[wanted].body
        );
        assert_eq!(
            app.notes.last().unwrap().body,
            "Somewhere else",
            "the note selected meanwhile is left alone"
        );
        let stored = std::fs::read_dir(root.join("attachments"))
            .unwrap()
            .filter_map(|entry| entry.ok())
            .count();
        assert_eq!(stored, 1, "the picture is kept beside the notes");

        // Cancelling the dialog adds nothing and leaves no dialog pending.
        let before = app.notes[wanted].body.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        sender.send(None).unwrap();
        app.picture_pick = Some(PicturePick {
            note_id,
            result: receiver,
        });
        app.poll_picture_pick();
        assert!(app.picture_pick.is_none());
        assert_eq!(
            app.notes[wanted].body, before,
            "a cancelled dialog adds nothing"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_a_picture_is_kept_beside_the_note_and_written_where_the_caret_is() {
        let (mut app, dir, root) = app_with_note("First line\nSecond line");
        let index = app.notes.len() - 1;
        app.last_editor_selection = Some((10, 10));

        app.insert_picture(index, &png_bytes(), "My Cat Photo", "png")
            .unwrap();

        let file = stored_picture_name(&png_bytes(), "My Cat Photo", "png");
        let marker = format!("![My Cat Photo](attachments/{file})");
        assert_eq!(
            app.notes[index].body,
            format!("First line\n{marker}\nSecond line"),
            "the picture goes on a line of its own, in the place being typed"
        );
        // The file lives in the vault, where the note's relative address finds it, and the note
        // on disk already mentions it instead of waiting for the next keystroke.
        assert!(root.join("attachments").join(&file).exists());
        let saved =
            std::fs::read_to_string(root.join(format!("{}.md", app.notes[index].id))).unwrap();
        assert!(saved.contains(&marker), "the note was saved: {saved}");
        // The caret sits behind the picture, ready for the next line.
        assert_eq!(app.last_editor_caret, Some(10 + 1 + marker.chars().count()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Dragging a picture writes its size into the picture that was dragged, even when another
    /// picture (here one sitting in a code block, which is not drawn) comes before it in the text.
    #[test]
    fn test_dragging_a_picture_writes_the_size_of_the_picture_that_was_dragged() {
        let dir = std::env::temp_dir().join(format!(
            "elora-picture-drag-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let pixels = image::RgbaImage::from_pixel(400, 200, image::Rgba([90, 150, 210, 255]));
        pixels.save(dir.join("wide.png")).unwrap();

        let markdown = "A code block with a picture in it:\n\n```\n![inline](wide.png)\n```\n\n![first](wide.png)\n\n![second](wide.png)";
        let p = ThemeMode::Obsidian.palette();
        let ctx = egui::Context::default();
        install_tabler_icons(&ctx);
        let mut images = NoteImages::default();
        let mut preview = PreviewActions::default();
        let mut handles: Vec<Pos2> = Vec::new();

        let frame = |events: Vec<egui::Event>,
                     images: &mut NoteImages,
                     preview: &mut PreviewActions,
                     handles: &mut Vec<Pos2>| {
            images.handles.clear();
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 900.0))),
                events,
                ..Default::default()
            };
            let _ = ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    render_markdown_preview(ui, markdown, &p, Some(&dir), images, preview);
                });
            });
            *handles = images
                .handles
                .iter()
                .map(|(_, rect)| rect.center())
                .collect();
        };

        frame(Vec::new(), &mut images, &mut preview, &mut handles);
        assert_eq!(handles.len(), 2, "the code block's picture is not drawn");

        // Grab the first drawn picture — the second picture in the text — and pull it 60 points.
        let start = handles[0];
        frame(
            vec![
                egui::Event::PointerMoved(start),
                egui::Event::PointerButton {
                    pos: start,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
            &mut images,
            &mut preview,
            &mut handles,
        );
        let moved_to = start + Vec2::new(60.0, 0.0);
        frame(
            vec![egui::Event::PointerMoved(moved_to)],
            &mut images,
            &mut preview,
            &mut handles,
        );
        frame(
            vec![egui::Event::PointerButton {
                pos: moved_to,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
            &mut images,
            &mut preview,
            &mut handles,
        );

        assert_eq!(preview.image, Some((1, 460)));
        // And that index is the drawn picture, not the one in the code block before it.
        let reworded = set_image_width(markdown, 1, 460).unwrap();
        assert!(reworded.contains("![first](wide.png){width=460}"));
        assert!(reworded.contains("```\n![inline](wide.png)\n```"));
        assert!(reworded.ends_with("![second](wide.png)"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The corner drag in the real note view: the pane's column, its scroll area, and the
    /// write-back into the note's text. A size chosen by dragging has to end up in the note.
    #[test]
    fn test_dragging_a_picture_in_the_note_view_writes_the_size_into_the_note() {
        let (mut app, dir, root) = app_with_note("A picture:\n\n![wide](wide.png)\n\nAfter it.");
        image::RgbaImage::from_pixel(400, 200, image::Rgba([90, 150, 210, 255]))
            .save(root.join("wide.png"))
            .unwrap();
        app.view_mode = EditorViewMode::Preview;

        let ctx = egui::Context::default();
        install_tabler_icons(&ctx);
        ThemeMode::Obsidian.apply(&ctx, &ColorOverrides::default(), 15.0, 0.0);
        let p = ThemeMode::Obsidian.palette();
        let mut handle = Pos2::ZERO;
        let frame = |events: Vec<egui::Event>, app: &mut EloraApp, handle: &mut Pos2| {
            app.note_images.handles.clear();
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(900.0, 900.0))),
                events,
                ..Default::default()
            };
            let _ = ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    app.render_editor(ui, &p, 700.0, 0.0);
                });
            });
            *handle = app
                .note_images
                .handles
                .first()
                .map(|(_, rect)| rect.center())
                .unwrap_or(Pos2::ZERO);
        };

        frame(Vec::new(), &mut app, &mut handle);
        assert_ne!(handle, Pos2::ZERO, "the picture was drawn with its handle");
        frame(
            vec![
                egui::Event::PointerMoved(handle),
                egui::Event::PointerButton {
                    pos: handle,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
            &mut app,
            &mut handle,
        );
        let moved_to = handle + Vec2::new(60.0, 0.0);
        frame(
            vec![egui::Event::PointerMoved(moved_to)],
            &mut app,
            &mut handle,
        );
        // The size reaches the note while the picture is still being dragged, so letting go
        // late, or not at all, cannot throw it away.
        assert!(
            app.notes
                .iter()
                .any(|note| note.body.contains("![wide](wide.png){width=460}")),
            "the size should be in the note before the pointer is released",
        );
        frame(
            vec![egui::Event::PointerButton {
                pos: moved_to,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
            &mut app,
            &mut handle,
        );

        assert!(
            app.notes
                .iter()
                .any(|note| note.body.contains("![wide](wide.png){width=460}")),
            "the dragged size should be in the note, got {:?}",
            app.notes.iter().map(|note| &note.body).collect::<Vec<_>>()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_a_picture_goes_to_the_end_when_the_editor_showed_another_note() {
        let (mut app, dir, _root) = app_with_note("One line");
        let index = app.notes.len() - 1;
        // The caret still remembers a position from the note that was open before this one.
        app.last_seen_body = Some(("another-note".to_string(), String::new()));
        app.last_editor_selection = Some((0, 0));

        app.insert_picture(index, &png_bytes(), "cat", "png")
            .unwrap();

        let file = stored_picture_name(&png_bytes(), "cat", "png");
        assert_eq!(
            app.notes[index].body,
            format!("One line\n![cat](attachments/{file})\n"),
            "a stale caret must not put the picture in the middle of the note"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_a_picture_chosen_from_the_toolbar_is_added_like_a_dropped_one() {
        let (mut app, dir, root) = app_with_note("Intro");
        let index = app.notes.len() - 1;
        let picture = dir.join("Kitchen.png");
        std::fs::write(&picture, png_bytes()).unwrap();

        app.add_picture_file(index, &picture).unwrap();

        let file = stored_picture_name(&png_bytes(), "Kitchen", "png");
        assert!(root.join("attachments").join(&file).exists());
        assert!(app.notes[index]
            .body
            .contains(&format!("![Kitchen](attachments/{file})")));

        // A file that is not a picture is refused with a reason, and the note is left alone.
        let document = dir.join("recipe.pdf");
        std::fs::write(&document, b"not a picture").unwrap();
        let problem = app.add_picture_file(index, &document).unwrap_err();
        assert!(
            problem.contains("recipe.pdf") && problem.contains("PNG"),
            "the refusal should say which file and which formats work: {problem}"
        );
        assert_eq!(app.notes[index].body.matches("![").count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_dropping_a_picture_file_adds_it_and_says_when_a_file_is_not_one() {
        let (mut app, dir, root) = app_with_note("Intro");
        let index = app.notes.len() - 1;
        let picture = dir.join("Holiday Photo.png");
        std::fs::write(&picture, png_bytes()).unwrap();
        let document = dir.join("notes.pdf");
        std::fs::write(&document, b"not a picture").unwrap();

        app.add_dropped_pictures(vec![
            egui::DroppedFile {
                path: Some(picture),
                name: "Holiday Photo.png".to_string(),
                mime: String::new(),
                last_modified: None,
                bytes: None,
            },
            egui::DroppedFile {
                path: Some(document),
                name: "notes.pdf".to_string(),
                mime: String::new(),
                last_modified: None,
                bytes: None,
            },
        ]);

        let file = stored_picture_name(&png_bytes(), "Holiday Photo", "png");
        assert!(root.join("attachments").join(&file).exists());
        assert!(
            app.notes[index]
                .body
                .contains(&format!("![Holiday Photo](attachments/{file})")),
            "the dropped picture is written into the note: {}",
            app.notes[index].body
        );
        let status = app.status_message.clone().unwrap();
        assert!(
            status.contains("Added 1 picture") && status.contains("pdf"),
            "the report should mention the picture and the file that was not one: {status}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_the_same_picture_is_filed_once_and_different_ones_apart() {
        let (app, dir, _root) = app_with_note("");
        let first = app.store_picture(&png_bytes(), "cat", "png").unwrap();
        let again = app.store_picture(&png_bytes(), "cat", "png").unwrap();
        assert_eq!(first, again, "the same bytes belong to the same file");

        // Two people's `screenshot.png` are two different pictures, so they get two files even
        // though they were named the same.
        let mut other = png_bytes();
        other.push(7);
        let different = app.store_picture(&other, "cat", "png").unwrap();
        assert_ne!(first.1, different.1);
        assert!(first.1.starts_with("cat-") && first.1.ends_with(".png"));
        assert!(app.storage.root_dir().join(&first.0).exists());
        assert!(app.storage.root_dir().join(&different.0).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_picture_names_survive_being_put_in_markdown() {
        // A space would end the address early, so names are folded into dashes.
        assert_eq!(picture_name("My Cat Photo (1)"), "my-cat-photo-1");
        assert_eq!(picture_name("  ***  "), "picture");
        assert_eq!(picture_name(&"a".repeat(80)).len(), 48);
        // Only the formats the preview decodes are accepted as they are.
        assert_eq!(picture_extension("JPEG"), "jpeg");
        assert_eq!(picture_extension("webp"), "png");
        assert!(is_picture_extension("PNG"));
        assert!(!is_picture_extension("webp"));

        // A stored name carries a digest of the picture, so it is stable and unique.
        let named = stored_picture_name(b"bytes", "Holiday Photo", "PNG");
        assert!(named.starts_with("holiday-photo-") && named.ends_with(".png"));
        assert_eq!(named, stored_picture_name(b"bytes", "Holiday Photo", "PNG"));
        assert_ne!(
            named,
            stored_picture_name(b"other bytes", "Holiday Photo", "PNG")
        );
    }

    #[test]
    fn test_a_picture_from_the_library_joins_the_vault_untouched() {
        let (app, dir, root) = app_with_note("");
        assert!(app
            .save_shared_picture("holiday-1a2b3c4d.png", b"picture")
            .is_ok());
        assert_eq!(
            std::fs::read(root.join("attachments/holiday-1a2b3c4d.png")).unwrap(),
            b"picture"
        );
        // A file already here is left alone, so a local copy is never replaced by the library.
        assert!(app
            .save_shared_picture("holiday-1a2b3c4d.png", b"other")
            .is_ok());
        assert_eq!(
            std::fs::read(root.join("attachments/holiday-1a2b3c4d.png")).unwrap(),
            b"picture"
        );
        // Nothing outside the pictures folder, and nothing that is not a picture.
        assert!(app.save_shared_picture("../escape.png", b"x").is_err());
        assert!(!root.parent().unwrap().join("escape.png").exists());
        assert!(app.save_shared_picture("notes.md", b"x").is_err());
        assert!(app.save_shared_picture("empty-00000000.png", b"").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_a_note_names_the_pictures_it_wants_from_the_library() {
        let body = "Text\n![a](attachments/holiday-1a2b3c4d.png)\n\
                    ![b](attachments/my-cat.JPEG){width=200}\n\
                    ![remote](https://example.com/x.png)\n\
                    ![outside](../secrets.png)";
        let names: Vec<String> = picture_references(body).into_iter().collect();
        assert_eq!(
            names,
            vec!["holiday-1a2b3c4d.png", "my-cat.JPEG"],
            "only pictures kept in the vault are asked for"
        );
    }

    /// Runs the app's connection handling until `done` holds: this is what a window does every
    /// frame, minus the drawing.
    fn wait_for_shared(
        app: &mut EloraApp,
        ctx: &egui::Context,
        label: &str,
        done: impl Fn(&EloraApp) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            app.poll_collab(ctx);
            app.sync_open_note();
            if done(app) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "{label} did not settle: {:?}",
            app.notes
                .iter()
                .filter(|n| app.remote_ids.contains(&n.id))
                .map(|n| (n.display_title(), n.body.clone(), n.folder.clone()))
                .collect::<Vec<_>>()
        );
    }

    fn shared_body(app: &EloraApp, id: &str) -> String {
        app.notes
            .iter()
            .find(|n| n.id == id)
            .map(|n| n.body.clone())
            .unwrap_or_default()
    }

    /// The identifier the owner uses in the sharing test. The fixture note is written before the
    /// app that will own it exists, so its key is worked out from this string both here and
    /// where the app is built.
    const OWNER_IDENTIFIER: &str = "shared-test-owner-identifier-0123456789abcdef";

    /// Starts a real server on a loopback port with one shared note, for app-level tests.
    fn start_shared_server(label: &str) -> (PathBuf, u16, String) {
        let dir = std::env::temp_dir().join(format!(
            "elora-app-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let notes_dir = dir.join("notes");
        std::fs::create_dir_all(&notes_dir).unwrap();
        let note_id = "shared-0001".to_string();
        let owner_key = crate::people::public_key(OWNER_IDENTIFIER);
        std::fs::write(
            notes_dir.join(format!("{note_id}.md")),
            // Owned by nox, open to everyone: the shape a note has once its owner has made it
            // in the library, and the starting point the privacy tests need. The key is the
            // part that decides anything; the name is what the sidebar prints.
            format!(
                "---\ntitle: \"Shared\"\npinned: false\ntrashed: false\ntags: []\n\
                 owner: \"nox\"\nowner_key: \"{owner_key}\"\n\
                 created_at: 2026-01-01T00:00:00+00:00\n\
                 updated_at: 2026-01-01T00:00:00+00:00\n---\nhello"
            ),
        )
        .unwrap();

        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);

        // SAFETY: the lock keeps the other test that changes this variable out of the way.
        let _guard = crate::tls::server_dir_lock();
        unsafe { std::env::set_var("ELORA_SERVER_DIR", dir.join("server")) };
        let config = crate::net::ServerConfig {
            name: "Test Vault".into(),
            port,
            password: "letmein".into(),
            notes_dir,
            bind: "127.0.0.1".into(),
        };
        let server = std::sync::Arc::new(crate::server::Server::new(config).expect("server"));
        std::thread::spawn(move || {
            let _ = server.run();
        });

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        (dir, port, note_id)
    }

    /// Two people on one server: closing a note takes it out of the other person's library, and
    /// naming them on it brings it back — the whole point of the identifier.
    #[test]
    fn test_a_note_closed_to_others_leaves_their_library_until_they_are_named() {
        let (dir, port, shared_id) = start_shared_server("sharing");
        let ctx = egui::Context::default();
        let profile = ServerProfile {
            host: "127.0.0.1".into(),
            port,
            password: "letmein".into(),
            fingerprint: None,
        };

        let mut owner = EloraApp::with_context(&ctx, NoteStorage::new(dir.join("owner")).unwrap());
        owner.display_name = "nox".to_string();
        owner.identifier = OWNER_IDENTIFIER.to_string();
        owner.server_form = profile.clone();
        owner.connect_server();
        wait_for_shared(&mut owner, &ctx, "owner connected", |app| {
            app.collab.is_connected() && app.collab.notes().iter().any(|n| n.id == shared_id)
        });

        let mut friend =
            EloraApp::with_context(&ctx, NoteStorage::new(dir.join("friend")).unwrap());
        friend.display_name = "Mira".to_string();
        friend.server_form = profile;
        friend.connect_server();
        wait_for_shared(&mut friend, &ctx, "friend connected", |app| {
            app.collab.is_connected()
                && app.notes.iter().any(|n| n.id == shared_id)
                && app.remote_ids.contains(&shared_id)
        });

        // The note is somebody else's, so a guest cannot close it — and being told so leaves the
        // connection up, with the reason in the corner.
        friend.collab.set_share(&shared_id, true, Vec::new());
        wait_for_shared(&mut friend, &ctx, "the refusal", |app| {
            app.status_message
                .as_deref()
                .is_some_and(|message| message.contains("only nox"))
        });
        assert!(friend.collab.is_connected(), "a refusal is not an outage");

        // "Only me": it disappears for the other person, in their sidebar too.
        owner.collab.set_share(&shared_id, true, Vec::new());
        wait_for_shared(&mut owner, &ctx, "closed for the owner", |app| {
            app.collab
                .notes()
                .iter()
                .any(|meta| meta.id == shared_id && meta.private)
        });
        wait_for_shared(&mut friend, &ctx, "gone for the friend", |app| {
            !app.notes.iter().any(|note| note.id == shared_id)
        });
        assert!(
            !friend.remote_ids.contains(&shared_id),
            "the restricted note left the friend's list of shared notes"
        );

        // Naming them — with a different capitalisation of their identifier — lets them in.
        owner
            .collab
            .set_share(&shared_id, true, vec!["mira".to_string()]);
        wait_for_shared(&mut friend, &ctx, "shared back", |app| {
            app.notes.iter().any(|note| note.id == shared_id)
        });
        wait_for_shared(&mut owner, &ctx, "the owner sees the name", |app| {
            app.collab
                .notes()
                .iter()
                .any(|meta| meta.id == shared_id && meta.shared_with.len() == 1)
        });

        // And opening it again takes it back to the whole server.
        owner.collab.set_share(&shared_id, false, Vec::new());
        wait_for_shared(&mut owner, &ctx, "opened again", |app| {
            app.collab
                .notes()
                .iter()
                .any(|meta| meta.id == shared_id && !meta.private)
        });
        let _ = std::fs::remove_dir_all(&dir);
    }
    /// The footer's mark is Ko-fi's own symbol, baked into two masks: the cup's structure and
    /// the heart, which are tinted separately. Neither mask carries the white face, so the panel
    /// shows through it and the mark reads as an outline in whatever theme is on.
    #[test]
    fn test_the_kofi_masks_come_from_the_official_symbol() {
        assert_eq!(KOFI_CUP_ALPHA.len(), KOFI_MASK_WIDTH * KOFI_MASK_HEIGHT);
        assert_eq!(KOFI_HEART_ALPHA.len(), KOFI_MASK_WIDTH * KOFI_MASK_HEIGHT);
        let at = |mask: &[u8], x: usize, y: usize| mask[y * KOFI_MASK_WIDTH + x];

        // The cup's wall is solid where it should be, a fifth of the way in from the left...
        assert!(at(KOFI_CUP_ALPHA, 10, 40) > 240, "the cup's left wall");
        // ...and the face inside is empty, so the footer's panel shows through it.
        assert_eq!(at(KOFI_CUP_ALPHA, 48, 20), 0, "the face above the heart");
        assert_eq!(at(KOFI_CUP_ALPHA, 48, 60), 0, "the face below it");
        // The handle's loop sits inside the right edge, above the middle.
        assert!(at(KOFI_CUP_ALPHA, 66, 24) > 200, "the handle's loop");
        // The heart has a mask of its own, and the cup is empty where it sits, so the accent
        // goes on the panel rather than over another colour.
        assert!(at(KOFI_HEART_ALPHA, 40, 30) > 240, "the heart");
        assert_eq!(at(KOFI_CUP_ALPHA, 40, 30), 0, "nothing under the heart");

        // Nothing spills out of the mask, and the heart sits inside the cup.
        let span = |mask: &[u8]| {
            let mut box_ = (usize::MAX, 0, usize::MAX, 0);
            for (index, value) in mask.iter().enumerate() {
                if *value > 8 {
                    let (x, y) = (index % KOFI_MASK_WIDTH, index / KOFI_MASK_WIDTH);
                    box_ = (box_.0.min(x), box_.1.max(x), box_.2.min(y), box_.3.max(y));
                }
            }
            box_
        };
        let cup = span(KOFI_CUP_ALPHA);
        let heart = span(KOFI_HEART_ALPHA);
        assert!(
            cup.0 > 0 && cup.1 < KOFI_MASK_WIDTH - 1 && cup.2 > 0 && cup.3 < KOFI_MASK_HEIGHT - 1,
            "the cup stays inside its mask: {cup:?}"
        );
        assert!(
            heart.0 > cup.0 && heart.1 < cup.1 && heart.2 > cup.2 && heart.3 < cup.3,
            "the heart sits inside the cup: {heart:?} in {cup:?}"
        );
    }

    /// The mark keeps the symbol's proportions and fits the space the footer gives it.
    #[test]
    fn test_the_kofi_mark_fits_its_button() {
        let button = Rect::from_min_size(Pos2::new(4.0, 6.0), Vec2::new(20.0, 18.0));
        let mark = kofi_mark_rect(button);
        assert!(button.contains_rect(mark), "the mark stays in the button");
        let ratio = mark.width() / mark.height();
        let expected = KOFI_MASK_WIDTH as f32 / KOFI_MASK_HEIGHT as f32;
        assert!(
            (ratio - expected).abs() < 0.01,
            "the symbol's own proportions: {ratio} against {expected}"
        );
        assert!((mark.width() - 19.0).abs() < 0.01);
        assert!((mark.center() - button.center()).length() < 0.01);
    }

    /// The button paints both halves of the mark, each from its own texture, in a box of the
    /// size the footer was laid out for.
    #[test]
    fn test_the_kofi_button_paints_both_masks() {
        let ctx = egui::Context::default();
        install_tabler_icons(&ctx);
        let mark = KofiMark::load(&ctx).expect("the masks are embedded");
        let p = ThemeMode::Obsidian.palette();
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(120.0, 40.0))),
            ..Default::default()
        };
        let mut size = Vec2::ZERO;
        let output = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                size = kofi_button(ui, &p, Some(&mark)).rect.size();
            });
        });
        assert_eq!(size, Vec2::new(20.0, 18.0));
        let textured = output
            .shapes
            .iter()
            .filter(|clipped| {
                matches!(&clipped.shape, egui::Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default())
            })
            .count();
        assert_eq!(textured, 2, "the cup and the heart are each drawn");
    }

    /// Both masks load as textures of their own size, and a mask of the wrong length is refused
    /// instead of being painted as a stretched line.
    #[test]
    fn test_the_kofi_textures_load() {
        let ctx = egui::Context::default();
        install_tabler_icons(&ctx);
        let mark = KofiMark::load(&ctx).expect("the masks are embedded");
        assert_eq!(mark.cup.size(), [KOFI_MASK_WIDTH, KOFI_MASK_HEIGHT]);
        assert_eq!(mark.heart.size(), [KOFI_MASK_WIDTH, KOFI_MASK_HEIGHT]);
        assert!(mask_texture(&ctx, "short", &[0, 1, 2]).is_none());
    }

    /// The footer's Ko-fi button hands the browser the page it advertises, on every platform.
    #[test]
    fn test_the_coffee_button_opens_the_kofi_page() {
        assert_eq!(KOFI_URL, "https://ko-fi.com/noxilith");
        let command = browser_command(KOFI_URL);
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.iter().any(|arg| arg == KOFI_URL),
            "the address is what gets opened: {args:?}"
        );
        assert!(
            !command.get_program().is_empty(),
            "something on this platform can open it"
        );
    }

    /// The header's lock button says who can read the note, in one line.
    #[test]
    fn test_the_lock_button_says_who_can_read_the_note() {
        let nobody: Vec<String> = Vec::new();
        let (glyph, text) = share_look(false, &nobody, "nox", true, Some("Home"));
        assert_eq!(glyph, icon::WORLD);
        assert_eq!(text, "Everyone on Home can see this note");

        let (glyph, text) = share_look(true, &nobody, "nox", true, Some("Home"));
        assert_eq!(glyph, icon::LOCK);
        assert_eq!(text, "Only you can see this note");

        let (_, text) = share_look(
            true,
            &["SuperSaiyan".to_string(), "Mira".to_string()],
            "nox",
            true,
            Some("Home"),
        );
        assert_eq!(text, "Shared with SuperSaiyan, Mira");

        // Somebody who was let in is told whose note it is, and cannot claim it as theirs.
        let (_, text) = share_look(true, &nobody, "Nox", false, Some("Home"));
        assert_eq!(text, "Nox shared this note with you");
    }

    #[test]
    fn test_shared_notes_keep_their_own_text_and_folder() {
        let (dir, port, shared_id) = start_shared_server("shared-notes");
        let storage = NoteStorage::new(dir.join("local")).unwrap();
        let ctx = egui::Context::default();
        let mut app = EloraApp::with_context(&ctx, storage);

        app.server_form = ServerProfile {
            host: "127.0.0.1".into(),
            port,
            password: "letmein".into(),
            fingerprint: None,
        };
        app.connect_server();
        wait_for_shared(&mut app, &ctx, "connect", |a| {
            a.collab.is_connected() && !a.collab.notes().is_empty()
        });

        // Opening the shared note hands the window its text.
        app.selected_id = Some(shared_id.clone());
        app.sync_open_note();
        wait_for_shared(&mut app, &ctx, "shared note", |a| {
            shared_body(a, &shared_id) == "hello"
        });

        // The New button with a shared note selected: the note is created on the server.
        app.create_note_from_template(NoteTemplate::Blank);
        wait_for_shared(&mut app, &ctx, "created note", |a| {
            a.notes
                .iter()
                .any(|n| a.remote_ids.contains(&n.id) && n.id != shared_id)
        });
        let new_id = app
            .notes
            .iter()
            .find(|n| app.remote_ids.contains(&n.id) && n.id != shared_id)
            .map(|n| n.id.clone())
            .expect("the new shared note");

        // Type in it the way the editor does, then switch away and back: each note shows its
        // own text, and the new one is not the note it was created from.
        let typed = "the new note's own words";
        let idx = app.notes.iter().position(|n| n.id == new_id).unwrap();
        app.notes[idx].body = typed.to_string();
        app.collab.send_body(&new_id, typed);

        app.selected_id = Some(shared_id.clone());
        app.sync_open_note();
        wait_for_shared(&mut app, &ctx, "back on the shared note", |a| {
            shared_body(a, &shared_id) == "hello"
        });
        app.selected_id = Some(new_id.clone());
        app.sync_open_note();
        wait_for_shared(&mut app, &ctx, "back on the new note", |a| {
            shared_body(a, &new_id) == typed
        });

        // A note whose copy in the window went stale is put right when it is opened again: the
        // room is empty, so only an unconditional hand-over of its text can clear the old one.
        app.create_note_from_template(NoteTemplate::Blank);
        wait_for_shared(&mut app, &ctx, "second new note", |a| {
            a.notes
                .iter()
                .filter(|n| a.remote_ids.contains(&n.id) && n.id != shared_id)
                .count()
                == 2
        });
        let empty_id = app
            .notes
            .iter()
            .find(|n| app.remote_ids.contains(&n.id) && n.id != shared_id && n.id != new_id)
            .map(|n| n.id.clone())
            .expect("the second new note");
        let idx = app.notes.iter().position(|n| n.id == empty_id).unwrap();
        app.notes[idx].body = "hello".to_string();
        app.remote_bodies
            .insert(empty_id.clone(), "hello".to_string());
        app.selected_id = Some(shared_id.clone());
        app.sync_open_note();
        wait_for_shared(&mut app, &ctx, "away from the empty note", |a| {
            shared_body(a, &shared_id) == "hello"
        });
        app.selected_id = Some(empty_id.clone());
        app.sync_open_note();
        // The window is still showing the stale copy here, so the editor must be held back until
        // the room answers with its own (empty) text, or that copy would be written into it.
        assert!(
            app.room_body_pending(&empty_id),
            "the editor should wait for the note's own text"
        );
        wait_for_shared(&mut app, &ctx, "the empty note again", |a| {
            shared_body(a, &empty_id).is_empty()
        });
        assert!(
            !app.room_body_pending(&empty_id),
            "the note's own text arrived, so the editor can send again"
        );

        // A shared note can be filed, and taken out of its folder again.
        app.selected_id = Some(shared_id.clone());
        app.sync_open_note();
        wait_for_shared(&mut app, &ctx, "shared note again", |a| {
            shared_body(a, &shared_id) == "hello"
        });
        let idx = app.notes.iter().position(|n| n.id == shared_id).unwrap();
        app.notes[idx].folder = Some("Team".to_string());
        app.persist_note(idx).unwrap();
        wait_for_shared(&mut app, &ctx, "filed on the server", |a| {
            a.collab
                .notes()
                .iter()
                .any(|m| m.id == shared_id && m.folder.as_deref() == Some("Team"))
        });
        let idx = app.notes.iter().position(|n| n.id == shared_id).unwrap();
        app.notes[idx].folder = None;
        app.persist_note(idx).unwrap();
        wait_for_shared(&mut app, &ctx, "taken out of the folder", |a| {
            a.collab
                .notes()
                .iter()
                .any(|m| m.id == shared_id && m.folder.is_none())
        });

        app.collab.disconnect();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A picture put into a shared note reaches the other computer: the note's text names it,
    /// the library keeps its bytes, and the other app asks for what it has not got and writes
    /// the answer into its own pictures folder, where the preview looks.
    #[test]
    fn test_a_picture_added_to_a_shared_note_reaches_the_other_computer() {
        let (dir, port, shared_id) = start_shared_server("shared-picture");
        let ctx = egui::Context::default();
        let storage = |who: &str| NoteStorage::new(dir.join(who)).unwrap();
        let profile = || ServerProfile {
            host: "127.0.0.1".into(),
            port,
            password: "letmein".into(),
            fingerprint: None,
        };

        let mut alice = EloraApp::with_context(&ctx, storage("alice"));
        let mut bob = EloraApp::with_context(&ctx, storage("bob"));
        for (app, who) in [(&mut alice, "alice"), (&mut bob, "bob")] {
            app.server_form = profile();
            app.connect_server();
            wait_for_shared(app, &ctx, who, |a| {
                a.collab.is_connected() && !a.collab.notes().is_empty()
            });
            app.selected_id = Some(shared_id.clone());
            app.sync_open_note();
            wait_for_shared(app, &ctx, who, |a| shared_body(a, &shared_id) == "hello");
        }

        // Alice puts a picture into the shared note, the way the picture button does.
        let picture = b"\x89PNG a little picture".to_vec();
        let idx = alice
            .notes
            .iter()
            .position(|n| n.id == shared_id)
            .expect("the shared note");
        alice
            .insert_picture(idx, &picture, "holiday", "png")
            .expect("the picture is added to the note");

        let body = shared_body(&alice, &shared_id);
        let name = picture_references(&body)
            .into_iter()
            .next()
            .expect("the note should name the picture it just gained");

        // The library keeps the bytes, so the picture outlives this computer.
        let stored = dir.join("notes").join("attachments").join(&name);
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline && !stored.is_file() {
            alice.poll_collab(&ctx);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            stored.is_file(),
            "the picture should be in the library: {} was never written",
            stored.display()
        );

        // Bob's copy of the note names the picture, though his computer has never had it.
        wait_for_shared(&mut bob, &ctx, "bob's note text", |a| {
            shared_body(a, &shared_id).contains(&name)
        });
        assert!(!bob.attachments_dir().join(&name).is_file());

        // Opening the note asks the library for it, and the answer is kept beside Bob's notes.
        bob.request_shared_pictures(&body);
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline && !bob.attachments_dir().join(&name).is_file() {
            bob.poll_collab(&ctx);
            std::thread::sleep(Duration::from_millis(10));
        }
        let arrived = std::fs::read(bob.attachments_dir().join(&name)).unwrap_or_default();
        assert_eq!(
            arrived, picture,
            "the picture should arrive byte for byte on the other computer"
        );

        alice.collab.disconnect();
        bob.collab.disconnect();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The find bar is drawn above the editor and its matches are painted over the text without
    /// disturbing the note: the smoke test for the parts a headless frame can reach.
    #[test]
    fn test_the_find_bar_draws_over_the_open_note() {
        let dir = std::env::temp_dir().join(format!(
            "elora-find-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let storage = NoteStorage::new(dir.join("notes")).unwrap();
        let notes_dir = storage.root_dir().to_path_buf();
        let ctx = egui::Context::default();
        install_tabler_icons(&ctx);
        let mut app = EloraApp::with_context(&ctx, storage);

        let mut note = Note::new("Find me", "hello one, hello two, HELLO three", &notes_dir);
        note.body = "hello one, hello two, HELLO three\n\nsecond line without it".to_string();
        app.notes = vec![note];
        app.selected_id = Some(app.notes[0].id.clone());
        app.view_mode = EditorViewMode::Split;
        app.find_open = true;
        app.find_query = "hello".to_string();
        app.find_pending_jump = true;

        let p = app.theme.palette();
        // Two frames: the second already knows the editor's state from the first.
        for _ in 0..2 {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 700.0))),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    app.render_editor(ui, &p, 620.0, 0.0);
                });
            });
        }

        assert_eq!(find_matches(&app.notes[0].body, &app.find_query).len(), 3);
        assert_eq!(app.find_current, 0, "the first match is the one stepped to");
        assert!(
            !app.find_pending_jump,
            "the editor took the jump and moved the caret"
        );
        let editor_id = Id::new(("md_editor", app.notes[0].id.clone()));
        let caret = egui::TextEdit::load_state(&ctx, editor_id)
            .and_then(|state| state.cursor.char_range())
            .map(|range| range.primary.index);
        assert_eq!(
            caret,
            Some(0),
            "the caret sits at the first match, which starts the note"
        );

        // Stepping on to the third match takes the caret with it.
        app.find_current = 2;
        app.find_pending_jump = true;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 700.0))),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                app.render_editor(ui, &p, 620.0, 0.0);
            });
        });
        let caret = egui::TextEdit::load_state(&ctx, editor_id)
            .and_then(|state| state.cursor.char_range())
            .map(|range| range.primary.index);
        assert_eq!(caret, Some(22), "the third HELLO starts at character 22");
        assert!(
            app.notes[0].body.contains("hello one"),
            "the note itself is untouched by looking in it: {:?}",
            app.notes[0].body
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The report lists a `[[link]]` that leads nowhere once, with the note it was found in; a
    /// link to a note in the trash counts as broken, because the link would not open it.
    #[test]
    fn test_the_broken_link_report_names_missing_targets_and_their_sources() {
        let dir = std::env::temp_dir().join(format!(
            "elora-links-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let storage = NoteStorage::new(dir.join("notes")).unwrap();
        let notes_dir = storage.root_dir().to_path_buf();
        let ctx = egui::Context::default();
        let mut app = EloraApp::with_context(&ctx, storage);

        let mut index = Note::new("Index", "See [[Roadmap]] and [[Home]].", &notes_dir);
        index.body = "See [[Roadmap]] and [[Home]]. Another [[Roadmap]] link.".to_string();
        let mut home = Note::new("Home", "the note the link points at", &notes_dir);
        let planner = Note::new("Planner", "also points at [[Roadmap]]", &notes_dir);
        let mut old = Note::new("Old", "links to [[Gone]]", &notes_dir);
        old.trashed = true;
        app.notes = vec![index, home.clone(), planner, old];

        let broken = app.collect_broken_links();
        assert_eq!(
            broken,
            vec![(
                "Roadmap".to_string(),
                vec!["Index".to_string(), "Planner".to_string()]
            )],
            "a target is listed once, with every note that links to it"
        );

        // A note in the trash cannot be opened by its link, so the report says so.
        home.trashed = true;
        app.notes[1] = home;
        let broken = app.collect_broken_links();
        assert!(
            broken
                .iter()
                .any(|(target, from)| target == "Home" && from == &vec!["Index".to_string()]),
            "a link into the trash is broken: {broken:?}"
        );
        assert!(
            broken.iter().all(|(target, _)| target != "Gone"),
            "notes in the trash are not searched for links"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The find field looks for the words in the open note: ignoring case, once per match, and
    /// with offsets that always land on a character boundary.
    #[test]
    fn test_find_matches_ignores_case_and_finds_each_match_once() {
        assert_eq!(
            find_matches("One one ONE\nsecond line", "one"),
            vec![0, 4, 8]
        );
        assert!(find_matches("One one ONE", "two").is_empty());
        assert!(
            find_matches("One one ONE", "").is_empty(),
            "an empty query finds nothing"
        );
        assert!(
            find_matches("short", "much longer than the note").is_empty(),
            "a query longer than the text finds nothing"
        );

        // A two-byte character keeps every offset on a character start.
        let body = "héllo there, héllo again";
        let hits = find_matches(body, "héllo");
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert!(hits.iter().all(|&at| body.is_char_boundary(at)));
        assert_eq!(&body[hits[0]..hits[0] + "héllo".len()], "héllo");
    }

    /// Stepping through the matches wraps at both ends, so the arrows never dead-end.
    #[test]
    fn test_stepping_through_matches_wraps_around() {
        assert_eq!(step_match(0, 3, false), 1);
        assert_eq!(step_match(2, 3, false), 0);
        assert_eq!(step_match(0, 3, true), 2);
        assert_eq!(step_match(1, 3, true), 0);
        assert_eq!(step_match(0, 0, false), 0, "nothing to step through");
    }

    /// Tab puts one level of spaces in at the caret and Shift+Tab takes a level off the front of
    /// the caret's line, leaving the caret in the words it was in.
    #[test]
    fn test_indenting_a_line_moves_the_caret_with_the_text() {
        let (text, start, end) = indent_selection("one\ntwo", 4, 4, false);
        assert_eq!(text, "one\n    two");
        assert_eq!((start, end), (8, 8), "the caret stays in front of 'two'");

        // A caret in the middle of a line gets the indent where it stands.
        let (text, start, _) = indent_selection("abcdef", 3, 3, false);
        assert_eq!(text, "abc    def");
        assert_eq!(start, 7);

        // Shift+Tab eats the leading spaces only, never the text.
        let (text, start, end) = indent_selection("one\n    two", 8, 8, true);
        assert_eq!(text, "one\ntwo");
        assert_eq!((start, end), (4, 4));

        // A line with no indent is left alone.
        let (text, start, _) = indent_selection("plain", 2, 2, true);
        assert_eq!(text, "plain");
        assert_eq!(start, 2);

        // A caret sitting inside the indentation follows the line to its new start rather than
        // jumping past it.
        let (text, start, _) = indent_selection("    two", 2, 2, true);
        assert_eq!(text, "two");
        assert_eq!(start, 0);
    }

    /// A selection moves every line it touches, and a selection that stops at the very start of a
    /// line leaves that line where it is.
    #[test]
    fn test_indenting_a_selection_moves_each_line_and_keeps_the_selection() {
        let (text, start, end) = indent_selection("one\ntwo\nthree", 0, 7, false);
        assert_eq!(text, "    one\n    two\nthree");
        assert_eq!((start, end), (4, 15), "the same words stay selected");
        let (text, start, end) = indent_selection("one\ntwo\nthree", 0, 8, false);
        assert_eq!(
            text, "    one\n    two\nthree",
            "a selection that stops at the start of a line leaves that line alone"
        );
        assert_eq!(start, 4);
        assert_eq!(end, 16, "the selection ends at the line's new start");

        // The same block comes back off in one step.
        let (text, start, end) = indent_selection("    one\n    two\nthree", 4, 12, true);
        assert_eq!(text, "one\ntwo\nthree");
        assert_eq!((start, end), (0, 4));

        // Empty lines are not given trailing spaces.
        let (text, _, _) = indent_selection("one\n\ntwo", 0, 8, false);
        assert_eq!(text, "    one\n\n    two");

        // A two-byte character never lands the selection in the middle of it.
        let (text, start, end) = indent_selection("héllo\nwörld", 0, 11, false);
        assert_eq!(text, "    héllo\n    wörld");
        assert!(text.is_char_boundary(char_to_byte(&text, start)));
        assert!(text.is_char_boundary(char_to_byte(&text, end)));
    }

    /// Pressing Tab with the caret in the note indents it there instead of leaving the field,
    /// and Shift+Tab takes the level straight back out.
    #[test]
    fn test_pressing_tab_in_the_editor_indents_and_keeps_the_focus() {
        let dir = std::env::temp_dir().join(format!(
            "elora-tab-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let storage = NoteStorage::new(dir.join("notes")).unwrap();
        let notes_dir = storage.root_dir().to_path_buf();
        let ctx = egui::Context::default();
        install_tabler_icons(&ctx);
        let mut app = EloraApp::with_context(&ctx, storage);

        let mut note = Note::new("Tab", "one\ntwo", &notes_dir);
        note.body = "one\ntwo".to_string();
        app.notes = vec![note];
        app.selected_id = Some(app.notes[0].id.clone());
        app.view_mode = EditorViewMode::Edit;
        let editor_id = Id::new(("md_editor", app.notes[0].id.clone()));
        let p = app.theme.palette();

        let press = |app: &mut EloraApp, modifiers: egui::Modifiers| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 700.0))),
                focused: true,
                events: vec![egui::Event::Key {
                    key: Key::Tab,
                    physical_key: Some(Key::Tab),
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    app.render_editor(ui, &p, 620.0, 0.0);
                });
            });
        };

        // The first frame puts the caret in the field; the note is drawn with it focused so the
        // key press lands where a person's would.
        let mut state = egui::TextEdit::load_state(&ctx, editor_id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(4))));
        state.store(&ctx, editor_id);
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 700.0))),
            focused: true,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                app.render_editor(ui, &p, 620.0, 0.0);
                ui.memory_mut(|m| m.request_focus(editor_id));
            });
        });
        assert!(
            ctx.memory(|m| m.has_focus(editor_id)),
            "the note has the caret"
        );
        // One settled frame: egui keeps Tab for the field from the frame after it gains focus.
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 700.0))),
            focused: true,
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                app.render_editor(ui, &p, 620.0, 0.0);
            });
        });

        press(&mut app, egui::Modifiers::NONE);
        assert_eq!(
            app.notes[0].body, "one\n    two",
            "the line stepped in by one level"
        );
        assert!(
            ctx.memory(|m| m.has_focus(editor_id)),
            "Tab keeps the focus in the note"
        );
        let caret = egui::TextEdit::load_state(&ctx, editor_id)
            .and_then(|state| state.cursor.char_range())
            .map(|range| range.primary.index);
        assert_eq!(caret, Some(8), "the caret moved with 'two'");

        press(&mut app, egui::Modifiers::SHIFT);
        assert_eq!(
            app.notes[0].body, "one\ntwo",
            "Shift+Tab took the level back out"
        );
        let caret = egui::TextEdit::load_state(&ctx, editor_id)
            .and_then(|state| state.cursor.char_range())
            .map(|range| range.primary.index);
        assert_eq!(caret, Some(4));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Enter opens the next line at the same depth, and a list keeps its marker: a bullet stays
    /// a bullet, a task comes back unchecked, and a number counts on.
    #[test]
    fn test_enter_carries_the_indentation_and_the_list_marker() {
        let (text, caret) = newline_with_prefix("    foo", 7, 7);
        assert_eq!(text, "    foo\n    ");
        assert_eq!(caret, 12, "the caret sits after the new line's indentation");

        let (text, caret) = newline_with_prefix("- milk", 6, 6);
        assert_eq!(text, "- milk\n- ");
        assert_eq!(caret, 9);

        let (text, _) = newline_with_prefix("  - [x] done", 12, 12);
        assert_eq!(
            text, "  - [x] done\n  - [ ] ",
            "a done task comes back empty"
        );

        let (text, _) = newline_with_prefix("1. one", 6, 6);
        assert_eq!(text, "1. one\n2. ");

        let (text, _) = newline_with_prefix("9) nine", 7, 7);
        assert_eq!(text, "9) nine\n10) ");

        // An item with nothing in it ends the list: its marker goes and the caret takes the line
        // it leaves behind, rather than another empty bullet opening underneath.
        let (text, caret) = newline_with_prefix("- ", 2, 2);
        assert_eq!(text, "");
        assert_eq!(caret, 0);
        let (text, caret) = newline_with_prefix("  - [ ] ", 8, 8);
        assert_eq!(text, "");
        assert_eq!(caret, 0);
        let (text, caret) = newline_with_prefix("abc\n- \nmore", 6, 6);
        assert_eq!(
            text, "abc\n\nmore",
            "the empty item is gone, nothing else moved"
        );
        assert_eq!(caret, 4, "the caret is on the blank line the item was on");

        // A selection is still replaced by the break, as a plain Enter would replace it.
        let (text, caret) = newline_with_prefix("- \nmore", 0, 3);
        assert_eq!(text, "\nmore");
        assert_eq!(caret, 1);

        // A line that carries no layout gets a plain line break, and a selection is replaced by
        // the break the way a plain Enter would replace it.
        let (text, caret) = newline_with_prefix("plain text", 10, 10);
        assert_eq!(text, "plain text\n");
        assert_eq!(caret, 11);
        let (text, caret) = newline_with_prefix("hello world", 5, 11);
        assert_eq!(text, "hello\n");
        assert_eq!(caret, 6);

        // Splitting a list item in the middle keeps the list going on the new line.
        let (text, caret) = newline_with_prefix("- text", 2, 2);
        assert_eq!(text, "- \n- text");
        assert_eq!(
            caret, 5,
            "the caret sits in front of the words that moved down"
        );

        // A quote is not a list: its own marker carries over, untouched.
        let (text, caret) = newline_with_prefix("> quoted", 8, 8);
        assert_eq!(text, "> quoted\n> ");
        assert_eq!(caret, 11);
    }

    /// Enter inside a blockquote opens the next line still quoted, however deeply it is nested,
    /// and a list inside the quote keeps both its markers.
    #[test]
    fn test_enter_continues_a_blockquote_with_its_nesting() {
        let (text, caret) = newline_with_prefix("> > deep", 8, 8);
        assert_eq!(text, "> > deep\n> > ", "the nesting comes back as it was");
        assert_eq!(caret, 13, "the caret sits after both markers");

        let (text, _) = newline_with_prefix(">> compact", 10, 10);
        assert_eq!(text, ">> compact\n>> ");

        let (text, _) = newline_with_prefix("  > indented", 12, 12);
        assert_eq!(text, "  > indented\n  > ");

        // A list keeps going inside the quote, with its own marker added to the quote's.
        let (text, caret) = newline_with_prefix("> - item", 8, 8);
        assert_eq!(text, "> - item\n> - ");
        assert_eq!(caret, 13);
        let (text, _) = newline_with_prefix("> 1. one", 8, 8);
        assert_eq!(
            text, "> 1. one\n> 2. ",
            "the number counts on inside the quote"
        );
        let (text, _) = newline_with_prefix("> - [x] done", 12, 12);
        assert_eq!(text, "> - [x] done\n> - [ ] ");

        // An empty item inside the quote goes and leaves the quote behind it; an empty quote
        // line ends the quote and leaves a blank line in its place.
        let (text, caret) = newline_with_prefix("> - ", 4, 4);
        assert_eq!(text, "> ");
        assert_eq!(caret, 2);
        let (text, caret) = newline_with_prefix("> ", 2, 2);
        assert_eq!(text, "");
        assert_eq!(caret, 0);

        // Shift+Enter stays inside the quote, and inside an item in it as well.
        let (text, _) = newline_continuing_item("> quoted", 8, 8);
        assert_eq!(text, "> quoted\n> ");
        let (text, caret) = newline_continuing_item("> - item", 8, 8);
        assert_eq!(
            text, "> - item\n>   ",
            "no marker, just the quote and the item's depth"
        );
        assert_eq!(caret, 13);
    }

    /// Enter on an empty quote that sits inside a list item drops only the quote: the item stays
    /// and the caret is in it, ready for text. The quote level around it, if any, stays too.
    #[test]
    fn test_enter_on_an_empty_quote_inside_a_list_item_keeps_the_item() {
        let (text, caret) = newline_with_prefix("- > ", 4, 4);
        assert_eq!(text, "- ", "the quote goes, the bullet stays");
        assert_eq!(caret, 2, "the caret is in the item, not on a new line");

        let (text, caret) = newline_with_prefix("  - > ", 6, 6);
        assert_eq!(text, "  - ");
        assert_eq!(caret, 4);

        // Only the innermost level goes: a nested quote leaves the quote around it, and a quote
        // around the item is untouched.
        let (text, caret) = newline_with_prefix("- > > ", 6, 6);
        assert_eq!(text, "- > ");
        assert_eq!(caret, 4);
        let (text, caret) = newline_with_prefix("> - > ", 6, 6);
        assert_eq!(text, "> - ");
        assert_eq!(caret, 4);

        // A quote with words in it keeps going on the next line.
        let (text, caret) = newline_with_prefix("- > text", 8, 8);
        assert_eq!(text, "- > text\n- > ");
        assert_eq!(caret, 13);

        // Shift+Enter inside that quote stays in the quote, as at any other level.
        let (text, caret) = newline_continuing_item("- > ", 4, 4);
        assert_eq!(text, "- > \n- > ");
        assert_eq!(caret, 9);
    }

    /// A list inside a quote inside a list item keeps going at every level: each quote stays and
    /// each inner marker carries over, counting on or coming back unchecked as usual.
    #[test]
    fn test_enter_continues_a_list_nested_through_a_quote_at_every_level() {
        let (text, caret) = newline_with_prefix("- > - item", 10, 10);
        assert_eq!(text, "- > - item\n- > - ", "the inner bullet carries over");
        assert_eq!(caret, 17);

        let (text, caret) = newline_with_prefix("- > - > deeper", 14, 14);
        assert_eq!(text, "- > - > deeper\n- > - > ", "both quotes stay");
        assert_eq!(caret, 23);

        let (text, _) = newline_with_prefix("- - item", 8, 8);
        assert_eq!(text, "- - item\n- - ");

        let (text, _) = newline_with_prefix("- > 1. one", 10, 10);
        assert_eq!(text, "- > 1. one\n- > 2. ", "the number counts on in place");

        let (text, _) = newline_with_prefix("- > - [x] done", 14, 14);
        assert_eq!(text, "- > - [x] done\n- > - [ ] ");

        // Each empty level is taken away on its own, innermost first.
        let (text, caret) = newline_with_prefix("- > - > ", 8, 8);
        assert_eq!(text, "- > - ");
        assert_eq!(caret, 6);
        let (text, caret) = newline_with_prefix("- > - ", 6, 6);
        assert_eq!(text, "- > ");
        assert_eq!(caret, 4);

        // Shift+Enter stays in the innermost block: quotes carry on and the deepest marker
        // becomes as many spaces as it is wide.
        let (text, caret) = newline_continuing_item("- > - item", 10, 10);
        assert_eq!(text, "- > - item\n- >   ");
        assert_eq!(caret, 17);
    }

    /// Shift+Enter keeps the item going: the marker is not repeated, the line is only stepped in
    /// as far as the marker is wide, and nothing is removed.
    #[test]
    fn test_shift_enter_continues_the_same_item_without_another_marker() {
        let (text, caret) = newline_continuing_item("- milk", 6, 6);
        assert_eq!(
            text, "- milk\n  ",
            "no second bullet, just the item's depth"
        );
        assert_eq!(caret, 9);

        let (text, _) = newline_continuing_item("  - [ ] note", 12, 12);
        assert_eq!(text, "  - [ ] note\n        ", "the task box is six wide");

        let (text, _) = newline_continuing_item("12. twelve", 10, 10);
        assert_eq!(text, "12. twelve\n    ");

        // Even an empty item is only continued, never dropped, and a line that is not a list
        // keeps its plain indentation.
        let (text, _) = newline_continuing_item("- ", 2, 2);
        assert_eq!(text, "- \n  ");
        let (text, _) = newline_continuing_item("    text", 8, 8);
        assert_eq!(text, "    text\n    ");
        let (text, _) = newline_continuing_item("plain", 5, 5);
        assert_eq!(text, "plain\n");

        // A selection is replaced by the break, as anywhere else.
        let (text, caret) = newline_continuing_item("- text", 2, 3);
        assert_eq!(text, "- \n  ext");
        assert_eq!(caret, 5);
    }

    /// Pressing Enter in the note writes the indentation and marker through the editor's own
    /// caret, so the new line is really in the note and really where the caret ends up.
    #[test]
    fn test_pressing_enter_in_the_editor_lays_out_the_new_line() {
        let dir = std::env::temp_dir().join(format!(
            "elora-enter-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let storage = NoteStorage::new(dir.join("notes")).unwrap();
        let notes_dir = storage.root_dir().to_path_buf();
        let ctx = egui::Context::default();
        install_tabler_icons(&ctx);
        let mut app = EloraApp::with_context(&ctx, storage);

        let mut note = Note::new("Enter", "  - [x] done", &notes_dir);
        note.body = "  - [x] done".to_string();
        app.notes = vec![note];
        app.selected_id = Some(app.notes[0].id.clone());
        app.view_mode = EditorViewMode::Edit;
        let editor_id = Id::new(("md_editor", app.notes[0].id.clone()));
        let p = app.theme.palette();

        let frame = |app: &mut EloraApp, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 700.0))),
                focused: true,
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    app.render_editor(ui, &p, 620.0, 0.0);
                });
            });
        };

        let mut state = egui::TextEdit::load_state(&ctx, editor_id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(12))));
        state.store(&ctx, editor_id);
        frame(&mut app, Vec::new());
        ctx.memory_mut(|m| m.request_focus(editor_id));
        frame(&mut app, Vec::new());
        frame(&mut app, Vec::new());
        assert!(ctx.memory(|m| m.has_focus(editor_id)));

        frame(
            &mut app,
            vec![egui::Event::Key {
                key: Key::Enter,
                physical_key: Some(Key::Enter),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert_eq!(app.notes[0].body, "  - [x] done\n  - [ ] ");
        let caret = egui::TextEdit::load_state(&ctx, editor_id)
            .and_then(|state| state.cursor.char_range())
            .map(|range| range.primary.index);
        assert_eq!(caret, Some(21), "the caret starts the new, unchecked task");

        // Enter on that empty item ends the list instead of leaving an empty one behind.
        frame(
            &mut app,
            vec![egui::Event::Key {
                key: Key::Enter,
                physical_key: Some(Key::Enter),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert_eq!(app.notes[0].body, "  - [x] done\n");
        let caret = egui::TextEdit::load_state(&ctx, editor_id)
            .and_then(|state| state.cursor.char_range())
            .map(|range| range.primary.index);
        assert_eq!(caret, Some(13), "the caret sits on the line the item left");

        // Shift+Enter under a list item starts a line inside that item instead of a new one.
        app.notes[0].body = "- milk".to_string();
        let mut state = egui::TextEdit::load_state(&ctx, editor_id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(6))));
        state.store(&ctx, editor_id);
        frame(&mut app, Vec::new());
        frame(
            &mut app,
            vec![egui::Event::Key {
                key: Key::Enter,
                physical_key: Some(Key::Enter),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::SHIFT,
            }],
        );
        assert_eq!(app.notes[0].body, "- milk\n  ");
        let caret = egui::TextEdit::load_state(&ctx, editor_id)
            .and_then(|state| state.cursor.char_range())
            .map(|range| range.primary.index);
        assert_eq!(caret, Some(9), "the caret lines up under the item's words");

        // A blockquote carries on as well.
        app.notes[0].body = "> note".to_string();
        let mut state = egui::TextEdit::load_state(&ctx, editor_id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(6))));
        state.store(&ctx, editor_id);
        frame(&mut app, Vec::new());
        frame(
            &mut app,
            vec![egui::Event::Key {
                key: Key::Enter,
                physical_key: Some(Key::Enter),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert_eq!(app.notes[0].body, "> note\n> ");
        let caret = egui::TextEdit::load_state(&ctx, editor_id)
            .and_then(|state| state.cursor.char_range())
            .map(|range| range.primary.index);
        assert_eq!(caret, Some(9), "the caret is inside the new quoted line");

        // An empty quote inside a list item drops just the quote, through the editor as well.
        app.notes[0].body = "- > ".to_string();
        let mut state = egui::TextEdit::load_state(&ctx, editor_id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(4))));
        state.store(&ctx, editor_id);
        frame(&mut app, Vec::new());
        frame(
            &mut app,
            vec![egui::Event::Key {
                key: Key::Enter,
                physical_key: Some(Key::Enter),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert_eq!(app.notes[0].body, "- ");
        let caret = egui::TextEdit::load_state(&ctx, editor_id)
            .and_then(|state| state.cursor.char_range())
            .map(|range| range.primary.index);
        assert_eq!(caret, Some(2), "the caret is in the item that is left");

        // A list inside a quote inside an item keeps going at every level.
        app.notes[0].body = "- > - item".to_string();
        let mut state = egui::TextEdit::load_state(&ctx, editor_id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(CCursorRange::one(CCursor::new(10))));
        state.store(&ctx, editor_id);
        frame(&mut app, Vec::new());
        frame(
            &mut app,
            vec![egui::Event::Key {
                key: Key::Enter,
                physical_key: Some(Key::Enter),
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert_eq!(app.notes[0].body, "- > - item\n- > - ");
        let caret = egui::TextEdit::load_state(&ctx, editor_id)
            .and_then(|state| state.cursor.char_range())
            .map(|range| range.primary.index);
        assert_eq!(caret, Some(17), "the inner bullet carries over in the note");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The default list is one line per note, and the taller card is still there for anyone who
    /// turns it on: the row has to be roughly half its height, or the setting saves nothing.
    #[test]
    fn test_the_compact_note_row_takes_about_half_the_height_of_a_card() {
        let dir = std::env::temp_dir().join(format!(
            "elora-cards-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut note = Note::new("A note with a title", "the body", &dir);
        note.explicit_tags = vec!["project".to_string(), "spec".to_string()];

        let ctx = egui::Context::default();
        install_tabler_icons(&ctx);
        let p = ThemeMode::Obsidian.palette();
        let mut row_height = 0.0;
        let mut card_height = 0.0;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.scope(|ui| {
                    render_compact_note_row(ui, &note, false, &p, &[]);
                    row_height = ui.min_rect().height();
                });
                ui.scope(|ui| {
                    render_note_card(ui, &note, false, &p, "", &[], false);
                    card_height = ui.min_rect().height();
                });
            });
        });
        assert!(
            row_height > 0.0 && card_height > 0.0,
            "both styles have to draw something: {row_height} and {card_height}"
        );
        assert!(
            row_height * 1.6 <= card_height,
            "the one-line row is {row_height} against the card's {card_height}"
        );
        assert!(
            row_height <= 24.0,
            "a row taller than 24 points is not a file-list row: {row_height}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Each group in the sidebar follows the chosen order, and a search ignores it: the best
    /// match first is what somebody searching wants, not the newest note.
    #[test]
    fn test_the_sidebar_sort_orders_each_group_and_a_search_keeps_its_matches() {
        let dir = std::env::temp_dir().join(format!(
            "elora-sort-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let storage = NoteStorage::new(dir.join("notes")).unwrap();
        let notes_dir = storage.root_dir().to_path_buf();
        let ctx = egui::Context::default();
        let mut app = EloraApp::with_context(&ctx, storage);

        let now = Utc::now();
        let mut zulu = Note::new("Zulu", "body", &notes_dir);
        zulu.updated_at = now - chrono::Duration::days(1);
        zulu.created_at = now - chrono::Duration::days(3);
        let mut alpha = Note::new("Alpha", "body", &notes_dir);
        alpha.updated_at = now - chrono::Duration::days(3);
        alpha.created_at = now - chrono::Duration::days(4);
        let mut mike = Note::new("Mike", "body", &notes_dir);
        mike.updated_at = now - chrono::Duration::days(2);
        mike.created_at = now - chrono::Duration::days(2);
        app.notes = vec![zulu, alpha, mike];

        let mut group = vec![0, 1, 2];
        app.sort = NoteSort::Updated;
        app.sort_group(&mut group);
        assert_eq!(group, vec![0, 2, 1], "last edited first");

        group = vec![0, 1, 2];
        app.sort = NoteSort::Created;
        app.sort_group(&mut group);
        assert_eq!(group, vec![2, 0, 1], "newest note first");

        group = vec![0, 1, 2];
        app.sort = NoteSort::Title;
        app.sort_group(&mut group);
        assert_eq!(group, vec![1, 2, 0], "A to Z");

        // What the search found stays in the order the search ranked it.
        app.search_query = "o".to_string();
        group = vec![2, 0, 1];
        app.sort_group(&mut group);
        assert_eq!(group, vec![2, 0, 1], "relevance wins while searching");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The window's place and size are kept with the rest of the settings, so the next launch can
    /// open the window where the last one was left.
    #[test]
    fn test_the_window_is_remembered() {
        let dir = std::env::temp_dir().join(format!(
            "elora-window-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let storage = NoteStorage::new(dir.join("notes")).unwrap();
        let ctx = egui::Context::default();
        let mut app = EloraApp::with_context(&ctx, storage);
        assert_eq!(
            app.window_state, None,
            "a fresh install has nothing to restore"
        );

        app.window_state = Some(WindowState {
            width: 1180.0,
            height: 760.0,
            x: 120.0,
            y: 80.0,
            maximized: false,
        });
        app.persist_settings();
        let reopened = EloraApp::with_context(&ctx, NoteStorage::new(dir.join("notes")).unwrap());
        assert_eq!(reopened.window_state, app.window_state);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The sidebar has to hold what it draws. A long title in a card's one-line header used to
    /// widen the card past the panel — its right edge and every card below it cut off at the
    /// border — and a footer height reserved in advance was a few points short of what the icons
    /// need, leaving the bottom row hanging past the panel's edge. Both are measured here at
    /// three window sizes, with titles far longer than the sidebar.
    #[test]
    fn test_a_long_title_and_the_footer_both_stay_inside_the_sidebar() {
        let dir = std::env::temp_dir().join(format!(
            "elora-sidebar-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let storage = NoteStorage::new(dir.join("notes")).unwrap();
        for i in 0..12 {
            let mut note = Note::new(
                format!("Note number {i} whose title runs on far past the sidebar edge"),
                "body",
                storage.root_dir(),
            );
            note.explicit_tags = vec!["tag".to_string()];
            // Every other note sits in a folder whose name is far wider than the sidebar, so
            // the folder row is held to the same rule as the card titles.
            if i % 2 == 0 {
                note.folder = Some(format!(
                    "A folder name far longer than the sidebar number {i}"
                ));
            }
            std::fs::write(note.file_path.clone(), note.to_markdown()).unwrap();
        }
        let ctx = egui::Context::default();
        let mut app = EloraApp::with_context(&ctx, storage);
        let p = app.theme.palette_with(&app.colors, app.glow_strength);

        for (w, h) in [(1060.0, 700.0), (900.0, 500.0), (640.0, 420.0)] {
            let mut used = Rect::NOTHING;
            let mut clip = Rect::NOTHING;
            let mut footer = Rect::NOTHING;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(w, h))),
                ..Default::default()
            };
            // Three frames, because the second one already knows what the first drew.
            for _ in 0..3 {
                let _ =
                    ctx.run(input.clone(), |ctx| {
                        egui::SidePanel::left(format!("probe-{w}-{h}"))
                            .resizable(false)
                            .exact_width(SIDEBAR_WIDTH)
                            .show_separator_line(false)
                            .frame(Frame::new().inner_margin(Margin::symmetric(
                                SIDEBAR_MARGIN_X,
                                SIDEBAR_MARGIN_Y,
                            )))
                            .show(ctx, |ui| {
                                footer = app.render_sidebar(ui, &p);
                                used = ui.min_rect();
                                clip = ui.clip_rect();
                            });
                    });
            }

            let inner_right = clip.right() - f32::from(SIDEBAR_MARGIN_X);
            assert!(
                used.right() <= inner_right + 0.01,
                "at {w}x{h} the sidebar drew out to {} with the cards' edge at {inner_right}",
                used.right()
            );
            let inner_bottom = clip.bottom() - f32::from(SIDEBAR_MARGIN_Y);
            assert!(
                footer.bottom() <= inner_bottom + 0.01,
                "at {w}x{h} the footer ends at {} below the panel's edge at {inner_bottom}",
                footer.bottom()
            );
            assert!(
                footer.top() - clip.top() >= 120.0,
                "at {w}x{h} the footer took the whole sidebar: it starts at {}",
                footer.top()
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
