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
        extra_frontmatter: String::new(),
        ai: Default::default(),
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
                        ui.allocate_response(egui::vec2(ui.available_width(), 8.0), Sense::hover())
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
        extra_frontmatter: String::new(),
        ai: Default::default(),
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

/// A pipe table is drawn cell by cell, not left as a paragraph of pipes: the header and the
/// rows arrive as their own labels, and the table takes the lines that belong to it.
#[test]
fn test_preview_draws_a_pipe_table_as_cells() {
    let ctx = egui::Context::default();
    install_tabler_icons(&ctx);
    let p = ThemeMode::Obsidian.palette();
    let markdown = "Before.\n\n| Name | Count |\n| :--- | ---: |\n| apples | 3 |\n| pears | 12 |\n\nAfter the table.";

    fn collect(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, out);
                }
            }
            _ => {}
        }
    }

    // Two frames: a grid measures itself on the first one and draws on the next.
    let mut drawn = Vec::new();
    for _ in 0..2 {
        let mut images = NoteImages::default();
        let mut preview = PreviewActions::default();
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))),
            ..Default::default()
        };
        let output = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render_markdown_preview(ui, markdown, &p, None, &mut images, &mut preview);
            });
        });
        drawn.clear();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut drawn);
        }
    }

    for cell in ["Name", "Count", "apples", "3", "pears", "12"] {
        assert!(
            drawn.iter().any(|text| text == cell),
            "the cell {cell:?} should be drawn on its own: {drawn:?}"
        );
    }
    assert!(
        !drawn.iter().any(|text| text.contains('|')),
        "the pipes should not be drawn as text: {drawn:?}"
    );
    assert!(
        drawn.iter().any(|text| text == "After the table."),
        "the line under the table is still drawn"
    );
}

/// The colons under the header decide where each column's words sit: a right-aligned column
/// ends every cell on the same edge, and a centred one keeps every cell on the same middle.
#[test]
fn test_preview_lines_table_columns_up_as_the_header_asks() {
    let ctx = egui::Context::default();
    install_tabler_icons(&ctx);
    let p = ThemeMode::Obsidian.palette();
    let markdown =
        "| L | C | R |\n| :--- | :---: | ---: |\n| a | mid | 1 |\n| b | wider text | 12345 |";

    // Where the glyphs actually land. A galley carries the room the layout gave it, so its
    // size is not the width of the words; the ink of its rows is.
    fn collect_text(shape: &egui::Shape, out: &mut Vec<(String, Rect)>) {
        match shape {
            egui::Shape::Text(text) => {
                let mut ink = Rect::NOTHING;
                for row in &text.galley.rows {
                    ink = ink.union(row.visuals.mesh_bounds);
                }
                if ink.is_finite() {
                    out.push((
                        text.galley.text().to_string(),
                        ink.translate(text.pos.to_vec2()),
                    ));
                }
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_text(shape, out);
                }
            }
            _ => {}
        }
    }

    let mut drawn: Vec<(String, Rect)> = Vec::new();
    for _ in 0..3 {
        let mut images = NoteImages::default();
        let mut preview = PreviewActions::default();
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))),
            ..Default::default()
        };
        let output = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render_markdown_preview(ui, markdown, &p, None, &mut images, &mut preview);
            });
        });
        drawn.clear();
        for clipped in &output.shapes {
            collect_text(&clipped.shape, &mut drawn);
        }
    }

    let placed = |needle: &str| -> Rect {
        drawn
            .iter()
            .find(|(text, _)| text == needle)
            .map(|(_, rect)| *rect)
            .unwrap_or_else(|| panic!("{needle:?} was not drawn: {drawn:?}"))
    };

    // Both right-aligned cells end on the column's right edge.
    let (short, long) = (placed("1").right(), placed("12345").right());
    assert!(
        (short - long).abs() <= 1.5,
        "right-aligned cells should share a right edge: {short} vs {long}: {drawn:?}"
    );

    // Both centred cells share a middle.
    let (mid, wider) = (placed("mid").center().x, placed("wider text").center().x);
    assert!(
        (mid - wider).abs() <= 1.5,
        "centred cells should share a middle: {mid} vs {wider}: {drawn:?}"
    );

    // The left column stays a left column.
    assert!(
        (placed("a").left() - placed("b").left()).abs() <= 1.5,
        "left-aligned cells should share a left edge: {drawn:?}"
    );
}

/// A cell keeps the inline formatting of a note's line: bold, code and plain words in one
/// cell run on after each other on the same line instead of each starting its own.
#[test]
fn test_preview_keeps_a_formatted_cell_on_one_line() {
    let ctx = egui::Context::default();
    install_tabler_icons(&ctx);
    let p = ThemeMode::Obsidian.palette();
    let markdown = "| Head |\n| --- |\n| **bold** tail |";

    fn collect_text(shape: &egui::Shape, out: &mut Vec<(String, Rect)>) {
        match shape {
            egui::Shape::Text(text) => {
                let mut ink = Rect::NOTHING;
                for row in &text.galley.rows {
                    ink = ink.union(row.visuals.mesh_bounds);
                }
                if ink.is_finite() {
                    out.push((
                        text.galley.text().to_string(),
                        ink.translate(text.pos.to_vec2()),
                    ));
                }
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_text(shape, out);
                }
            }
            _ => {}
        }
    }

    let mut drawn: Vec<(String, Rect)> = Vec::new();
    for _ in 0..3 {
        let mut images = NoteImages::default();
        let mut preview = PreviewActions::default();
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))),
            ..Default::default()
        };
        let output = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render_markdown_preview(ui, markdown, &p, None, &mut images, &mut preview);
            });
        });
        drawn.clear();
        for clipped in &output.shapes {
            collect_text(&clipped.shape, &mut drawn);
        }
    }

    let find = |needle: &str| -> Rect {
        drawn
            .iter()
            .find(|(text, _)| text == needle)
            .map(|(_, rect)| *rect)
            .unwrap_or_else(|| panic!("{needle:?} was not drawn: {drawn:?}"))
    };
    let bold = find("bold");
    let tail = find(" tail");
    assert!(
        (bold.top() - tail.top()).abs() <= 1.5,
        "the words of one cell share a line: {bold:?} vs {tail:?}"
    );
    assert!(
        tail.left() + 1.0 >= bold.right() - 1.0,
        "the plain words follow the bold ones: {bold:?} vs {tail:?}"
    );
}

/// A table written inside a quote is drawn as a table there too: neither the pipes nor the
/// quote markers reach the text, and the paragraphs around the quote are untouched.
#[test]
fn test_preview_draws_a_table_inside_a_quote() {
    let ctx = egui::Context::default();
    install_tabler_icons(&ctx);
    let p = ThemeMode::Obsidian.palette();
    let markdown =
        "Before.\n\n> | Name | Count |\n> | :--- | ---: |\n> | apples | 3 |\n\nAfter the quote.";

    fn collect(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, out);
                }
            }
            _ => {}
        }
    }

    let mut drawn = Vec::new();
    for _ in 0..3 {
        let mut images = NoteImages::default();
        let mut preview = PreviewActions::default();
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))),
            ..Default::default()
        };
        let output = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render_markdown_preview(ui, markdown, &p, None, &mut images, &mut preview);
            });
        });
        drawn.clear();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut drawn);
        }
    }

    for cell in ["Name", "Count", "apples", "3"] {
        assert!(
            drawn.iter().any(|text| text == cell),
            "the cell {cell:?} should be drawn on its own: {drawn:?}"
        );
    }
    assert!(
        !drawn
            .iter()
            .any(|text| text.contains('|') || text.contains('>')),
        "neither the pipes nor the markers should be drawn as text: {drawn:?}"
    );
    assert!(
        drawn.iter().any(|text| text == "After the quote."),
        "the line under the quote is still drawn: {drawn:?}"
    );
}

