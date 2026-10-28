//! Sharing a note by link, from the desktop: the Share window, joining a link, and keeping the
//! note in step with the people in it.
//!
//! A *session* is one note being edited live with other people through a link. When this app
//! hosts the note, the note stays an ordinary file here and the session only carries what other
//! people type into it. When this app joined somebody else's link, the note exists only while the
//! session does (it is never written to disk) and is listed under "Shared with you".

use super::*;
use crate::collab::RemotePeer;
use crate::live::LiveNote;
use crate::share::{Access, Joined, Network, ShareHost};

pub(super) enum Role {
    Hosting { host: ShareHost },
    Joined { _joined: Joined, view_only: bool },
}

/// One note shared by link.
pub(super) struct Session {
    live: LiveNote,
    pub(super) role: Role,
    /// True once the note's text has arrived: until then the editor leaves the note alone.
    pub(super) ready: bool,
    /// The text this side last handed over or received, so only a change is sent.
    last_sent: String,
    last_cursor: Option<usize>,
    /// The note's text when sharing began, to notice typing done while it was starting.
    initial: String,
    pub(super) peers: Vec<RemotePeer>,
    pub(super) connected: bool,
    pub(super) error: Option<String>,
}

impl Session {
    pub(super) fn view_only(&self) -> bool {
        matches!(
            self.role,
            Role::Joined {
                view_only: true,
                ..
            }
        )
    }

    fn hosting(&self) -> bool {
        matches!(self.role, Role::Hosting { .. })
    }
}

#[derive(Default)]
pub(super) struct Shares {
    pub(super) sessions: HashMap<String, Session>,
    /// Notes joined from a link: they live only in memory, for as long as the session does.
    pub(super) joined_ids: BTreeSet<String>,
    starting: Option<(String, Receiver<Result<ShareHost, String>>)>,
    pub(super) joining: Option<Receiver<Result<Joined, String>>>,
    pub(super) share_open: bool,
    pub(super) join_open: bool,
    /// Which link the Share window shows: the one that only lets people watch.
    view_link: bool,
    pub(super) join_text: String,
    pub(super) message: Option<String>,
    /// Tests share over this computer only, with no network.
    pub(super) network_override: Option<Network>,
}

fn random_hex(bytes: usize) -> String {
    (0..bytes)
        .map(|_| format!("{:02x}", rand::random::<u8>()))
        .collect()
}

impl EloraApp {
    /// True when typing into this note must not change it right now: a note somebody lets you
    /// only watch, or one whose text has not arrived yet.
    pub(super) fn session_blocks_editing(&self, id: &str) -> bool {
        self.shares
            .sessions
            .get(id)
            .is_some_and(|session| session.view_only() || !session.ready)
    }

    /// How this person is named to the others, and a one-off identifier for this session.
    fn name_for_sessions(&self) -> (String, String) {
        let name = self.display_name.trim();
        let name = if name.is_empty() {
            format!("Guest-{}", random_hex(2))
        } else {
            name.to_string()
        };
        (name, random_hex(16))
    }

