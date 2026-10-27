use crate::collab::{Collab, ConnectionState, Incoming, ServerProfile};
use crate::icon_names::TABLER_ICON_NAMES;
use crate::icons::{icon, install_tabler_icons, tabler_glyph};
use crate::images::{resolve, ImageRef, NoteImages};
use crate::markdown::{
    apply_highlight_selection, build_editor_layout_job, clear_formatting_selection,
    color_swatch_button, insert_at_selection, list_item_body, markdown_to_html_with, parse_table,
    quote_block, render_inline_spans_ui, rgb_to_hex, set_image_width, split_images, wrap_selection,
    ColumnAlign, ExportTargets, LinePiece, Table, HIGHLIGHT_PRESETS,
};
use crate::net::{NoteMeta, PresenceUser};
use crate::people::MIN_IDENTIFIER;
use crate::storage::{
    extract_wiki_links, fuzzy_subsequence, normalize_folder, normalize_tag, sort_notes,
    AppSettings, GraphMotion, Note, NoteSort, NoteStorage, NoteTemplate, WindowState,
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
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::f32::consts::TAU;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

// The app is split by area; each module adds its own `impl EloraApp` block.
mod clients;
mod disk;
mod export;
mod graph;
mod history;
mod pictures;
mod preview;
mod settings;
mod sharing;
mod text;
mod updates;
use clients::McpClients;
use disk::DiskWatch;
use export::*;
use graph::*;
use history::History;
use pictures::*;
use preview::*;
use settings::{McpCheck, McpOptions};
use sharing::*;
use text::*;
use updates::*;

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
    /// Notes an AI assistant made or last changed.
    Assistant,
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
    /// How far the graph's orbits have turned, in radians. It resets when the graph opens.
    graph_angle: f32,
    /// Whether the graph drifts normally, drifts more slowly, or holds still until it is dragged.
    graph_motion: GraphMotion,
    /// Shared-library notes assistants may read but not change; see `AppSettings`.
    mcp_read_only_ids: Vec<String>,
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
    /// Tells the window when another program changed something in the notes folder.
    disk_watch: DiskWatch,
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
    /// Where the last export went, so the next one starts there. `None` until somebody picks a
    /// folder: exports go to the downloads folder until then.
    export_dir: Option<PathBuf>,
    /// A "choose a folder" dialog running on a thread of its own, so the window keeps drawing
    /// while somebody browses.
    export_folder_pick: Option<Receiver<Option<PathBuf>>>,
    /// An export that has been worked out and is waiting for a yes. Nothing is written until the
    /// size has been shown and confirmed.
    export_offer: Option<ExportPlan>,
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
    /// The window of a note's earlier versions, while it is open.
    history: Option<History>,
    /// What the Settings page last found when it tried the assistant connection.
    mcp_check: McpCheck,
    /// The AI apps found on this computer, for adding EloraNotes to them from Settings.
    mcp_clients: McpClients,
    /// The choices that shape the assistant command Settings hands out.
    mcp_options: McpOptions,
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

impl EloraApp {
    pub fn new(cc: &eframe::CreationContext<'_>, storage: NoteStorage) -> Self {
        let mut app = Self::with_context(&cc.egui_ctx, storage);
        app.start_update_check(&cc.egui_ctx, false);
        app.mcp_clients.watch_at_start = true;
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

        let storage_dir = storage.root_dir().to_path_buf();
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
            export_offer: None,
            export_dir: settings.export_dir.as_ref().map(PathBuf::from),
            export_folder_pick: None,
            highlight_rgb: settings.highlight_rgb,
            colors: settings.colors,
            font_size: settings.font_size,
            glow_strength: settings.glow_strength,
            show_settings: false,
            graph: None,
            graph_angle: 0.0,
            graph_motion: settings.graph_motion,
            mcp_read_only_ids: settings.mcp_read_only_ids.clone(),
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
            disk_watch: DiskWatch::start(ctx, storage_dir),
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
            history: None,
            mcp_check: McpCheck::default(),
            mcp_clients: McpClients::default(),
            mcp_options: McpOptions::default(),
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
            export_dir: self
                .export_dir
                .as_ref()
                .map(|dir| dir.display().to_string()),
            show_card_previews: self.show_card_previews,
            graph_motion: self.graph_motion,
            mcp_read_only_ids: self.mcp_read_only_ids.clone(),
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
            let saved = self.storage.save_note(&note).map_err(|e| e.to_string());
            // Something else changed the file since it was read here; that version is now a
            // note of its own rather than lost under this save.
            let kept = self.storage.take_kept_copies();
            if let Some(first) = kept.first() {
                self.status_message = Some(format!(
                    "“{}” was changed by another program; that version is kept as “{}”",
                    note.display_title(),
                    first.display_title()
                ));
                // Appended, not re-sorted: callers still hold `note_idx` and indices before it.
                self.notes.extend(kept);
            }
            return saved;
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
                    SidebarSection::Assistant => {
                        if note.trashed || !note.ai.touched() {
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
        if let Some(existing) = note_for_link(&self.notes, target_trimmed) {
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
        // The panel's own rectangle, taken before anything is drawn: a row that comes out wider
        // than the panel stretches the one `ui.max_rect()` reports, and the footer, which is
        // anchored to that, would be dragged off the panel's edge with it.
        let panel_rect = ui.max_rect();
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
        let assistant_count = self
            .notes
            .iter()
            .filter(|n| !n.trashed && n.ai.touched())
            .count();

        // Notes and Pinned keep their counts at the smaller pill size, and the glyph-only trash
        // button sits to the right of Pinned. The row wraps rather than overflowing if big
        // counts ever make it wider than the sidebar.
        ui.horizontal_wrapped(|ui| {
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
                // The assistant filter, shown once an assistant has made a note.
                let show_assistant =
                    assistant_count > 0 || self.sidebar_section == SidebarSection::Assistant;
                const ASSISTANT_RESERVE: f32 = 34.0;
                let reserve = SORT_RESERVE
                    + if show_assistant {
                        ASSISTANT_RESERVE
                    } else {
                        0.0
                    };
                let room = (ui.available_width() - reserve).max(80.0);
                if !all_tags.is_empty() {
                    ui.scope(|ui| {
                        ui.set_max_width(room);
                        tag_filter_menu(ui, p, &all_tags, &mut self.active_tag);
                    });
                } else {
                    ui.add_space((ui.available_width() - reserve).max(0.0));
                }
                if show_assistant {
                    ui.spacing_mut().button_padding = Vec2::new(6.0, 3.0);
                    if nav_icon_pill(
                        ui,
                        icon::ROBOT,
                        self.sidebar_section == SidebarSection::Assistant,
                        p,
                    )
                    .on_hover_text(format!(
                        "Notes an assistant made or changed ({assistant_count})"
                    ))
                    .clicked()
                    {
                        // A second click leaves the filter again.
                        self.sidebar_section = if self.sidebar_section == SidebarSection::Assistant
                        {
                            SidebarSection::AllNotes
                        } else {
                            SidebarSection::Assistant
                        };
                        self.select_first_visible();
                    }
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
        let content = panel_rect;
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

                // The note graph, just left of Ko-fi: the same view as the palette's
                // "Show Note Graph", one click from the sidebar.
                let graph_open = self.graph.is_some();
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new(icon::GRAPH).size(15.0).color(if graph_open {
                                p.accent
                            } else {
                                p.fg_secondary
                            }),
                        )
                        .frame(false),
                    )
                    .on_hover_text(if graph_open {
                        "Close the note graph"
                    } else {
                        "Show the note graph"
                    })
                    .clicked()
                {
                    self.toggle_graph();
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
                    self.notes[note_idx].ai.edited = false;
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
            self.notes[note_idx].ai.edited = false;
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

            // What an assistant may do with this note, and the earlier versions kept of it.
            let marks = self.notes[note_idx].ai.clone();
            let read_only = if on_server {
                self.mcp_read_only_ids.contains(&selected_id)
            } else {
                marks.read_only
            };
            let mut set_read_only = None;
            let mut show_history = false;
            let robot_tint = if read_only || marks.touched() {
                p.accent
            } else {
                p.fg_secondary
            };
            ui.menu_button(
                RichText::new(icon::ROBOT).size(13.0).color(robot_tint),
                |ui| {
                    ui.set_min_width(250.0);
                    if marks.created {
                        ui.label(
                            RichText::new("An assistant made this note")
                                .size(11.5)
                                .color(p.fg_muted),
                        );
                    } else if marks.edited {
                        ui.label(
                            RichText::new("An assistant changed this note last")
                                .size(11.5)
                                .color(p.fg_muted),
                        );
                    }
                    let mut value = read_only;
                    if ui
                        .checkbox(&mut value, "Assistants can read, not change")
                        .on_hover_text(
                            "An assistant connected over MCP can still read this note, but cannot \
                         edit, rename, move, trash or delete it.",
                        )
                        .changed()
                    {
                        set_read_only = Some(value);
                    }
                    if !on_server
                        && ui
                            .button(format!("{}  Earlier versions…", icon::HISTORY))
                            .on_hover_text("Copies kept when an assistant changed or deleted it")
                            .clicked()
                    {
                        show_history = true;
                        ui.close_menu();
                    }
                },
            )
            .response
            .on_hover_text("Assistants and earlier versions");
            if let Some(value) = set_read_only {
                self.set_mcp_read_only(note_idx, value);
            }
            if show_history {
                self.open_history(note_idx);
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
            self.notes[note_idx].ai.edited = false;
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
        self.check_ai_apps_at_start(ctx);
        self.poll_collab(ctx);
        self.poll_picture_pick();
        self.poll_disk();
        self.sync_open_note();
        self.settings_window(ctx);
        self.graph_window(ctx);
        self.history_window(ctx);
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
        self.render_export_offer(ctx, &p);
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

/// Selecting with the mouse and pulling past the top or bottom of the note keeps the note
/// scrolling that way for as long as the button is held, in the editor and in the preview alike.
/// egui only scrolls to a selection when it changes, so a held pointer used to stop.
fn scroll_while_selecting(ui: &mut Ui) {
    let (pointer, pressed_at, down, dt) = ui.input(|i| {
        (
            i.pointer.latest_pos(),
            i.pointer.press_origin(),
            i.pointer.primary_down(),
            i.stable_dt,
        )
    });
    let clip = ui.clip_rect();
    // The press has to start in the text, not on the scroll bar at the right edge.
    let in_text = pressed_at.is_some_and(|at| {
        clip.contains(at) && at.x < clip.right() - 16.0 && ui.ctx().dragged_id().is_some()
    });
    if let (true, true, Some(pointer)) = (down, in_text, pointer) {
        let step = drag_scroll_step(pointer.y, clip.top(), clip.bottom(), dt);
        if step != 0.0 {
            ui.scroll_with_delta(Vec2::new(0.0, step));
            ui.ctx().request_repaint();
        }
    }
}

/// How far to scroll this frame while a selection is being dragged at `pointer_y`: positive moves
/// the text down (towards its top), negative moves it up. Nothing happens until the pointer is
/// within a band of the edge or past it; beyond that the speed grows with the distance.
fn drag_scroll_step(pointer_y: f32, top: f32, bottom: f32, dt: f32) -> f32 {
    const BAND: f32 = 24.0;
    const PER_POINT: f32 = 12.0;
    const FASTEST: f32 = 1800.0;
    let speed = |distance: f32| ((distance + BAND) * PER_POINT).clamp(0.0, FASTEST) * dt;
    if pointer_y < top + BAND {
        speed(top - pointer_y)
    } else if pointer_y > bottom - BAND {
        -speed(pointer_y - bottom)
    } else {
        0.0
    }
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
                scroll_while_selecting(ui);
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
const KOFI_CUP_ALPHA: &[u8] = include_bytes!("../../assets/icons/kofi-cup.alpha");
const KOFI_HEART_ALPHA: &[u8] = include_bytes!("../../assets/icons/kofi-heart.alpha");

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

/// Opens the system's "choose a folder" dialog on a thread of its own, the way the picture chooser
/// does, so the window keeps drawing while somebody browses.
fn choose_folder(ctx: &egui::Context, start: &Path) -> Receiver<Option<PathBuf>> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let wake = ctx.clone();
    let start = start.to_path_buf();
    std::thread::spawn(move || {
        let mut dialog = rfd::FileDialog::new().set_title("Where should the export go?");
        if start.is_dir() {
            dialog = dialog.set_directory(start);
        }
        let chosen = dialog.pick_folder();
        let _ = sender.send(chosen);
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
            // The whole card is a button: a press on the name has to open the note, not start
            // selecting the words.
            ui.style_mut().interaction.selectable_labels = false;
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
    // The row is a button: a press on the name has to open the note, not start selecting it.
    row.style_mut().interaction.selectable_labels = false;
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

#[cfg(test)]
mod tests;