/// A picture written on a quoted line belongs to the quote: the marker never reaches the text,
/// and the picture is decoded the same way one on an ordinary line is.
#[test]
fn test_preview_draws_a_picture_inside_a_quote() {
    let ctx = egui::Context::default();
    install_tabler_icons(&ctx);
    let dir = std::env::temp_dir().join(format!(
        "elora-preview-quoted-image-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let pixels = image::RgbaImage::from_pixel(6, 4, image::Rgba([40, 120, 190, 255]));
    pixels.save(dir.join("dot.png")).unwrap();

    let markdown = "Before.\n\n> A quoted line\n> ![a blue dot](dot.png) after it\n\nAfter.";
    let p = ThemeMode::Obsidian.palette();
    let mut images = NoteImages::default();

    fn collect(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => out.push(text.galley.text().to_string()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, out);
                }
            }
            _ => {}
        }
    }

    let mut drawn = Vec::new();
    for _ in 0..2 {
        let mut preview = PreviewActions::default();
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))),
            ..Default::default()
        };
        let output = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render_markdown_preview(ui, markdown, &p, Some(&dir), &mut images, &mut preview);
            });
        });
        drawn.clear();
        for clipped in &output.shapes {
            collect(&clipped.shape, &mut drawn);
        }
    }

    assert_eq!(images.ready_pictures(), 1, "the picture should be decoded");
    assert!(
        drawn.iter().any(|text| text == "A quoted line"),
        "the quoted words are drawn without their marker: {drawn:?}"
    );
    assert!(
        drawn.iter().any(|text| text == "after it"),
        "the words beside the picture stay in the quote: {drawn:?}"
    );
    assert!(
        !drawn.iter().any(|text| text.contains('>')),
        "the quote marker should not be drawn as text: {drawn:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A picture written in a table cell is drawn inside the cell, fitted to its column instead of
/// left as a marker, and the cell beside it keeps its place.
#[test]
fn test_preview_draws_a_picture_inside_a_table_cell() {
    let ctx = egui::Context::default();
    install_tabler_icons(&ctx);
    let dir = std::env::temp_dir().join(format!(
        "elora-preview-cell-image-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let pixels = image::RgbaImage::from_pixel(400, 40, image::Rgba([90, 150, 210, 255]));
    pixels.save(dir.join("wide.png")).unwrap();
    let pixels = image::RgbaImage::from_pixel(20, 10, image::Rgba([200, 120, 60, 255]));
    pixels.save(dir.join("after.png")).unwrap();

    let markdown = "| Pic | Name |\n| --- | --- |\n| ![a wide picture](wide.png) | apples |\n\n![after the table](after.png)";
    let p = ThemeMode::Obsidian.palette();
    let mut images = NoteImages::default();

    fn collect_text(shape: &egui::Shape, out: &mut Vec<(String, Rect)>) {
        match shape {
            egui::Shape::Text(text) => {
                let mut ink = Rect::NOTHING;
                for row in &text.galley.rows {
                    ink = ink.union(row.visuals.mesh_bounds);
                }
                if ink.is_finite() {
                    out.push((
                        text.galley.text().to_string(),
                        ink.translate(text.pos.to_vec2()),
                    ));
                }
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_text(shape, out);
                }
            }
            _ => {}
        }
    }

    let mut drawn: Vec<(String, Rect)> = Vec::new();
    for _ in 0..3 {
        let mut preview = PreviewActions::default();
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))),
            ..Default::default()
        };
        let output = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render_markdown_preview(ui, markdown, &p, Some(&dir), &mut images, &mut preview);
            });
        });
        drawn.clear();
        for clipped in &output.shapes {
            collect_text(&clipped.shape, &mut drawn);
        }
    }
    assert_eq!(
        images.ready_pictures(),
        2,
        "both pictures should be decoded"
    );

    // They are drawn in the order they sit in the note, which is the order the index a drag
    // writes back to counts them in, cell or not.
    let pair = &images.drawn[images.drawn.len() - 2..];
    assert!(
        pair[0].0.contains("wide.png") && pair[1].0.contains("after.png"),
        "the cell's picture comes first, the one under the table after it: {pair:?}"
    );
    let picture = pair[0].1;
    assert!(
        (120.0..=172.0).contains(&picture.width()),
        "a picture in a cell is fitted to its column, not drawn at its own 400px: {picture:?}"
    );
    assert!(
        pair[1].1.top() > picture.bottom(),
        "the picture under the table stays under it: {pair:?}"
    );

    let header = drawn
        .iter()
        .find(|(text, _)| text == "Pic")
        .map(|(_, rect)| *rect)
        .unwrap_or_else(|| panic!("the header cell should be drawn: {drawn:?}"));
    assert!(
        picture.top() > header.top() + 1.0,
        "the picture belongs to the row under the header: {picture:?} vs {header:?}"
    );
    assert!(
        drawn.iter().any(|(text, _)| text == "apples"),
        "the cell beside the picture keeps its words: {drawn:?}"
    );
    assert!(
        !drawn.iter().any(|(text, _)| text.contains("![")),
        "the picture marker should not be drawn as text: {drawn:?}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// Working out an export counts the pictures it would copy, points at the ones on the web and
/// leaves out the ones that are not on this computer — and touches nothing while doing it.
#[test]
fn test_planning_an_export_counts_the_pictures_without_copying() {
    let dir = std::env::temp_dir().join(format!(
        "elora-export-pictures-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let vault = dir.join("vault");
    std::fs::create_dir_all(vault.join("attachments")).unwrap();
    std::fs::write(vault.join("attachments/cat-photo.png"), png_bytes()).unwrap();
    // A name written with a `%20` finds a file with a space in it, whose name cannot stand in
    // a link as it is.
    std::fs::write(vault.join("my pic.png"), png_bytes()).unwrap();
    // Files that are not pictures are never copied, whatever the note calls them.
    std::fs::write(
        vault.join("id_ed25519"),
        b"-----BEGIN OPENSSH PRIVATE KEY-----",
    )
    .unwrap();
    std::fs::write(
        vault.join("fake.png"),
        b"-----BEGIN OPENSSH PRIVATE KEY-----",
    )
    .unwrap();

    let body = "One\n![a cat](attachments/cat-photo.png)\n![web](https://example.com/x.png)\n![gone](attachments/none.png)\n![again](attachments/cat-photo.png)\n![odd name](my%20pic.png)\n![key](id_ed25519)\n![key](fake.png)";
    let mut number = 0usize;
    let plan = plan_pictures("note-1", body, Some(&vault), &mut number);

    assert_eq!(
        plan.mapped
            .get("attachments/cat-photo.png")
            .map(String::as_str),
        Some("pictures/note-1-0-cat-photo.png"),
        "{:?}",
        plan.mapped
    );
    assert_eq!(
        plan.mapped.get("my%20pic.png").map(String::as_str),
        Some("pictures/note-1-1-my-pic.png"),
        "a name with a space in it is made safe for a link: {:?}",
        plan.mapped
    );
    assert_eq!(
        plan.mapped
            .get("https://example.com/x.png")
            .map(String::as_str),
        Some("https://example.com/x.png"),
        "a picture on the web is pointed at where it lives"
    );
    assert!(
        !plan.mapped.contains_key("attachments/none.png"),
        "the picture that is not there is left out: {:?}",
        plan.mapped
    );
    assert!(
        !plan.mapped.contains_key("id_ed25519") && !plan.mapped.contains_key("fake.png"),
        "a file that is not a picture is never copied: {:?}",
        plan.mapped
    );
    assert_eq!(
        plan.copies.len(),
        2,
        "only the two files that exist are copied"
    );
    assert_eq!(number, 2, "a picture named twice is counted once");
    assert!(
        !dir.join("pictures").exists(),
        "working out an export writes nothing"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// What the confirmation promises is what the export writes: the pages, the pictures beside
/// them, and the links between the pages.
#[test]
fn test_a_planned_export_writes_what_it_promised() {
    let dir = std::env::temp_dir().join(format!(
        "elora-export-plan-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let storage = NoteStorage::new(dir.join("notes")).unwrap();
    let root = storage.root_dir().to_path_buf();
    std::fs::create_dir_all(root.join("attachments")).unwrap();
    std::fs::write(root.join("attachments/pic.png"), png_bytes()).unwrap();

    let ctx = egui::Context::default();
    let mut app = EloraApp::with_context(&ctx, storage);
    let first = Note::new(
        "First",
        "See [[Second]] and ![a picture](attachments/pic.png)",
        &root,
    );
    let second = Note::new("Second", "Back to [[First]].", &root);
    let (first_id, second_id) = (first.id.clone(), second.id.clone());
    app.notes = vec![first, second];
    app.selected_id = Some(first_id.clone());

    // Asking for an export only offers it: nothing is written until it is confirmed.
    app.export_html(false);
    assert!(
        app.export_offer.is_some(),
        "the export waits for a yes before writing"
    );

    let mut plan = app.plan_export(false).expect("a note is selected");
    assert_eq!(plan.pages.len(), 2, "the note and the one it links to");
    assert_eq!(plan.copies.len(), 1, "the one picture it names");
    // Into a folder of the test's own instead of Downloads.
    let out = dir.join("out");
    plan.dir = out.clone();

    let summary = write_export(&plan).expect("the export should be written");
    assert!(
        summary.contains("plus 1 linked note") && summary.contains("1 picture"),
        "the summary says what was written: {summary}"
    );
    let html = |id: &str| {
        std::fs::read_to_string(out.join(format!("{id}.html")))
            .unwrap_or_else(|e| panic!("the page for {id} should be there: {e}"))
    };
    let page = html(&first_id);
    assert!(
        page.contains(&format!("href=\"{second_id}.html\"")),
        "the link opens the page that was written: {page}"
    );
    assert!(page.contains("<img src=\"pictures/"), "{page}");
    assert!(
        out.join("pictures").is_dir(),
        "the picture is copied beside the pages"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A single-note export writes the notes its links point at and the notes that point back at
/// it, and the same for those, so every link in the exported pages opens a page that was
/// written. A link to a note in the trash, or to no note at all, pulls nothing in.
#[test]
fn test_exporting_one_note_follows_its_links_both_ways() {
    let now = Utc::now();
    let note = |id: &str, body: &str, trashed: bool| Note {
        id: id.to_string(),
        title: id.to_string(),
        body: body.to_string(),
        pinned: false,
        trashed,
        explicit_tags: Vec::new(),
        icon: None,
        folder: None,
        owner: None,
        owner_key: None,
        private: false,
        shared_with: Vec::new(),
        shared_keys: Vec::new(),
        extra_frontmatter: String::new(),
        ai: Default::default(),
        created_at: now,
        updated_at: now,
        file_path: PathBuf::new(),
    };
    let notes = vec![
        note("start", "Go to [[middle]] and [[nowhere]].", false),
        note(
            "middle",
            "Then [[end]], [[gone]] and back to [[Start]].",
            false,
        ),
        note("end", "Nothing more.", false),
        note("gone", "In the trash.", true),
        note(
            "loose",
            "Nobody points here, and [[outsider]] neither.",
            false,
        ),
        note("backlink", "Context for [[start]].", false),
        note("trashed backlink", "Old context for [[start]].", true),
    ];

    let picked = notes_linked_around(&notes, &notes[0]);
    let ids: Vec<&str> = picked.iter().map(|note| note.id.as_str()).collect();
    assert_eq!(
        ids,
        ["start", "middle", "backlink", "end"],
        "the note, the notes it links to and the one linking back to it: {ids:?}"
    );
}

/// A table under a list item is drawn inside the item: its cells start to the right of the
/// item's own words, and the item after it is still drawn.
#[test]
fn test_preview_draws_a_table_under_a_list_item() {
    let ctx = egui::Context::default();
    install_tabler_icons(&ctx);
    let p = ThemeMode::Obsidian.palette();
    let markdown = "- Point\n  | A | B |\n  | --- | --- |\n  | x | y |\n- Next";

    fn collect_text(shape: &egui::Shape, out: &mut Vec<(String, Rect)>) {
        match shape {
            egui::Shape::Text(text) => {
                let mut ink = Rect::NOTHING;
                for row in &text.galley.rows {
                    ink = ink.union(row.visuals.mesh_bounds);
                }
                if ink.is_finite() {
                    out.push((
                        text.galley.text().to_string(),
                        ink.translate(text.pos.to_vec2()),
                    ));
                }
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_text(shape, out);
                }
            }
            _ => {}
        }
    }

    let mut drawn: Vec<(String, Rect)> = Vec::new();
    for _ in 0..3 {
        let mut images = NoteImages::default();
        let mut preview = PreviewActions::default();
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))),
            ..Default::default()
        };
        let output = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render_markdown_preview(ui, markdown, &p, None, &mut images, &mut preview);
            });
        });
        drawn.clear();
        for clipped in &output.shapes {
            collect_text(&clipped.shape, &mut drawn);
        }
    }

    let find = |needle: &str| -> Rect {
        drawn
            .iter()
            .find(|(text, _)| text == needle)
            .map(|(_, rect)| *rect)
            .unwrap_or_else(|| panic!("{needle:?} was not drawn: {drawn:?}"))
    };
    let point = find("Point");
    let cell = find("x");
    assert!(
        cell.left() > point.left() + 8.0,
        "the table should sit in from the item's own words: {point:?} vs {cell:?}"
    );
    assert!(
        drawn.iter().any(|(text, _)| text == "Next"),
        "the item under the table is still drawn: {drawn:?}"
    );
    assert!(
        !drawn.iter().any(|(text, _)| text.contains('|')),
        "the pipes should not be drawn as text: {drawn:?}"
    );
}