    /// Starts sharing the note, on a thread of its own: reaching the relay can take a moment.
    pub(super) fn start_sharing(&mut self, note_id: &str) {
        if self.shares.starting.is_some() || self.shares.sessions.contains_key(note_id) {
            return;
        }
        let Some(note) = self.notes.iter().find(|n| n.id == note_id) else {
            return;
        };
        let (title, body) = (note.display_title(), note.body.clone());
        let network = self.shares.network_override.unwrap_or(Network::Internet);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(ShareHost::start(&title, &body, network));
        });
        self.shares.starting = Some((note_id.to_string(), rx));
        self.shares.message = None;
    }

    pub(super) fn stop_sharing(&mut self, note_id: &str) {
        // Dropping the session closes the door and removes the temporary server.
        self.shares.sessions.remove(note_id);
        self.shares.message = None;
    }

    /// Leaves a note joined from a link: it goes from the list, and nothing of it stays on disk.
    pub(super) fn leave_shared(&mut self, note_id: &str) {
        self.shares.sessions.remove(note_id);
        self.shares.joined_ids.remove(note_id);
        self.notes.retain(|n| n.id != note_id);
        if self.selected_id.as_deref() == Some(note_id) {
            self.selected_id = None;
            self.select_first_visible();
        }
    }

    pub(super) fn join_link(&mut self) {
        if self.shares.joining.is_some() {
            return;
        }
        let link = self.shares.join_text.trim().to_string();
        if link.is_empty() {
            return;
        }
        let network = self.shares.network_override.unwrap_or(Network::Internet);
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::share::join(&link, network));
        });
        self.shares.joining = Some(rx);
        self.shares.message = None;
    }

    fn begin_hosting(&mut self, note_id: String, host: ShareHost) {
        let Some(note) = self.notes.iter().find(|n| n.id == note_id) else {
            return;
        };
        let body = note.body.clone();
        let (name, identifier) = self.name_for_sessions();
        let live = LiveNote::open(
            host.local_profile(),
            name,
            identifier,
            host.note_id().to_string(),
        );
        self.shares.sessions.insert(
            note_id,
            Session {
                live,
                role: Role::Hosting { host },
                ready: false,
                last_sent: body.clone(),
                last_cursor: None,
                initial: body,
                peers: Vec::new(),
                connected: false,
                error: None,
            },
        );
    }

    fn begin_joined(&mut self, joined: Joined) {
        let mut note = Note::new(joined.title.clone(), "", self.storage.root_dir());
        note.id = format!("shared-with-you-{}", random_hex(4));
        let id = note.id.clone();
        let (name, identifier) = self.name_for_sessions();
        let live = LiveNote::open(
            joined.profile.clone(),
            name,
            identifier,
            joined.note_id.clone(),
        );
        let view_only = joined.view_only;
        self.notes.push(note);
        self.shares.joined_ids.insert(id.clone());
        self.shares.sessions.insert(
            id.clone(),
            Session {
                live,
                role: Role::Joined {
                    _joined: joined,
                    view_only,
                },
                ready: false,
                last_sent: String::new(),
                last_cursor: None,
                initial: String::new(),
                peers: Vec::new(),
                connected: false,
                error: None,
            },
        );
        self.selected_id = Some(id);
        if view_only {
            self.view_mode = EditorViewMode::Preview;
        }
        self.shares.join_open = false;
        self.shares.join_text.clear();
    }

    /// Once a frame: finish what was being started, and bring each shared note up to date with
    /// what the other people have typed.
    pub(super) fn poll_shares(&mut self, ctx: &egui::Context) {
        if let Some((id, rx)) = self.shares.starting.take() {
            match rx.try_recv() {
                Ok(Ok(host)) => self.begin_hosting(id, host),
                Ok(Err(why)) => self.shares.message = Some(why),
                Err(TryRecvError::Empty) => self.shares.starting = Some((id, rx)),
                Err(TryRecvError::Disconnected) => {
                    self.shares.message = Some("sharing stopped before it started".to_string());
                }
            }
        }
        if let Some(rx) = self.shares.joining.take() {
            match rx.try_recv() {
                Ok(Ok(joined)) => self.begin_joined(joined),
                Ok(Err(why)) => self.shares.message = Some(why),
                Err(TryRecvError::Empty) => self.shares.joining = Some(rx),
                Err(TryRecvError::Disconnected) => {
                    self.shares.message = Some("joining stopped before it finished".to_string());
                }
            }
        }
        if self.shares.sessions.is_empty()
            && self.shares.starting.is_none()
            && self.shares.joining.is_none()
        {
            return;
        }
        // Somebody's typing arrives at any moment, and an idle window is never asked to draw.
        ctx.request_repaint_after(Duration::from_millis(60));

        let ids: Vec<String> = self.shares.sessions.keys().cloned().collect();
        for id in ids {
            let Some(index) = self.notes.iter().position(|n| n.id == id) else {
                self.shares.sessions.remove(&id);
                continue;
            };
            let Some(session) = self.shares.sessions.get_mut(&id) else {
                continue;
            };
            let look = session.live.poll();
            session.connected = look.connected;
            session.error = look.error;
            session.peers = look
                .peers
                .iter()
                .map(|peer| RemotePeer {
                    name: peer.name.clone(),
                    color: peer.color,
                    cursor: peer.cursor,
                    typing: peer.typing,
                    updated_at: Instant::now(),
                })
                .collect();

            let Some(text) = look.text else {
                // A note somebody lets you only watch is kept exactly as it was received.
                if session.view_only()
                    && session.ready
                    && self.notes[index].body != session.last_sent
                {
                    self.notes[index].body = session.last_sent.clone();
                }
                continue;
            };
            if !session.ready {
                session.ready = true;
                // Words typed here while sharing was starting are not thrown away: they are
                // handed over instead.
                if session.hosting() && self.notes[index].body != session.initial {
                    let body = self.notes[index].body.clone();
                    session.live.set_text(&body);
                    session.last_sent = body;
                    continue;
                }
            }
            session.last_sent = text.clone();
            if self.notes[index].body == text {
                continue;
            }
            let before = std::mem::replace(&mut self.notes[index].body, text.clone());
            self.notes[index].updated_at = Utc::now();
            if self.selected_id.as_deref() == Some(id.as_str()) {
                // The caret belongs to the words around it, so it travels with them.
                self.reveal_caret |= carry_caret_over(ctx, &id, &before, &text);
            }
            // A note hosted from here is a file here: what the others typed is kept.
            if self.shares.sessions.get(&id).is_some_and(|s| s.hosting()) {
                if let Err(why) = self.persist_note(index) {
                    self.status_message = Some(format!("Auto-save error: {why}"));
                }
            }
        }
    }

    /// Hands the other people what was typed in the open note, and where the caret is.
    pub(super) fn send_session_edits(&mut self, note_index: usize) {
        let id = self.notes[note_index].id.clone();
        let caret = self.last_editor_caret;
        let Some(session) = self.shares.sessions.get_mut(&id) else {
            return;
        };
        if !session.ready || session.view_only() {
            return;
        }
        let body = &self.notes[note_index].body;
        if *body != session.last_sent {
            session.live.set_text(body);
            session.last_sent = body.clone();
        }
        if let Some(caret) = caret {
            let char_index = caret.min(body.chars().count());
            let byte = body
                .char_indices()
                .nth(char_index)
                .map(|(i, _)| i)
                .unwrap_or(body.len());
            if session.last_cursor != Some(byte) {
                session.live.set_cursor(byte);
                session.last_cursor = Some(byte);
            }
        }
    }

    /// The Share window and the Join window, when they are open.
    pub(super) fn share_windows(&mut self, ctx: &egui::Context) {
        self.poll_shares(ctx);
        let p = self.theme.palette_with(&self.colors, self.glow_strength);

        if self.shares.share_open {
            let mut open = true;
            egui::Window::new("Share this note")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .default_width(400.0)
                .show(ctx, |ui| self.share_window_contents(ui, &p));
            if !open {
                self.shares.share_open = false;
            }
        }

        if self.shares.join_open {
            let mut open = true;
            egui::Window::new("Join a shared note")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .default_width(400.0)
                .show(ctx, |ui| {
                    ui.label(
                        RichText::new(
                            "Paste the link somebody sent you. You do not need an account or a \
                             server.",
                        )
                        .size(12.0)
                        .color(p.fg_muted),
                    );
                    ui.add_space(6.0);
                    ui.add(
                        TextEdit::singleline(&mut self.shares.join_text)
                            .hint_text("elora://share/…")
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(6.0);
                    let busy = self.shares.joining.is_some();
                    ui.horizontal(|ui| {
                        let ready = !self.shares.join_text.trim().is_empty() && !busy;
                        if ui.add_enabled(ready, egui::Button::new("Join")).clicked() {
                            self.join_link();
                        }
                        if busy {
                            ui.spinner();
                            ui.label(RichText::new("Connecting…").size(12.0).color(p.fg_muted));
                        }
                    });
                    if let Some(message) = &self.shares.message {
                        ui.add_space(4.0);
                        ui.label(RichText::new(message).size(12.0).color(p.danger));
                    }
                });
            if !open {
                self.shares.join_open = false;
            }
        }
    }

    fn share_window_contents(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let Some(id) = self.selected_id.clone() else {
            ui.label("Open a note to share it.");
            return;
        };
        if self.is_remote(&id) {
            ui.label(
                "This note is in a shared library, so everybody on that server can already edit \
                 it together.",
            );
            return;
        }
        if self.shares.joined_ids.contains(&id) {
            ui.label("You joined this note from a link. Only the person who shared it can share it further.");
            if ui.button("Leave this note").clicked() {
                self.shares.share_open = false;
                self.leave_shared(&id);
            }
            return;
        }

        if let Some(session) = self.shares.sessions.get(&id) {
            let access = if self.shares.view_link {
                Access::View
            } else {
                Access::Edit
            };
            let Role::Hosting { host } = &session.role else {
                return;
            };
            let link = host.link(access).to_string();
            let connected = session.connected;
            let error = session.error.clone();
            let people: Vec<(String, [u8; 3])> = session
                .peers
                .iter()
                .map(|peer| (peer.name.clone(), peer.color))
                .collect();
            ui.label(RichText::new("This note is being shared.").strong());
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.shares.view_link, false, "Can edit");
                ui.radio_value(&mut self.shares.view_link, true, "Can only watch");
            });
            ui.add_space(4.0);
            ui.add(
                TextEdit::multiline(&mut link.as_str())
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .font(egui::TextStyle::Monospace),
            );
            ui.horizontal(|ui| {
                if ui.button(format!("{}  Copy link", icon::COPY)).clicked() {
                    ui.ctx().copy_text(link.clone());
                    self.status_message = Some("Copied the share link".to_string());
                }
                if ui.button("Stop sharing").clicked() {
                    self.stop_sharing(&id);
                }
            });
            ui.add_space(6.0);
            ui.label(
                RichText::new(if self.shares.view_link {
                    "Anyone with this link can watch the note while EloraNotes is open on this \
                     computer. They cannot change it."
                } else {
                    "Anyone with this link can edit the note while EloraNotes is open on this \
                     computer. Send it only to people you trust."
                })
                .size(11.5)
                .color(p.fg_muted),
            );
            ui.add_space(4.0);
            if !connected {
                ui.label(
                    RichText::new(error.unwrap_or_else(|| "Getting ready…".to_string()))
                        .size(11.5)
                        .color(p.fg_muted),
                );
            } else if people.is_empty() {
                ui.label(
                    RichText::new("Nobody else is in the note yet.")
                        .size(11.5)
                        .color(p.fg_muted),
                );
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!("{} In the note", icon::USERS))
                            .size(11.5)
                            .color(p.fg_muted),
                    );
                    for (name, [r, g, b]) in people {
                        ui.label(
                            RichText::new(format!("{} {name}", icon::BRUSH))
                                .size(11.5)
                                .color(Color32::from_rgb(r, g, b)),
                        );
                    }
                });
            }
            return;
        }

        if self.shares.starting.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Starting…");
            });
            return;
        }

        ui.label(
            "Make a link that anyone can open to edit this note with you, or just watch it, as \
             you type. There is no server and nobody needs an account.",
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.radio_value(&mut self.shares.view_link, false, "People can edit");
            ui.radio_value(&mut self.shares.view_link, true, "People can only watch");
        });
        ui.add_space(6.0);
        if ui
            .button(format!("{}  Start sharing", icon::USER_PLUS))
            .clicked()
        {
            self.start_sharing(&id);
        }
        ui.add_space(4.0);
        ui.label(
            RichText::new(
                "The link works while EloraNotes stays open on this computer. The note itself \
                 stays here as a normal file.",
            )
            .size(11.5)
            .color(p.fg_muted),
        );
        if let Some(message) = &self.shares.message {
            ui.add_space(4.0);
            ui.label(RichText::new(message).size(12.0).color(p.danger));
        }
    }
}
