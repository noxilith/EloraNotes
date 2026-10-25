//! The shared library: connecting to a server, mirroring its notes and choosing who sees them.
use super::*;

impl EloraApp {
    /// The identifier this client goes by: what was typed in Settings, or the account name when
    /// a settings file from before identifiers has nothing in it.
    pub(super) fn server_display_name(&self) -> String {
        let typed = self.display_name.trim();
        if typed.is_empty() {
            crate::net::os_username()
        } else {
            typed.to_string()
        }
    }

    /// Connects to the server in Settings and remembers the details.
    pub(super) fn connect_server(&mut self) {
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

    pub(super) fn disconnect_server(&mut self) {
        self.collab.disconnect();
        self.collab_open = None;
        self.awaiting_room_body = None;
        self.drop_remote_notes();
        self.status_message = Some("Left the shared library".to_string());
    }

    /// Removes the server's notes from the list after disconnecting.
    pub(super) fn drop_remote_notes(&mut self) {
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
    pub(super) fn move_note_to_server(&mut self, note_idx: usize) {
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
    pub(super) fn copy_shared_note_to_computer(&mut self, note_idx: usize) {
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
    pub(super) fn apply_library(&mut self, editing_title: bool) {
        let metas = self.collab.notes().to_vec();
        let selected = self.selected_id.clone();
        let live: BTreeSet<String> = metas.iter().map(|m| m.id.clone()).collect();
        self.notes
            .retain(|n| !self.remote_ids.contains(&n.id) || live.contains(&n.id));
        self.remote_ids.retain(|id| live.contains(id));

        for meta in metas {
            // An id goes into file names (an export writes `<id>.html`), so one that is not a
            // plain word is dropped. So is one a note on this computer already uses: taking it
            // over would stop that note being saved to disk and show the server's in its place.
            let local_clash = self
                .notes
                .iter()
                .any(|n| n.id == meta.id && !self.remote_ids.contains(&n.id));
            if !is_safe_note_id(&meta.id) || local_clash {
                continue;
            }
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
                        extra_frontmatter: String::new(),
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
    pub(super) fn poll_collab(&mut self, ctx: &egui::Context) {
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
                            extra_frontmatter: String::new(),
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
                // The fingerprint itself is picked up below with `take_fingerprint`, which
                // opens the prompt; the connection resumes once the user trusts it.
                Incoming::Unverified { .. } => {}
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
    pub(super) fn sync_open_note(&mut self) {
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
    pub(super) fn room_body_pending(&self, id: &str) -> bool {
        self.awaiting_room_body
            .as_ref()
            .is_some_and(|(pending, since)| pending == id && since.elapsed() < ROOM_BODY_WAIT)
    }

    /// Pins the certificate the user just confirmed and connects for real. Only now does the
    /// client send its password and identifier: the first attempt stopped after reading the
    /// certificate.
    pub(super) fn trust_server(&mut self, fingerprint: String) {
        self.server.fingerprint = Some(fingerprint);
        self.server_form.fingerprint = self.server.fingerprint.clone();
        self.persist_settings();
        self.trust_prompt = None;
        self.status_message = Some("Server trusted".to_string());
        self.connect_server();
    }

    /// Asks the user to confirm a server's certificate the first time they connect to it.
    pub(super) fn trust_prompt_window(&mut self, ctx: &egui::Context) {
        let Some((fingerprint, address)) = self.trust_prompt.clone() else {
            return;
        };
        let p = self.theme.palette_with(&self.colors, self.glow_strength);
        // Eight groups of four, on two lines so the code stays large enough to compare.
        let groups: Vec<String> = crate::net::short_fingerprint(&fingerprint)
            .split('-')
            .map(str::to_string)
            .collect();
        let short = if groups.len() > 4 {
            format!("{}\n{}", groups[..4].join("-"), groups[4..].join("-"))
        } else {
            groups.join("-")
        };
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
                    RichText::new("Nothing has been sent to the server yet. EloraNotes remembers this code, so nobody can pretend to be this server later.")
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
            self.trust_server(fingerprint);
        } else if reject || !open {
            self.trust_prompt = None;
            self.disconnect_server();
        }
    }

    /// The panel behind the header's lock button: open the note to the whole server, close it to
    /// everybody but its owner, or name the people who may read it. Only the owner may change it;
    /// everybody else sees which note they were let into and by whom.
    pub(super) fn render_share_picker(&mut self, ctx: &egui::Context, p: &Palette) {
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
}

/// The glyph and the one-line answer for who can read a note, used by the header's lock button.
pub(super) fn share_look(
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
pub(super) fn default_display_name() -> String {
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
pub(super) fn fresh_identifier() -> String {
    let bytes: [u8; 16] = rand::random();
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Whether a note id from the server can stand in a file name as it is: letters, digits, `-`
/// and `_`, and not too long.
pub(super) fn is_safe_note_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().count() <= 128
        && id
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_'))
}