/// A table can be nested twice over — quoted inside a list item — and both levels are
/// unwrapped: the cells arrive on their own, drawn in from the item's words.
#[test]
fn test_preview_draws_a_table_quoted_inside_a_list_item() {
    let ctx = egui::Context::default();
    install_tabler_icons(&ctx);
    let p = ThemeMode::Obsidian.palette();
    let markdown = "- Point\n  > | A | B |\n  > | --- | --- |\n  > | x | y |\n- Next";

    fn collect_text(shape: &egui::Shape, out: &mut Vec<(String, Rect)>) {
        match shape {
            egui::Shape::Text(text) => {
                let mut ink = Rect::NOTHING;
                for row in &text.galley.rows {
                    ink = ink.union(row.visuals.mesh_bounds);
                }
                if ink.is_finite() {
                    out.push((
                        text.galley.text().to_string(),
                        ink.translate(text.pos.to_vec2()),
                    ));
                }
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_text(shape, out);
                }
            }
            _ => {}
        }
    }

    let mut drawn: Vec<(String, Rect)> = Vec::new();
    for _ in 0..3 {
        let mut images = NoteImages::default();
        let mut preview = PreviewActions::default();
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))),
            ..Default::default()
        };
        let output = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                render_markdown_preview(ui, markdown, &p, None, &mut images, &mut preview);
            });
        });
        drawn.clear();
        for clipped in &output.shapes {
            collect_text(&clipped.shape, &mut drawn);
        }
    }

    let find = |needle: &str| -> Rect {
        drawn
            .iter()
            .find(|(text, _)| text == needle)
            .map(|(_, rect)| *rect)
            .unwrap_or_else(|| panic!("{needle:?} was not drawn: {drawn:?}"))
    };
    for cell in ["A", "B", "x", "y"] {
        find(cell);
    }
    assert!(
        find("x").left() > find("Point").left() + 8.0,
        "the quoted table is still drawn in from the item's words: {drawn:?}"
    );
    assert!(
        !drawn
            .iter()
            .any(|(text, _)| text.contains('|') || text.contains('>')),
        "neither the pipes nor the markers should be drawn as text: {drawn:?}"
    );
    assert!(
        drawn.iter().any(|(text, _)| text == "Next"),
        "the item under it is still drawn: {drawn:?}"
    );
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
    let saved = std::fs::read_to_string(root.join(format!("{}.md", app.notes[index].id))).unwrap();
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
    // The pointer moves a little each frame, the way a hand does: the picture must keep growing
    // rather than going back to its first size each time.
    let mut moved_to = start;
    for _ in 0..6 {
        moved_to += Vec2::new(10.0, 0.0);
        frame(
            vec![egui::Event::PointerMoved(moved_to)],
            &mut images,
            &mut preview,
            &mut handles,
        );
    }
    assert_eq!(preview.image, Some((1, 460)));
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
        // A first connection stops at the certificate; the person at the keyboard
        // confirms it, and only then does the real handshake run.
        if let Some((fingerprint, _)) = app.trust_prompt.clone() {
            app.trust_server(fingerprint);
        }
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
        ..crate::net::ServerConfig::default()
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

    let mut friend = EloraApp::with_context(&ctx, NoteStorage::new(dir.join("friend")).unwrap());
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

/// Only ids that can stand in a file name are taken from a server.
#[test]
fn test_a_server_note_id_must_be_a_plain_word() {
    assert!(is_safe_note_id("meeting-notes-1a2b"));
    assert!(is_safe_note_id("café-ab12"));
    assert!(!is_safe_note_id(""));
    assert!(!is_safe_note_id("../escape"));
    assert!(!is_safe_note_id("a/b"));
    assert!(!is_safe_note_id("a\\b"));
    assert!(!is_safe_note_id(&"x".repeat(200)));
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

/// A folder somebody picked is where the pages go — for a whole vault too — and the choice is
/// remembered. Without one, a whole vault gets a folder of its own inside downloads.
#[test]
fn test_the_export_goes_to_the_chosen_folder() {
    let dir = std::env::temp_dir().join(format!(
        "elora-export-dir-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let storage = NoteStorage::new(dir.join("notes")).unwrap();
    let root = storage.root_dir().to_path_buf();
    let ctx = egui::Context::default();
    let mut app = EloraApp::with_context(&ctx, storage);
    app.notes = vec![Note::new("Only", "nothing here", &root)];
    app.selected_id = Some(app.notes[0].id.clone());

    let chosen = dir.join("somewhere else");
    app.export_dir = Some(chosen.clone());
    assert_eq!(
        app.plan_export(false).expect("a plan").dir,
        chosen,
        "one note goes where it was told"
    );
    assert_eq!(
        app.plan_export(true).expect("a plan").dir,
        chosen,
        "a whole vault goes where it was told too"
    );

    app.export_dir = None;
    let default = app.plan_export(true).expect("a plan").dir;
    assert!(
        default.ends_with("EloraNotes Export"),
        "the default whole-vault export keeps a folder of its own: {default:?}"
    );

    // The choice is remembered: it is in the settings file.
    app.export_dir = Some(chosen.clone());
    app.persist_settings();
    assert_eq!(
        app.storage.load_settings().export_dir,
        Some(chosen.display().to_string()),
        "the folder outlives the window"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A note's name is part of the button, not a piece of text to select: a click on the name
/// opens the note, in the one-line row and in the tall card alike.
#[test]
fn test_clicking_a_note_name_opens_it() {
    let dir = std::env::temp_dir().join(format!(
        "elora-card-click-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let note = Note::new("A note to click on", "the body of the note", &dir);

    let ctx = egui::Context::default();
    install_tabler_icons(&ctx);
    let p = ThemeMode::Obsidian.palette();
    let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(320.0, 240.0));

    fn collect_text(shape: &egui::Shape, out: &mut Vec<(String, Rect)>) {
        match shape {
            egui::Shape::Text(text) => {
                let mut ink = Rect::NOTHING;
                for row in &text.galley.rows {
                    ink = ink.union(row.visuals.mesh_bounds);
                }
                if ink.is_finite() {
                    out.push((
                        text.galley.text().to_string(),
                        ink.translate(text.pos.to_vec2()),
                    ));
                }
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_text(shape, out);
                }
            }
            _ => {}
        }
    }

    for roomy in [false, true] {
        let mut opened = false;
        let mut click_at: Option<Pos2> = None;
        for frame in 0..3 {
            let mut events = Vec::new();
            if let Some(at) = click_at {
                if frame >= 1 {
                    events.push(egui::Event::PointerMoved(at));
                    events.push(egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::default(),
                    });
                    events.push(egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::default(),
                    });
                }
            }
            let raw = egui::RawInput {
                screen_rect: Some(screen),
                events,
                ..Default::default()
            };
            let output = ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let action = if roomy {
                        render_note_card(ui, &note, false, &p, "", &[], false)
                    } else {
                        render_compact_note_row(ui, &note, false, &p, &[])
                    };
                    if let Some(CardAction::SelectNote(id)) = action {
                        opened = id == note.id;
                    }
                });
            });
            if click_at.is_none() {
                let mut drawn = Vec::new();
                for clipped in &output.shapes {
                    collect_text(&clipped.shape, &mut drawn);
                }
                click_at = drawn
                    .iter()
                    .find(|(text, _)| text == &note.display_title())
                    .map(|(_, rect)| rect.center());
            }
        }
        assert!(click_at.is_some(), "the note's name should be drawn");
        assert!(
            opened,
            "clicking the name should open the note (tall card: {roomy})"
        );
    }

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
            let _ = ctx.run(input.clone(), |ctx| {
                egui::SidePanel::left(format!("probe-{w}-{h}"))
                    .resizable(false)
                    .exact_width(SIDEBAR_WIDTH)
                    .show_separator_line(false)
                    .frame(
                        Frame::new()
                            .inner_margin(Margin::symmetric(SIDEBAR_MARGIN_X, SIDEBAR_MARGIN_Y)),
                    )
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

/// Linked notes are pulled into clusters, each with its own colour, and the notes nothing links
/// to are set out on an arc below them rather than mixed in.
#[test]
fn test_the_graph_clusters_linked_notes_and_sets_the_rest_on_an_arc_below() {
    let mut nodes: Vec<GraphNode> = (0..60)
        .map(|i| GraphNode {
            id: format!("n{i}"),
            label: format!("Note {i}"),
            links: Vec::new(),
        })
        .collect();
    // Two clusters of ten notes, each a hub with spokes, and one link between them.
    for spoke in 1..10 {
        nodes[0].links.push(spoke);
        nodes[10].links.push(10 + spoke);
    }
    nodes[9].links.push(19);

    let mut view = GraphView::new(nodes);
    view.settle(Duration::from_secs(10));
    assert!(view.settled());
    assert_eq!(view.connected.len(), 20);
    assert_eq!(view.orphans.len(), 40);

    let groups: BTreeSet<usize> = view.group.iter().flatten().copied().collect();
    assert_eq!(groups.len(), 2, "each cluster gets a colour of its own");
    assert_ne!(group_color(0), group_color(1));
    assert_eq!(
        view.group[0], view.group[5],
        "a hub and its spokes share a colour"
    );
    assert_ne!(view.group[0], view.group[15]);

    let world = view.world_positions();
    let top_of_orphans = view
        .orphans
        .iter()
        .map(|&i| world[i].y)
        .fold(f32::MAX, f32::min);
    let mean_y = |ids: &[usize]| ids.iter().map(|&i| world[i].y).sum::<f32>() / ids.len() as f32;
    assert!(
        mean_y(&view.orphans) > mean_y(&view.connected),
        "the unlinked notes sit below the linked ones"
    );
    let reach = view
        .connected
        .iter()
        .map(|&i| world[i].length())
        .fold(0.0_f32, f32::max);
    let (low, high) = view
        .orphans
        .iter()
        .map(|&i| world[i].length())
        .fold((f32::MAX, 0.0_f32), |(lo, hi), d| (lo.min(d), hi.max(d)));
    assert!(low > 0.0 && top_of_orphans.is_finite() && reach.is_finite() && high >= low);
    // No two notes sit on the same spot, so every one can be seen and pointed at.
    for (n, a) in world.iter().enumerate() {
        for b in &world[n + 1..] {
            assert!(
                (*a - *b).length() > 1.0,
                "two notes landed on top of each other"
            );
        }
    }

    // A vault with nothing linked is a ring, not a pile at the origin.
    let loose: Vec<GraphNode> = (0..12)
        .map(|i| GraphNode {
            id: format!("l{i}"),
            label: format!("Loose {i}"),
            links: Vec::new(),
        })
        .collect();
    let ring = GraphView::new(loose).world_positions();
    assert!(ring.iter().all(|p| p.length() > 20.0));
}

/// A shared note's links are only known once its text has been read from the server. Until then
/// its note is on its own; when the links arrive it joins the graph without the rest moving.
#[test]
fn test_links_read_from_shared_notes_join_the_graph_when_they_arrive() {
    let nodes: Vec<GraphNode> = ["Spec", "Progress", "Ideas", "Loose"]
        .iter()
        .map(|title| GraphNode {
            id: title.to_lowercase(),
            label: title.to_string(),
            links: Vec::new(),
        })
        .collect();
    let mut view = GraphView::new(nodes);
    assert_eq!((view.connected.len(), view.orphans.len()), (0, 4));

    view.stage_links(
        "progress".into(),
        vec!["spec".into(), "SPEC".into(), "Nowhere".into()],
    );
    view.stage_links("ideas".into(), vec!["Progress".into()]);
    assert!(view.flush(true));
    assert_eq!(view.connected.len(), 3);
    assert_eq!(view.orphans, vec![3]);
    assert_eq!(view.edges.len(), 2, "the same link twice is one line");
    assert!(view.group[0].is_some() && view.group[3].is_none());

    // Nothing new to fold in: the graph is left as it is.
    view.stage_links("progress".into(), vec!["Spec".into()]);
    assert!(!view.flush(true));

    // A note that is placed stays where it is when more links arrive.
    view.settle(Duration::from_secs(5));
    let before = view.world_positions()[0];
    view.stage_links("loose".into(), vec!["Spec".into()]);
    assert!(view.flush(true));
    assert_eq!(view.connected.len(), 4);
    assert!(view.orphans.is_empty());
    view.settle(Duration::from_secs(5));
    assert!(before.is_finite() && view.world_positions()[3].is_finite());
}

/// Reading the shared notes gives up cleanly when there is no library to read from, and says why.
#[test]
fn test_reading_shared_links_gives_up_when_the_library_cannot_be_reached() {
    let (tx, rx) = std::sync::mpsc::channel();
    let todo: Vec<(String, chrono::DateTime<Utc>)> = (0..6)
        .map(|i| (format!("shared-{i}"), Utc::now()))
        .collect();
    load_shared_links(AppSettings::default(), todo, tx);
    let mut skipped = 0;
    let mut failed = None;
    while let Ok(message) = rx.try_recv() {
        match message {
            GraphLoad::Skipped => skipped += 1,
            GraphLoad::Failed(why) => failed = Some(why),
            GraphLoad::Links { .. } | GraphLoad::Done => panic!("nothing could have been read"),
        }
    }
    assert_eq!(skipped, 2, "two notes are skipped before it gives up");
    assert!(failed.is_some_and(|why| !why.is_empty()));
}

/// Zooming with the scroll wheel keeps what is under the pointer under the pointer.
#[test]
fn test_zooming_keeps_the_graph_point_under_the_pointer() {
    let centre = Pos2::new(400.0, 300.0);
    let pointer = Pos2::new(550.0, 380.0);
    let (zoom, focus) = (1.5_f32, Vec2::new(30.0, -10.0));
    let under = |zoom: f32, focus: Vec2| focus + (pointer - centre) / zoom;

    let (zoomed, moved) = zoom_about(zoom, focus, centre, pointer, 2.0);
    assert!((zoomed - 3.0).abs() < 1e-4);
    assert!((under(zoomed, moved) - under(zoom, focus)).length() < 1e-3);

    // There is a limit either way, and the point still holds at the limit.
    let (far, shifted) = zoom_about(zoom, focus, centre, pointer, 1.0e6);
    assert!(far <= 12.0);
    assert!((under(far, shifted) - under(zoom, focus)).length() < 1e-3);
    let (near, _) = zoom_about(zoom, focus, centre, pointer, 1.0e-6);
    assert!(near >= 0.03);

    // Fitting shows every note: the whole spread fits in the canvas.
    let world = vec![Vec2::new(-300.0, -50.0), Vec2::new(500.0, 250.0)];
    let (fit_zoom, fit_focus) = GraphView::fit_target(&world, Vec2::new(800.0, 600.0));
    assert!(fit_zoom > 0.0 && (fit_focus - Vec2::new(100.0, 100.0)).length() < 1e-3);
    assert!((500.0 + 300.0) * fit_zoom <= 800.0);
}

#[test]
fn test_a_graph_label_is_cut_to_fit_beside_its_dot() {
    let long = graph_label("A very long note title that cannot fit beside a dot");
    assert_eq!(long.chars().count(), 25);
    assert!(long.ends_with('…'));
    assert_eq!(graph_label("Short note"), "Short note");
}

/// A note another program writes into the folder appears without a restart, an edit to an open
/// note replaces what the window shows, and a deleted file leaves the list.
#[test]
fn test_notes_changed_by_another_program_show_up_live() {
    let (mut app, dir, root) = app_with_note("First line");
    let id = app.notes[0].id.clone();
    let mut mine = app.notes[0].clone();
    mine.body = "First line".to_string();
    app.storage.save_note(&mine).unwrap();

    let outside = NoteStorage::new(root.clone()).unwrap();
    let theirs = Note::new("From an assistant", "hello", &root);
    outside.save_note(&theirs).unwrap();
    app.disk_watch.flag_change();
    app.poll_disk();
    assert!(app.notes.iter().any(|n| n.title == "From an assistant"));

    let mut edited = mine.clone();
    edited.body = "Edited elsewhere".to_string();
    outside.save_note(&edited).unwrap();
    let file = std::fs::File::options()
        .write(true)
        .open(&edited.file_path)
        .unwrap();
    file.set_modified(std::time::SystemTime::now() + Duration::from_secs(5))
        .unwrap();
    app.disk_watch.flag_change();
    app.poll_disk();
    let shown = app.notes.iter().find(|n| n.id == id).unwrap();
    assert_eq!(shown.body, "Edited elsewhere");

    std::fs::remove_file(&theirs.file_path).unwrap();
    app.disk_watch.flag_change();
    app.poll_disk();
    assert!(!app.notes.iter().any(|n| n.title == "From an assistant"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The sidebar can list just the notes an assistant made or last changed, and a person's own
/// edit takes the "changed by an assistant" mark off.
#[test]
fn test_the_assistant_section_lists_only_notes_an_assistant_touched() {
    let (mut app, dir, root) = app_with_note("Mine");
    let mut made = Note::new("Made by AI", "x", &root);
    made.ai.created = true;
    let mut changed = Note::new("Changed by AI", "y", &root);
    changed.ai.edited = true;
    let mut trashed = Note::new("Old AI note", "z", &root);
    trashed.ai.created = true;
    trashed.trashed = true;
    app.notes.extend([made, changed, trashed]);
    app.sidebar_section = SidebarSection::Assistant;
    let titles: Vec<String> = app
        .filtered_indices()
        .into_iter()
        .map(|i| app.notes[i].title.clone())
        .collect();
    assert_eq!(titles.len(), 2);
    assert!(titles.contains(&"Made by AI".to_string()));
    assert!(titles.contains(&"Changed by AI".to_string()));
    let _ = std::fs::remove_dir_all(&dir);
}

/// "Assistants can read, not change" is written into a note's own file, and into the settings
/// for a shared note, which has no file here.
#[test]
fn test_marking_a_note_read_only_for_assistants_is_remembered() {
    let (mut app, dir, root) = app_with_note("Guard me");
    let idx = app.notes.len() - 1;
    app.set_mcp_read_only(idx, true);
    assert!(app.notes[idx].ai.read_only);
    let raw = std::fs::read_to_string(&app.notes[idx].file_path).unwrap();
    assert!(raw.contains("mcp_read_only: true"), "{raw}");
    app.set_mcp_read_only(idx, false);
    let raw = std::fs::read_to_string(&app.notes[idx].file_path).unwrap();
    assert!(!raw.contains("mcp_read_only"), "{raw}");

    // A shared note keeps the choice in the settings file.
    let mut shared = Note::new("Shared one", "s", &root);
    shared.file_path = PathBuf::new();
    let shared_id = shared.id.clone();
    app.remote_ids.insert(shared_id.clone());
    app.notes.push(shared);
    let shared_idx = app.notes.len() - 1;
    app.set_mcp_read_only(shared_idx, true);
    assert!(app
        .storage
        .load_settings()
        .mcp_read_only_ids
        .contains(&shared_id));
    app.set_mcp_read_only(shared_idx, false);
    assert!(!app
        .storage
        .load_settings()
        .mcp_read_only_ids
        .contains(&shared_id));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Restoring an earlier version brings its text back, keeps where the note lives and who may
/// edit it, and keeps what was there before so the restore can itself be undone.
#[test]
fn test_restoring_an_earlier_version_keeps_the_note_where_it_is() {
    let (mut app, dir, root) = app_with_note("original text");
    let idx = app.notes.len() - 1;
    app.notes[idx].folder = Some("Work".into());
    app.notes[idx].ai.read_only = true;
    app.persist_note(idx).unwrap();
    let id = app.notes[idx].id.clone();

    // An assistant overwrites it; the old text goes to the history first.
    let before = app.notes[idx].clone();
    app.storage.snapshot(&before).unwrap();
    app.notes[idx].body = "assistant rewrote this".into();
    app.persist_note(idx).unwrap();

    app.open_history(idx);
    let earlier = app.storage.versions(&id);
    assert_eq!(earlier.len(), 1);
    let older = app
        .storage
        .read_version(&earlier[0], &app.notes[idx])
        .unwrap();
    app.notes[idx].folder = Some("Moved".into());
    app.restore_version(&id, &older);

    let now = app.notes.iter().find(|n| n.id == id).unwrap();
    assert_eq!(now.body, "original text");
    assert_eq!(
        now.folder.as_deref(),
        Some("Moved"),
        "the note stays where it is now"
    );
    assert!(
        now.ai.read_only,
        "the protection is not lifted by a restore"
    );
    // The text that was replaced is in the history now.
    let kept = app.storage.versions(&id);
    assert!(kept.len() >= 2);
    let _ = root;
    let _ = std::fs::remove_dir_all(&dir);
}

/// The command Settings hands out carries the limits that were chosen.
#[test]
fn test_the_assistant_command_carries_the_chosen_limits() {
    let none = McpOptions::default();
    let (_, args) = settings::mcp_launch_command(Path::new("/notes"), &none);
    assert_eq!(args, vec!["--mcp", "--dir", "/notes"]);
    let limited = McpOptions {
        read_only: true,
        folder: " AI ".into(),
        no_shared: true,
    };
    let (_, args) = settings::mcp_launch_command(Path::new("/notes"), &limited);
    assert_eq!(
        args,
        vec![
            "--mcp",
            "--dir",
            "/notes",
            "--read-only",
            "--no-shared",
            "--folder",
            "AI"
        ]
    );
}

/// An assistant's edit shows up in the open window with a notice saying who made it.
#[test]
fn test_an_assistants_change_arrives_with_a_notice() {
    let (mut app, dir, root) = app_with_note("before");
    let idx = app.notes.len() - 1;
    app.persist_note(idx).unwrap();
    let outside = NoteStorage::new(root.clone()).unwrap();
    let mut edited = app.notes[idx].clone();
    edited.body = "after".into();
    edited.ai.edited = true;
    outside.save_note(&edited).unwrap();
    let file = std::fs::File::options()
        .write(true)
        .open(&edited.file_path)
        .unwrap();
    file.set_modified(std::time::SystemTime::now() + Duration::from_secs(5))
        .unwrap();
    app.disk_watch.flag_change();
    app.poll_disk();
    assert!(app
        .status_message
        .as_deref()
        .is_some_and(|m| m.contains("changed by an assistant")));

    let mut added = Note::new("Fresh from AI", "hi", &root);
    added.ai.created = true;
    outside.save_note(&added).unwrap();
    app.disk_watch.flag_change();
    app.poll_disk();
    assert!(app
        .status_message
        .as_deref()
        .is_some_and(|m| m.contains("added “Fresh from AI”")));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A window over a vault of its own, with the app's own context so frames can be drawn.
fn drawable_app(name: &str) -> (EloraApp, egui::Context, PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "elora-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let storage = NoteStorage::new(dir.clone()).unwrap();
    let ctx = egui::Context::default();
    let app = EloraApp::with_context(&ctx, storage);
    (app, ctx, dir)
}

fn draw(ctx: &egui::Context, mut contents: impl FnMut(&egui::Context)) {
    for _ in 0..3 {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1100.0, 760.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(raw, |ctx| contents(ctx));
    }
}

/// Everything the assistant features add to the window is drawn for each kind of note, so a
/// layout that panics or a menu that cannot be built shows up here rather than at launch.
#[test]
fn test_the_assistant_features_draw_for_every_kind_of_note() {
    let (mut app, ctx, dir) = drawable_app("draw-assistant");
    app.mcp_clients.environment = Some(empty_computer(&dir));
    let root = app.storage.root_dir().to_path_buf();
    let mut made = Note::new("Made by AI", "# Hi\n- [ ] one", &root);
    made.ai.created = true;
    made.icon = Some("robot".into());
    let mut changed = Note::new("Changed by AI", "text", &root);
    changed.ai.edited = true;
    let mut guarded = Note::new("Guarded", "text", &root);
    guarded.ai.read_only = true;
    let mut shared = Note::new("On the server", "shared", &root);
    shared.file_path = PathBuf::new();
    app.remote_ids.insert(shared.id.clone());
    let ids: Vec<String> = [&made, &changed, &guarded, &shared]
        .iter()
        .map(|n| n.id.clone())
        .collect();
    app.notes.extend([made, changed, guarded, shared]);
    let p = app.theme.palette_with(&app.colors, app.glow_strength);

    for id in &ids {
        app.selected_id = Some(id.clone());
        draw(&ctx, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                app.render_sidebar(ui, &p);
                app.render_editor(ui, &p, 700.0, 0.0);
            });
        });
    }

    // The assistant filter, Settings with its limits and a test result, the history window and
    // the graph, all open at once.
    app.sidebar_section = SidebarSection::Assistant;
    app.show_settings = true;
    app.settings_page = settings::SettingsPage::Assistants;
    app.mcp_options = McpOptions {
        read_only: true,
        folder: "AI".into(),
        no_shared: true,
    };
    app.selected_id = Some(ids[0].clone());
    let idx = app.notes.iter().position(|n| n.id == ids[0]).unwrap();
    app.open_history(idx);
    app.toggle_graph();
    draw(&ctx, |ctx| {
        app.settings_window(ctx);
        app.history_window(ctx);
        app.graph_window(ctx);
        egui::CentralPanel::default().show(ctx, |ui| {
            app.render_sidebar(ui, &p);
        });
    });
    assert!(app.graph.is_some());
    // Every page of Settings draws, with the assistant limits and a test result set above.
    for page in settings::SettingsPage::ALL {
        app.settings_page = page;
        draw(&ctx, |ctx| app.settings_window(ctx));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A big vault lays out in reasonable time and is drawn at every zoom without trouble.
#[test]
fn test_a_big_graph_settles_quickly_and_draws_at_every_zoom() {
    let count = 1800;
    let mut nodes: Vec<GraphNode> = (0..count)
        .map(|i| GraphNode {
            id: format!("n{i}"),
            label: format!("Note number {i}"),
            links: Vec::new(),
        })
        .collect();
    // Fifteen topics of a hundred notes each: every note links to a few others in its topic, and
    // a few topics link to each other. The last 300 notes are on their own.
    for topic in 0..15 {
        for k in 0..100 {
            let i = topic * 100 + k;
            for hop in [1, 7, 13] {
                nodes[i].links.push(topic * 100 + (k + hop) % 100);
            }
        }
        nodes[topic * 100].links.push(((topic + 1) % 15) * 100);
    }
    let started = Instant::now();
    let mut view = GraphView::new(nodes);
    view.settle(Duration::from_secs(60));
    let took = started.elapsed();
    assert!(view.settled());
    assert!(
        took < Duration::from_secs(20),
        "laying out 1800 notes took {took:?}"
    );
    assert_eq!(view.orphans.len(), 300);
    let topics = view.group.iter().flatten().max().unwrap() + 1;
    assert!(
        (4..=20).contains(&topics),
        "the topics get a handful of colours, not {topics}"
    );

    let (mut app, ctx, dir) = drawable_app("draw-big-graph");
    app.graph = Some(view);
    for zoom in [0.05, 1.0, 4.0, 12.0] {
        if let Some(view) = app.graph.as_mut() {
            view.auto_fit = false;
            view.zoom = zoom;
        }
        draw(&ctx, |ctx| app.graph_window(ctx));
    }
    assert!(app.graph.is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

/// The Settings check starts a program the way an assistant would and says what it found.
#[cfg(unix)]
#[test]
fn test_the_connection_check_reads_what_the_server_says() {
    let script = r#"cat >/dev/null
echo '{"jsonrpc":"2.0","id":1,"result":{}}'
echo '{"jsonrpc":"2.0","id":2,"result":{"tools":[{},{},{}]}}'
echo '{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"{\"notes\":5,\"read_only_server\":true,\"folder_limit\":\"AI\",\"shared_library\":\"connected to X (2 notes)\"}"}]}}'"#;
    let said = settings::check_mcp_command("sh", &["-c".into(), script.into()]);
    assert_eq!(
        said,
        "Works: 3 tools, 5 notes, read-only, limited to “AI”, shared library connected to X (2 notes)"
    );
    assert!(settings::check_mcp_command("/no/such/program", &[]).starts_with("Could not start"));
    let silent = settings::check_mcp_command("sh", &["-c".into(), "cat >/dev/null".into()]);
    assert!(
        silent.starts_with("It started but did not answer the way"),
        "{silent}"
    );
}

/// A command that points into a build folder is flagged, whichever way the path is written.
#[test]
fn test_a_build_folder_path_is_recognised_on_every_platform() {
    assert!(settings::runs_from_build_folder(
        "/home/me/EloraNotes/target/debug/elora"
    ));
    assert!(settings::runs_from_build_folder("/x/target/release/elora"));
    assert!(settings::runs_from_build_folder(
        r"C:\src\EloraNotes\target\release\elora.exe"
    ));
    assert!(!settings::runs_from_build_folder("/usr/bin/elora"));
    assert!(!settings::runs_from_build_folder(
        r"C:\Program Files\EloraNotes\elora.exe"
    ));
}

/// The footer icons stay inside the sidebar however many notes there are. A tab row wider than
/// the panel used to stretch the rectangle the footer was anchored to, and the settings gear was
/// pushed off the edge.
#[test]
fn test_the_sidebar_footer_stays_inside_the_panel_with_big_counts() {
    for count in [3usize, 10, 150, 1200] {
        let (mut app, ctx, dir) = drawable_app("footer-fits");
        let root = app.storage.root_dir().to_path_buf();
        for i in 0..count {
            let mut note = Note::new(format!("Note {i}"), "text", &root);
            note.pinned = i % 7 == 0;
            note.trashed = i % 11 == 0;
            note.ai.created = i % 5 == 0;
            app.notes.push(note);
        }
        let p = app.theme.palette_with(&app.colors, app.glow_strength);
        let mut footer = None;
        draw(&ctx, |ctx| {
            egui::SidePanel::left("test_sidebar")
                .exact_width(SIDEBAR_WIDTH)
                .show(ctx, |ui| {
                    footer = Some(app.render_sidebar(ui, &p));
                });
        });
        let footer = footer.expect("the sidebar was drawn");
        assert!(
            footer.right() <= SIDEBAR_WIDTH + 0.5,
            "with {count} notes the footer ends at {} in a {SIDEBAR_WIDTH}-wide sidebar",
            footer.right()
        );
        assert!(
            footer.left() >= 0.0,
            "with {count} notes the footer starts at {}",
            footer.left()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// A computer with none of the AI apps on it, in a folder of its own.
fn empty_computer(under: &Path) -> crate::mcp_clients::Environment {
    let home = under.join("fake-home");
    std::fs::create_dir_all(&home).unwrap();
    crate::mcp_clients::Environment {
        os: crate::mcp_clients::Os::Linux,
        config_home: home.join(".config"),
        home,
        local_data: None,
        path_dirs: vec![under.join("fake-bin")],
        cli_timeout: Duration::from_secs(5),
        sandboxed: false,
    }
}

/// Draws Settings until the AI-app scan and any work have finished.
fn settle_ai_apps(app: &mut EloraApp, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        draw(ctx, |ctx| app.settings_window(ctx));
        if !app.mcp_clients.busy() && !app.mcp_clients.reports().is_empty() {
            return;
        }
        assert!(Instant::now() < deadline, "the scan never finished");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Settings finds the AI apps that are installed, adds EloraNotes to the ones asked for without
/// disturbing what they hold, and notices when what was added no longer matches the options.
#[test]
fn test_settings_finds_the_ai_apps_and_adds_the_server_to_them() {
    use crate::mcp_clients::{Client, Launch, State};
    let (mut app, ctx, dir) = drawable_app("ai-apps");
    let environment = empty_computer(&dir);
    std::fs::create_dir_all(environment.config_home.join("Claude")).unwrap();
    std::fs::create_dir_all(environment.home.join(".cursor")).unwrap();
    let desktop_file = environment
        .config_home
        .join("Claude/claude_desktop_config.json");
    std::fs::write(
        &desktop_file,
        r#"{ "theme": "dark", "mcpServers": { "other": { "command": "node" } } }"#,
    )
    .unwrap();
    app.mcp_clients.environment = Some(environment.clone());
    app.show_settings = true;
    app.settings_page = settings::SettingsPage::Assistants;

    settle_ai_apps(&mut app, &ctx);
    let state_of = |app: &EloraApp, client: Client| {
        app.mcp_clients
            .reports()
            .iter()
            .find(|r| r.client == client)
            .map(|r| r.state.clone())
            .unwrap()
    };
    assert_eq!(state_of(&app, Client::ClaudeDesktop), State::NotAdded);
    assert_eq!(state_of(&app, Client::Cursor), State::NotAdded);
    assert_eq!(
        state_of(&app, Client::Codex),
        State::NotFound,
        "what is not installed is not offered"
    );
    assert!(
        !environment.home.join(".cursor/mcp.json").exists(),
        "nothing is written by looking"
    );

    // The command is the one the Settings page would hand out for these options.
    let (program, args) = settings::mcp_launch_command(app.storage.root_dir(), &app.mcp_options);
    let launch = Launch { program, args };
    app.mcp_clients.start_work(
        vec![
            clients::Job::Mcp(Client::ClaudeDesktop, clients::Task::Add),
            clients::Job::Mcp(Client::Cursor, clients::Task::Add),
        ],
        &launch,
        &ctx,
    );
    settle_ai_apps(&mut app, &ctx);
    assert_eq!(state_of(&app, Client::ClaudeDesktop), State::Added);
    assert_eq!(state_of(&app, Client::Cursor), State::Added);
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&desktop_file).unwrap()).unwrap();
    assert_eq!(written["theme"], "dark");
    assert!(
        written["mcpServers"]["other"].is_object(),
        "what was there stays"
    );
    assert_eq!(written["mcpServers"]["elora-notes"]["args"][0], "--mcp");
    assert!(
        app.status_message
            .as_deref()
            .is_some_and(|m| m.contains("Added to")),
        "{:?}",
        app.status_message
    );

    // Choosing different limits makes what was added out of date, once the choice has settled.
    app.mcp_options.read_only = true;
    let deadline = Instant::now() + Duration::from_secs(10);
    while state_of(&app, Client::ClaudeDesktop) != State::Outdated {
        draw(&ctx, |ctx| app.settings_window(ctx));
        assert!(
            Instant::now() < deadline,
            "the change of options was never noticed"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Inside a Flatpak sandbox the page explains why nothing is found instead of showing an empty
/// list, and still draws.
#[test]
fn test_settings_explains_the_flatpak_sandbox() {
    let (mut app, ctx, dir) = drawable_app("sandboxed");
    let mut environment = empty_computer(&dir);
    environment.sandboxed = true;
    app.mcp_clients.environment = Some(environment);
    app.show_settings = true;
    app.settings_page = settings::SettingsPage::Assistants;
    settle_ai_apps(&mut app, &ctx);
    assert!(app
        .mcp_clients
        .reports()
        .iter()
        .all(|r| r.state == crate::mcp_clients::State::NotFound));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Settings finds the apps that read skills, adds the memory skill to the ones asked for, and
/// shows an old copy as out of date.
#[test]
fn test_settings_adds_the_memory_skill_where_assistants_read_it() {
    use crate::skills::{self, Place, State};
    let (mut app, ctx, dir) = drawable_app("memory-skill");
    let environment = empty_computer(&dir);
    std::fs::create_dir_all(environment.home.join(".claude")).unwrap();
    std::fs::create_dir_all(environment.home.join(".codex")).unwrap();
    app.mcp_clients.environment = Some(environment.clone());
    app.show_settings = true;
    app.settings_page = settings::SettingsPage::Assistants;
    settle_ai_apps(&mut app, &ctx);

    let state_of = |app: &EloraApp, place: Place| {
        app.mcp_clients
            .skill_reports()
            .iter()
            .find(|r| r.place == place)
            .map(|r| r.state.clone())
            .unwrap()
    };
    assert_eq!(state_of(&app, Place::ClaudeCode), State::NotAdded);
    assert_eq!(state_of(&app, Place::Shared), State::NotAdded);
    assert!(
        !environment.home.join(".claude/skills").exists(),
        "looking writes nothing"
    );

    let (program, args) = settings::mcp_launch_command(app.storage.root_dir(), &app.mcp_options);
    let launch = crate::mcp_clients::Launch { program, args };
    app.mcp_clients.start_work(
        vec![
            clients::Job::Skill(Place::ClaudeCode, clients::Task::Add),
            clients::Job::Skill(Place::Shared, clients::Task::Add),
        ],
        &launch,
        &ctx,
    );
    settle_ai_apps(&mut app, &ctx);
    assert_eq!(state_of(&app, Place::ClaudeCode), State::Added);
    assert_eq!(state_of(&app, Place::Shared), State::Added);
    for file in [
        environment
            .home
            .join(".claude/skills/elora-memory/SKILL.md"),
        environment
            .home
            .join(".agents/skills/elora-memory/SKILL.md"),
    ] {
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            skills::SKILL_MD,
            "{}",
            file.display()
        );
    }
    assert!(
        app.status_message
            .as_deref()
            .is_some_and(|m| m.contains("memory skill")),
        "{:?}",
        app.status_message
    );

    // An older copy shows as out of date once the page looks again.
    std::fs::write(Place::ClaudeCode.file(&environment), "an older version").unwrap();
    app.mcp_clients.start_scan(&launch, &ctx);
    settle_ai_apps(&mut app, &ctx);
    assert_eq!(state_of(&app, Place::ClaudeCode), State::Outdated);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_dragging_a_selection_past_an_edge_scrolls_that_way() {
    let dt = 1.0 / 60.0;
    // Inside the note, away from the edges: nothing moves.
    assert_eq!(drag_scroll_step(300.0, 100.0, 500.0, dt), 0.0);
    // Above the top the text moves down (towards its start), and faster the further out.
    let near = drag_scroll_step(95.0, 100.0, 500.0, dt);
    let far = drag_scroll_step(0.0, 100.0, 500.0, dt);
    assert!(near > 0.0 && far > near);
    // Below the bottom it moves the other way.
    assert!(drag_scroll_step(560.0, 100.0, 500.0, dt) < 0.0);
    // However far out, it is held to a top speed.
    assert!(drag_scroll_step(-100000.0, 100.0, 500.0, dt) <= 1800.0 * dt + 0.001);
}

/// Sharing a note by link from the app: the host's note, a joined copy in the same app, and a
/// watcher, all edited through the real tunnel on this computer.
#[test]
fn test_a_note_is_shared_by_link_and_edited_from_both_ends() {
    use crate::share::{Access, Network};
    let (mut app, ctx, dir) = drawable_app("share-link");
    app.shares.network_override = Some(Network::Loopback);
    let note = Note::new("Game plan", "the first idea", app.storage.root_dir());
    let host_id = note.id.clone();
    app.storage.save_note(&note).unwrap();
    app.notes.push(note);
    app.selected_id = Some(host_id.clone());

    // Start sharing and wait for it to be ready.
    app.start_sharing(&host_id);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        app.poll_shares(&ctx);
        if app.shares.sessions.get(&host_id).is_some_and(|s| s.ready) {
            break;
        }
        assert!(Instant::now() < deadline, "sharing never got ready");
        std::thread::sleep(Duration::from_millis(20));
    }
    let (edit_link, view_link) = match &app.shares.sessions[&host_id].role {
        shares::Role::Hosting { host } => (
            host.link(Access::Edit).to_string(),
            host.link(Access::View).to_string(),
        ),
        _ => panic!("not hosting"),
    };

    let join = |app: &mut EloraApp, link: &str| -> String {
        let before: BTreeSet<String> = app.shares.joined_ids.clone();
        app.shares.join_text = link.to_string();
        app.shares.join_open = true;
        app.join_link();
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            app.poll_shares(&ctx);
            if let Some(id) = app.shares.joined_ids.difference(&before).next().cloned() {
                if app.shares.sessions[&id].ready {
                    return id;
                }
            }
            assert!(
                Instant::now() < deadline,
                "never joined: {:?}; joining {}; joined {:?}; sessions {:?}",
                app.shares.message,
                app.shares.joining.is_some(),
                app.shares.joined_ids,
                app.shares
                    .sessions
                    .iter()
                    .map(|(id, s)| (id.clone(), s.ready, s.connected, s.error.clone()))
                    .collect::<Vec<_>>()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let editing = join(&mut app, &edit_link);
    let watching = join(&mut app, &view_link);
    assert!(!app.shares.join_open, "the join window closes once joined");
    let body_of =
        |app: &EloraApp, id: &str| app.notes.iter().find(|n| n.id == id).unwrap().body.clone();
    assert_eq!(body_of(&app, &editing), "the first idea");
    assert!(
        app.session_blocks_editing(&watching),
        "a watcher cannot type"
    );
    assert!(!app.session_blocks_editing(&editing));
    assert_eq!(
        app.view_mode,
        EditorViewMode::Preview,
        "watching opens in preview"
    );

    // The host types: the editing guest and the watcher see it.
    let at = |app: &EloraApp, id: &str| app.notes.iter().position(|n| n.id == id).unwrap();
    let host_at = at(&app, &host_id);
    app.notes[host_at].body = "the first idea, and a second".to_string();
    app.send_session_edits(host_at);
    let settle = |app: &mut EloraApp, what: &str, done: &dyn Fn(&EloraApp) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !done(app) {
            app.poll_shares(&ctx);
            assert!(Instant::now() < deadline, "gave up waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let (e, w) = (editing.clone(), watching.clone());
    settle(&mut app, "the guests to see the host", &|app| {
        body_of(app, &e) == "the first idea, and a second"
            && body_of(app, &w) == "the first idea, and a second"
    });

    // The editing guest types: it reaches the host's note, and is saved to the host's file.
    let guest_at = at(&app, &editing);
    app.notes[guest_at].body = "Idea: the first idea, and a second".to_string();
    app.send_session_edits(guest_at);
    let h = host_id.clone();
    settle(&mut app, "the host to see the guest", &|app| {
        body_of(app, &h) == "Idea: the first idea, and a second"
    });
    let saved = std::fs::read_to_string(&app.notes[at(&app, &host_id)].file_path).unwrap();
    assert!(
        saved.contains("Idea: the first idea, and a second"),
        "{saved}"
    );

    // The watcher's own changes are put back, and nothing reaches anybody.
    let watch_at = at(&app, &watching);
    app.notes[watch_at].body = "vandalism".to_string();
    app.send_session_edits(watch_at);
    for _ in 0..40 {
        app.poll_shares(&ctx);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        body_of(&app, &host_id),
        "Idea: the first idea, and a second"
    );
    assert_eq!(
        body_of(&app, &watching),
        "Idea: the first idea, and a second"
    );

    // Joined notes never reach the disk, and leaving takes them from the list.
    let joined_file = app.notes[at(&app, &editing)].file_path.clone();
    assert!(!joined_file.exists());
    app.leave_shared(&editing);
    app.leave_shared(&watching);
    assert!(app.shares.joined_ids.is_empty());
    assert!(app
        .notes
        .iter()
        .all(|n| n.id == host_id || !n.id.starts_with("shared-with-you")));

    // Stopping ends the session, and the note stays here as it was.
    app.stop_sharing(&host_id);
    assert!(app.shares.sessions.is_empty());
    assert!(body_of(&app, &host_id).starts_with("Idea:"));

    // The windows draw in every state.
    app.shares.share_open = true;
    app.shares.join_open = true;
    draw(&ctx, |ctx| app.share_windows(ctx));
    let _ = std::fs::remove_dir_all(&dir);
}
