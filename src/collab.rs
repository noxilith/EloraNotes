//! The client side of the shared library.
//!
//! A worker thread owns the TLS connection and the Yjs document for whichever note is open, so
//! the UI never blocks on the network. The UI keeps the text it draws (as it always has) and
//! hands every local change to the worker; the worker diffs it against what the document holds
//! and ships only the changed range, so two people typing at once merge instead of overwriting.
//!
//! Events come back through a channel and are drained once per frame by [`Collab::poll`].

use crate::net::{
    self, ClientMsg, NoteMeta, PresenceUser, ServerMsg, FRAME_AWARENESS, FRAME_JSON, FRAME_SYNC,
    PROTOCOL_VERSION,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};
use yrs::sync::{Awareness, AwarenessUpdate, Message as YMessage, SyncMessage};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, GetString, ReadTxn, Text, TextRef, Transact};

/// How long a read waits before the worker looks at its command channel again.
const POLL: Duration = Duration::from_millis(8);
/// First retry delay after a dropped connection; doubles up to [`MAX_RETRY`].
const FIRST_RETRY: Duration = Duration::from_secs(2);
const MAX_RETRY: Duration = Duration::from_secs(15);
/// Cursor updates are throttled: a caret that moves every keystroke does not need every frame.
const CURSOR_INTERVAL: Duration = Duration::from_millis(120);

/// Colours handed out to people editing together, picked to stay readable on both themes.
pub const PEER_COLORS: [[u8; 3]; 8] = [
    [137, 180, 250],
    [203, 166, 247],
    [250, 179, 135],
    [166, 227, 161],
    [243, 139, 168],
    [137, 220, 235],
    [249, 192, 98],
    [186, 172, 240],
];

/// What the user typed into Settings. The fingerprint is filled in after the first connect, so
/// the same server can never be swapped out from under them later.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerProfile {
    #[serde(default)]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub password: String,
    /// SHA-256 of the server certificate, hex. `None` until the first connection is accepted.
    #[serde(default)]
    pub fingerprint: Option<String>,
}

fn default_port() -> u16 {
    7331
}

impl Default for ServerProfile {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: default_port(),
            password: String::new(),
            fingerprint: None,
        }
    }
}

impl ServerProfile {
    pub fn is_configured(&self) -> bool {
        !self.host.trim().is_empty() && !self.password.is_empty()
    }

    /// `host:port`, accepting a pasted `host:port` in the host field.
    pub fn address(&self) -> String {
        let host = self.host.trim();
        if host.starts_with('[') {
            // Already bracketed IPv6; only add the port when it is missing.
            if host.contains("]:") {
                return host.to_string();
            }
            return format!("{host}:{}", self.port);
        }
        match host.matches(':').count() {
            0 => format!("{host}:{}", self.port),
            // A bare IPv6 address has more than one colon and no port of its own.
            1 => {
                if let Some((_, port)) = host.rsplit_once(':') {
                    if port.parse::<u16>().is_ok() {
                        return host.to_string();
                    }
                }
                format!("{host}:{}", self.port)
            }
            _ => format!("[{host}]:{}", self.port),
        }
    }
}

/// What the client knows about its connection, for the sidebar status line.
#[derive(Clone, Debug, PartialEq)]
pub enum ConnectionState {
    Offline,
    Connecting,
    Online { server: String, you: String },
}

/// Somebody else with the open note in front of them.
#[derive(Clone, Debug, PartialEq)]
pub struct RemotePeer {
    pub name: String,
    pub color: [u8; 3],
    /// Caret position in bytes into the note body, when they have one.
    pub cursor: Option<usize>,
    /// True while they are typing, so the caret can glow instead of sitting still.
    pub typing: bool,
    pub updated_at: Instant,
}

/// Messages from the worker thread.
#[derive(Debug)]
pub enum Incoming {
    /// Handshake accepted. `fingerprint` is what the server presented.
    Online {
        server: String,
        you: String,
        notes: Vec<NoteMeta>,
        fingerprint: String,
    },
    Notes(Vec<NoteMeta>),
    /// The note this client just asked for, so it can be opened right away.
    Created(NoteMeta),
    Presence(Vec<(String, Vec<PresenceUser>)>),
    /// The body of the open note, after a remote change.
    Body {
        id: String,
        text: String,
    },
    Peers(Vec<RemotePeer>),
    Offline(String),
}

/// What the UI wants to happen when a message arrives. The server binary builds without a window
/// at all, so this is a plain callback rather than a handle on egui: the app passes a function
/// that asks for a frame. Without it a remote keystroke would sit in the channel until the local
/// mouse moved, because an idle window is never asked to draw.
pub type Waker = std::sync::Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
struct Notify {
    events: Sender<Incoming>,
    wake: Option<Waker>,
}

impl Notify {
    fn send(&self, event: Incoming) {
        let _ = self.events.send(event);
        if let Some(wake) = &self.wake {
            wake();
        }
    }
}

enum Command {
    Open(String),
    Close,
    Body {
        text: String,
    },
    Cursor(usize),
    Create {
        title: String,
        folder: Option<String>,
        tags: Vec<String>,
        body: String,
        pinned: bool,
        icon: Option<String>,
    },
    Update {
        id: String,
        meta: NoteMeta,
    },
    Delete {
        id: String,
        permanent: bool,
    },
    Stop,
}

/// The UI-facing half of the connection.
pub struct Collab {
    commands: Option<Sender<Command>>,
    events: Receiver<Incoming>,
    state: ConnectionState,
    notes: Vec<NoteMeta>,
    presence: BTreeMap<String, Vec<PresenceUser>>,
    peers: Vec<RemotePeer>,
    /// Certificate the server presented, for the UI to pin into the profile.
    pending_fingerprint: Option<String>,
    /// The name this client is known by on the server, so its own presence is not shown.
    you: Option<String>,
    /// Note the worker currently has open.
    open: Option<String>,
    /// Body last known to the worker, so unchanged frames are not sent twice.
    sent_body: Option<(String, String)>,
    /// True once a failure has been reported, so the retry loop below it stays quiet.
    offline_reported: bool,
    /// Called by the worker when a message arrives, to redraw the window straight away.
    wake: Option<Waker>,
}

impl Default for Collab {
    fn default() -> Self {
        Self::new()
    }
}

impl Collab {
    pub fn new() -> Self {
        let (_sender, receiver) = mpsc::channel();
        Self {
            commands: None,
            events: receiver,
            state: ConnectionState::Offline,
            notes: Vec::new(),
            presence: BTreeMap::new(),
            peers: Vec::new(),
            pending_fingerprint: None,
            you: None,
            open: None,
            sent_body: None,
            offline_reported: false,
            wake: None,
        }
    }

    /// Registers what wakes the window when a message arrives. The app passes a function that
    /// asks for a new frame; the server binary, with no window, passes nothing at all.
    pub fn set_waker(&mut self, wake: Waker) {
        self.wake = Some(wake);
    }

    pub fn state(&self) -> &ConnectionState {
        &self.state
    }

    pub fn is_connected(&self) -> bool {
        matches!(self.state, ConnectionState::Online { .. })
    }

    pub fn is_active(&self) -> bool {
        self.commands.is_some()
    }

    pub fn server_name(&self) -> Option<&str> {
        match &self.state {
            ConnectionState::Online { server, .. } => Some(server),
            _ => None,
        }
    }

    /// The notes the server holds, as last reported.
    pub fn notes(&self) -> &[NoteMeta] {
        &self.notes
    }

    /// Names of the people with a note open right now.
    pub fn watchers(&self, note_id: &str) -> &[PresenceUser] {
        self.presence
            .get(note_id)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub fn is_watched(&self, note_id: &str) -> bool {
        !self.watchers(note_id).is_empty()
    }

    pub fn peers(&self) -> &[RemotePeer] {
        &self.peers
    }

    pub fn pending_fingerprint(&self) -> Option<&str> {
        self.pending_fingerprint.as_deref()
    }

    pub fn take_fingerprint(&mut self) -> Option<String> {
        self.pending_fingerprint.take()
    }

    /// Starts a worker thread for `profile`. Any previous connection is dropped first.
    pub fn connect(&mut self, profile: ServerProfile, username: String) {
        self.disconnect();
        if !profile.is_configured() {
            self.state = ConnectionState::Offline;
            return;
        }
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let notify = Notify {
            events: event_tx,
            wake: self.wake.clone(),
        };
        std::thread::Builder::new()
            .name("elora-collab".to_string())
            .spawn(move || worker(profile, username, command_rx, notify))
            .ok();
        self.commands = Some(command_tx);
        self.events = event_rx;
        self.state = ConnectionState::Connecting;
        self.offline_reported = false;
        self.notes.clear();
        self.presence.clear();
        self.peers.clear();
        self.open = None;
        self.sent_body = None;
    }

    pub fn disconnect(&mut self) {
        if let Some(commands) = self.commands.take() {
            let _ = commands.send(Command::Stop);
        }
        self.state = ConnectionState::Offline;
        self.notes.clear();
        self.presence.clear();
        self.peers.clear();
        self.open = None;
        self.sent_body = None;
    }

    /// Drains everything the worker sent since the last frame.
    pub fn poll(&mut self) -> Vec<Incoming> {
        let mut out = Vec::new();
        while let Ok(event) = self.events.try_recv() {
            // A retry that fails again should not repeat the same message every few seconds;
            // one report per outage is enough for the status line.
            if matches!(event, Incoming::Offline(_)) {
                if self.offline_reported {
                    continue;
                }
                self.offline_reported = true;
            }
            self.absorb(&event);
            out.push(event);
        }
        out
    }

    fn absorb(&mut self, event: &Incoming) {
        match event {
            Incoming::Online {
                server,
                you,
                notes,
                fingerprint,
            } => {
                self.state = ConnectionState::Online {
                    server: server.clone(),
                    you: you.clone(),
                };
                self.you = Some(you.clone());
                self.offline_reported = false;
                self.notes = notes.clone();
                self.pending_fingerprint = Some(fingerprint.clone());
            }
            Incoming::Notes(notes) => {
                self.notes = notes.clone();
                // Presence for a note that no longer exists is stale.
                self.presence
                    .retain(|id, _| notes.iter().any(|n| &n.id == id));
            }
            Incoming::Presence(notes) => {
                // Seeing yourself in the list is noise; the dots are for other people.
                self.presence = notes
                    .iter()
                    .map(|(id, users)| {
                        let others: Vec<PresenceUser> = users
                            .iter()
                            .filter(|u| Some(&u.name) != self.you.as_ref())
                            .cloned()
                            .collect();
                        (id.clone(), others)
                    })
                    .filter(|(_, users)| !users.is_empty())
                    .collect();
            }
            Incoming::Peers(peers) => self.peers = peers.clone(),
            Incoming::Offline(_) => {
                self.state = ConnectionState::Offline;
                self.peers.clear();
                self.presence.clear();
            }
            Incoming::Body { .. } | Incoming::Created(_) => {}
        }
    }

    fn send(&self, command: Command) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(command);
        }
    }

    /// Starts mirroring one note. `body` is what the UI is showing, so the worker can tell the
    /// server about text typed before the note was opened as a shared one.
    pub fn open_note(&mut self, id: &str, body: &str) {
        if !self.is_active() {
            return;
        }
        if self.open.as_deref() == Some(id) {
            return;
        }
        self.open = Some(id.to_string());
        self.sent_body = Some((id.to_string(), body.to_string()));
        self.peers.clear();
        self.send(Command::Open(id.to_string()));
    }

    pub fn close_note(&mut self) {
        if self.open.take().is_some() {
            self.send(Command::Close);
            self.peers.clear();
        }
        self.sent_body = None;
    }

    /// Hands the editor's current text to the worker; it ships only the changed range.
    pub fn send_body(&mut self, id: &str, body: &str) {
        if !self.is_connected() {
            return;
        }
        if let Some((sent_id, sent)) = &self.sent_body {
            if sent_id == id && sent == body {
                return;
            }
        }
        self.sent_body = Some((id.to_string(), body.to_string()));
        self.send(Command::Body {
            text: body.to_string(),
        });
    }

    pub fn send_cursor(&mut self, byte_index: usize) {
        if self.is_connected() {
            self.send(Command::Cursor(byte_index));
        }
    }

    /// Marks the local copy as already known, after applying a remote body.
    pub fn note_body_applied(&mut self, id: &str, body: &str) {
        self.sent_body = Some((id.to_string(), body.to_string()));
    }

    /// Asks the server for a new note. The answer arrives as [`Incoming::Created`].
    pub fn create_note(
        &mut self,
        title: &str,
        folder: Option<&str>,
        tags: &[String],
        body: &str,
        pinned: bool,
        icon: Option<&str>,
    ) {
        self.send(Command::Create {
            title: title.to_string(),
            folder: folder.map(str::to_string),
            tags: tags.to_vec(),
            body: body.to_string(),
            pinned,
            icon: icon.map(str::to_string),
        });
    }

    pub fn update_note(&mut self, meta: &NoteMeta) {
        self.send(Command::Update {
            id: meta.id.clone(),
            meta: meta.clone(),
        });
    }

    pub fn delete_note(&mut self, id: &str, permanent: bool) {
        self.send(Command::Delete {
            id: id.to_string(),
            permanent,
        });
    }
}

/// One connection attempt; dropping it closes the socket.
struct Session {
    stream: rustls::StreamOwned<rustls::ClientConnection, std::net::TcpStream>,
    awareness: Awareness,
    text: TextRef,
    /// Last text the UI was told about (or that the UI sent), used to diff local edits.
    mirror: String,
    open: Option<String>,
    /// True between asking for a note and the server sending that note's state. Updates that
    /// arrive in that window belong to the note just left, whose document has been thrown away,
    /// and must not be merged into this note.
    awaiting_room_state: bool,
    buffer: Vec<u8>,
    last_cursor: Instant,
    last_cursor_sent: Option<usize>,
    name: String,
    color: [u8; 3],
}

fn worker(profile: ServerProfile, name: String, commands: Receiver<Command>, events: Notify) {
    let mut delay = FIRST_RETRY;
    loop {
        if matches!(
            commands.try_recv(),
            Ok(Command::Stop) | Err(TryRecvError::Disconnected)
        ) {
            return;
        }
        match Session::start(&profile, &name, &events, &commands) {
            Ok(mut session) => {
                delay = FIRST_RETRY;
                let reason = session.run(&commands, &events);
                events.send(Incoming::Offline(reason));
            }
            Err(error) => {
                events.send(Incoming::Offline(error));
            }
        }

        // Wait before retrying, but stay responsive to Disconnect.
        let deadline = Instant::now() + delay;
        while Instant::now() < deadline {
            match commands.recv_timeout(Duration::from_millis(50)) {
                Ok(Command::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        delay = (delay * 2).min(MAX_RETRY);
    }
}

impl Session {
    fn start(
        profile: &ServerProfile,
        name: &str,
        events: &Notify,
        commands: &Receiver<Command>,
    ) -> Result<Session, String> {
        let address = profile.address();
        let socket = std::net::TcpStream::connect(&address)
            .map_err(|e| format!("could not reach {address}: {e}"))?;
        socket.set_nodelay(true).ok();
        socket
            .set_read_timeout(Some(POLL))
            .map_err(|e| e.to_string())?;
        socket
            .set_write_timeout(Some(Duration::from_secs(10)))
            .map_err(|e| e.to_string())?;

        let (config, seen) = crate::tls::client_config(profile.fingerprint.clone());
        let server_name = rustls::pki_types::ServerName::try_from(profile.host.trim().to_string())
            .unwrap_or_else(|_| {
                rustls::pki_types::ServerName::try_from("elora-server").expect("valid name")
            });
        let connection = rustls::ClientConnection::new(config, server_name)
            .map_err(|e| format!("could not start TLS: {e}"))?;
        let mut stream = rustls::StreamOwned::new(connection, socket);

        let hello = ClientMsg::Hello {
            v: PROTOCOL_VERSION,
            name: name.to_string(),
            password: profile.password.clone(),
        };
        let payload = serde_json::to_vec(&hello).map_err(|e| e.to_string())?;
        net::write_frame(&mut stream, FRAME_JSON, &payload)
            .and_then(|_| stream.flush())
            .map_err(|e| format!("could not send the handshake: {e}"))?;

        // The server answers with Welcome or Error; anything else means this is not Elora.
        let mut buffer = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(15);
        let (server, you, notes) = 'handshake: loop {
            if Instant::now() > deadline {
                return Err("the server did not answer".to_string());
            }
            drain_pending(commands);
            match read_frames(&mut stream, &mut buffer)? {
                Some(frames) => {
                    for (kind, payload) in frames {
                        if kind != FRAME_JSON {
                            continue;
                        }
                        match serde_json::from_slice::<ServerMsg>(&payload) {
                            Ok(ServerMsg::Welcome {
                                server, you, notes, ..
                            }) => break 'handshake (server, you, notes),
                            Ok(ServerMsg::Error { message }) => return Err(message),
                            _ => {}
                        }
                    }
                }
                None => std::thread::sleep(Duration::from_millis(2)),
            }
        };

        let fingerprint = seen
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
            .ok_or_else(|| "the server presented no certificate".to_string())?;
        events.send(Incoming::Online {
            server,
            you: you.clone(),
            notes,
            fingerprint,
        });

        let doc = Doc::new();
        let text = doc.get_or_insert_text("body");
        let awareness = Awareness::new(doc);
        let mut session = Session {
            stream,
            awareness,
            text,
            mirror: String::new(),
            open: None,
            awaiting_room_state: false,
            buffer,
            last_cursor: Instant::now(),
            last_cursor_sent: None,
            name: name.to_string(),
            color: color_for(name),
        };
        session.publish_awareness(events, true);
        Ok(session)
    }

    fn run(&mut self, commands: &Receiver<Command>, events: &Notify) -> String {
        loop {
            loop {
                match commands.try_recv() {
                    Ok(Command::Stop) | Err(TryRecvError::Disconnected) => {
                        return "disconnected".to_string()
                    }
                    Ok(command) => {
                        if let Err(error) = self.handle(command, events) {
                            return error;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                }
            }

            match read_frames(&mut self.stream, &mut self.buffer) {
                Ok(Some(frames)) => {
                    for (kind, payload) in frames {
                        if let Err(error) = self.handle_frame(kind, payload, events) {
                            return error;
                        }
                    }
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(2)),
                Err(error) => return error,
            }
        }
    }

    fn handle(&mut self, command: Command, events: &Notify) -> Result<(), String> {
        match command {
            Command::Stop => return Err("disconnected".to_string()),
            Command::Open(id) => {
                if self.open.as_deref() == Some(id.as_str()) {
                    return Ok(());
                }
                // A document belongs to one note, not to the connection. Keeping the old one
                // meant the text of the note just left travelled into the next room: switching
                // notes, or creating one while another was open, handed the server the previous
                // note's text and the new note came out as a copy of it.
                let doc = Doc::new();
                self.text = doc.get_or_insert_text("body");
                self.awareness = Awareness::new(doc);
                self.mirror.clear();
                self.awaiting_room_state = true;
                self.send_msg(&ClientMsg::Open { id: id.clone() })?;
                self.open = Some(id);
                self.last_cursor_sent = None;
                // Announce ourselves in the room so the presence list has a name to show even
                // before this person moves their caret.
                self.publish_awareness(events, true);
            }
            Command::Close => {
                self.open = None;
                self.mirror.clear();
                self.clear_awareness();
                self.send_msg(&ClientMsg::Close)?;
                events.send(Incoming::Peers(Vec::new()));
            }
            Command::Body { text } => self.apply_local(&text)?,
            Command::Cursor(byte) => {
                if self.open.is_none() {
                    return Ok(());
                }
                // The first caret goes out immediately; after that it is throttled so typing
                // does not turn into a stream of awareness frames.
                let throttled =
                    self.last_cursor_sent.is_some() && self.last_cursor.elapsed() < CURSOR_INTERVAL;
                if self.last_cursor_sent == Some(byte) || throttled {
                    return Ok(());
                }
                self.last_cursor = Instant::now();
                self.last_cursor_sent = Some(byte);
                self.publish_awareness(events, false);
            }
            Command::Create {
                title,
                folder,
                tags,
                body,
                pinned,
                icon,
            } => self.send_msg(&ClientMsg::Create {
                title,
                folder,
                tags,
                body,
                pinned,
                icon,
            })?,
            Command::Update { id, meta } => self.send_msg(&ClientMsg::Update {
                id,
                title: Some(meta.title),
                folder: Some(meta.folder),
                tags: Some(meta.tags),
                pinned: Some(meta.pinned),
                trashed: Some(meta.trashed),
                icon: Some(meta.icon),
            })?,
            Command::Delete { id, permanent } => {
                self.send_msg(&ClientMsg::Delete { id, permanent })?
            }
        }
        Ok(())
    }

    fn handle_frame(&mut self, kind: u8, payload: Vec<u8>, events: &Notify) -> Result<(), String> {
        match kind {
            FRAME_JSON => {
                let message: ServerMsg = match serde_json::from_slice(&payload) {
                    Ok(message) => message,
                    Err(_) => return Ok(()),
                };
                match message {
                    ServerMsg::Notes { notes } => events.send(Incoming::Notes(notes)),
                    ServerMsg::Created { note } => events.send(Incoming::Created(note)),
                    ServerMsg::Presence { notes } => events.send(Incoming::Presence(notes)),
                    ServerMsg::Error { message } => events.send(Incoming::Offline(message)),
                    _ => {}
                }
            }
            FRAME_SYNC => {
                let Ok(message) = YMessage::decode_v1(&payload) else {
                    return Ok(());
                };
                match message {
                    YMessage::Sync(SyncMessage::SyncStep1(sv)) => {
                        let update = self
                            .awareness
                            .doc()
                            .transact()
                            .encode_state_as_update_v1(&sv);
                        let reply = YMessage::Sync(SyncMessage::SyncStep2(update)).encode_v1();
                        self.send_frame(FRAME_SYNC, &reply)?;
                    }
                    YMessage::Sync(SyncMessage::SyncStep2(update)) => {
                        // The room's own state, sent as soon as the note opens.
                        self.awaiting_room_state = false;
                        self.apply_remote(update, events)?;
                    }
                    YMessage::Sync(SyncMessage::Update(update)) => {
                        // An update still in flight from the note just left describes a document
                        // that no longer exists here, and must not be merged into this one.
                        if self.awaiting_room_state {
                            return Ok(());
                        }
                        self.apply_remote(update, events)?;
                    }
                    _ => {}
                }
            }
            FRAME_AWARENESS => {
                let Ok(update) = AwarenessUpdate::decode_v1(&payload) else {
                    return Ok(());
                };
                if self.awareness.apply_update(update).is_ok() {
                    events.send(Incoming::Peers(self.peers()));
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Merges an update from the server into this note's document.
    fn apply_remote(&mut self, update: Vec<u8>, events: &Notify) -> Result<(), String> {
        let Ok(update) = yrs::Update::decode_v1(&update) else {
            return Ok(());
        };
        if self
            .awareness
            .doc_mut()
            .transact_mut()
            .apply_update(update)
            .is_err()
        {
            return Ok(());
        }
        self.emit_body(events);
        Ok(())
    }

    /// Applies what the user typed to the shared document. Only the changed range is sent, and
    /// the position is mapped into the document so a remote insert elsewhere does not make the
    /// local text land in the wrong place.
    fn apply_local(&mut self, text: &str) -> Result<(), String> {
        if self.open.is_none() {
            return Ok(());
        }
        if text == self.mirror {
            return Ok(());
        }
        let current = self.doc_text();
        let (start, removed, inserted) = diff(&self.mirror, text);
        let base = self.mirror.clone();
        if base.is_empty() && current.is_empty() {
            // First edit after opening a note that was empty everywhere.
            let update = self.write_range(0, 0, text);
            self.mirror = text.to_string();
            return self.send_update(update);
        }

        let at = map_position(&base, &current, start);
        let at = at.min(current.len());
        let at = floor_char_boundary(&current, at);
        let removed_text = &base[start..start + removed];
        let delete = current[at..].starts_with(removed_text) && !removed_text.is_empty();
        let update = self.write_range(
            at,
            if delete { removed } else { 0 },
            &text[start..start + inserted],
        );
        self.mirror = text.to_string();
        self.send_update(update)
    }

    /// Applies one local change and returns the Yjs update describing it. The update has to be
    /// encoded inside the transaction that made the change, otherwise it carries nothing.
    fn write_range(&mut self, at: usize, removed: usize, inserted: &str) -> Vec<u8> {
        let mut txn = self.awareness.doc_mut().transact_mut();
        if removed > 0 {
            self.text.remove_range(&mut txn, at as u32, removed as u32);
        }
        if !inserted.is_empty() {
            self.text.insert(&mut txn, at as u32, inserted);
        }
        txn.encode_update_v1()
    }

    fn send_update(&mut self, update: Vec<u8>) -> Result<(), String> {
        if update.is_empty() {
            return Ok(());
        }
        let frame = YMessage::Sync(SyncMessage::Update(update)).encode_v1();
        self.send_frame(FRAME_SYNC, &frame)
    }

    fn emit_body(&mut self, events: &Notify) {
        let Some(id) = self.open.clone() else {
            return;
        };
        let text = self.doc_text();
        if text == self.mirror {
            return;
        }
        self.mirror = text.clone();
        events.send(Incoming::Body { id, text });
    }

    fn doc_text(&self) -> String {
        let txn = self.awareness.doc().transact();
        self.text.get_string(&txn)
    }

    fn publish_awareness(&mut self, events: &Notify, force: bool) {
        let cursor = if force { None } else { self.last_cursor_sent };
        let state = serde_json::json!({
            "name": self.name,
            "color": self.color,
            "cursor": cursor,
        });
        if self.awareness.set_local_state(state).is_err() {
            return;
        }
        // Only this client's own state goes out. `update()` would include every peer it has
        // heard of, which would have the server credit this connection with other people's
        // presence and make leaving remove them too.
        let own = self.awareness.client_id();
        if let Ok(update) = self.awareness.update_with_clients([own]) {
            let bytes = update.encode_v1();
            let _ = self.send_frame(FRAME_AWARENESS, &bytes);
            events.send(Incoming::Peers(self.peers()));
        }
    }

    fn clear_awareness(&mut self) {
        let own = self.awareness.client_id();
        self.awareness.clean_local_state();
        // `update()` leaves out the states that were just cleared, so the removal has to be asked
        // for by id: otherwise leaving a note tells nobody, and the others keep drawing this
        // person's name and caret over it.
        if let Ok(update) = self.awareness.update_with_clients([own]) {
            let _ = self.send_frame(FRAME_AWARENESS, &update.encode_v1());
        }
    }

    /// Everyone else with this note open, from the awareness states the server relayed.
    fn peers(&self) -> Vec<RemotePeer> {
        let own = self.awareness.client_id();
        let mut out = Vec::new();
        for (client_id, state) in self.awareness.iter() {
            if client_id == own {
                continue;
            }
            let Some(state) = state.data.as_deref() else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(state) else {
                continue;
            };
            let Some(name) = value.get("name").and_then(|n| n.as_str()) else {
                continue;
            };
            let color = value
                .get("color")
                .and_then(|c| c.as_array())
                .and_then(|c| {
                    Some([
                        c.first()?.as_u64()? as u8,
                        c.get(1)?.as_u64()? as u8,
                        c.get(2)?.as_u64()? as u8,
                    ])
                })
                .unwrap_or([137, 180, 250]);
            let cursor = value
                .get("cursor")
                .and_then(|c| c.as_u64())
                .map(|c| c as usize);
            out.push(RemotePeer {
                name: name.to_string(),
                color,
                cursor,
                typing: false,
                updated_at: Instant::now(),
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }

    fn send_msg(&mut self, message: &ClientMsg) -> Result<(), String> {
        let payload = message.to_text().map_err(|e| e.to_string())?;
        self.send_frame(FRAME_JSON, payload.as_bytes())
    }

    fn send_frame(&mut self, kind: u8, payload: &[u8]) -> Result<(), String> {
        net::write_frame(&mut self.stream, kind, payload)
            .and_then(|_| self.stream.flush())
            .map_err(|e| format!("lost the server: {e}"))
    }
}

/// Commands that arrive while the handshake is in flight are dropped, except Disconnect.
fn drain_pending(commands: &Receiver<Command>) {
    while let Ok(command) = commands.try_recv() {
        if matches!(command, Command::Stop) {
            return;
        }
    }
}

/// One frame off the wire: its kind and its payload.
type Frame = (u8, Vec<u8>);

/// Reads what the socket has and returns the frames now complete. `Ok(None)` means nothing new.
fn read_frames(
    stream: &mut rustls::StreamOwned<rustls::ClientConnection, std::net::TcpStream>,
    buffer: &mut Vec<u8>,
) -> Result<Option<Vec<Frame>>, String> {
    let mut chunk = [0u8; 16 * 1024];
    let mut got = false;
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => return Err("the server closed the connection".to_string()),
            Ok(n) => {
                buffer.extend_from_slice(&chunk[..n]);
                got = true;
                if n < chunk.len() {
                    break;
                }
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.kind() == std::io::ErrorKind::TimedOut =>
            {
                break
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(format!("connection lost: {error}")),
        }
    }
    let frames = net::take_frames(buffer);
    if frames.is_empty() && !got {
        Ok(None)
    } else {
        Ok(Some(frames))
    }
}

fn color_for(name: &str) -> [u8; 3] {
    let mut hash: u64 = 1469598103934665603;
    for byte in name.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(1099511628211);
    }
    PEER_COLORS[(hash % PEER_COLORS.len() as u64) as usize]
}

/// The changed range between two versions of the text: `(start, removed, inserted)` in bytes.
/// Both ends are pulled back to character boundaries, so a multi-byte character is never cut.
pub fn diff(base: &str, new: &str) -> (usize, usize, usize) {
    let base_bytes = base.as_bytes();
    let new_bytes = new.as_bytes();
    let max = base_bytes.len().min(new_bytes.len());
    let mut prefix = 0;
    while prefix < max && base_bytes[prefix] == new_bytes[prefix] {
        prefix += 1;
    }
    while prefix > 0 && !base.is_char_boundary(prefix) {
        prefix -= 1;
    }

    let mut suffix = 0;
    let base_rest = base_bytes.len() - prefix;
    let new_rest = new_bytes.len() - prefix;
    let limit = base_rest.min(new_rest);
    while suffix < limit
        && base_bytes[base_bytes.len() - 1 - suffix] == new_bytes[new_bytes.len() - 1 - suffix]
    {
        suffix += 1;
    }
    while suffix > 0
        && (!base.is_char_boundary(base.len() - suffix)
            || !new.is_char_boundary(new.len() - suffix))
    {
        suffix -= 1;
    }

    (prefix, base_rest - suffix, new_rest - suffix)
}

/// Maps a byte position in `base` onto the matching position in `current`, where the two differ
/// only by edits made elsewhere. Falls back to counting from the end when the change is before
/// the position, which keeps the caret's text on either side intact.
fn map_position(base: &str, current: &str, position: usize) -> usize {
    let mut common = 0;
    let limit = base.len().min(current.len());
    while common < limit && base.as_bytes()[common] == current.as_bytes()[common] {
        common += 1;
    }
    while common > 0 && !current.is_char_boundary(common) {
        common -= 1;
    }
    if common >= position {
        position
    } else {
        current.len().saturating_sub(base.len() - position)
    }
}

/// Where a caret at `index` characters ends up after somebody else replaced `removed` characters
/// with `inserted` at `start`. Text they add at the caret itself is theirs, so the caret stays in
/// front of it; anything before the caret carries the caret along with the words around it.
pub fn carry_caret(start: usize, removed: usize, inserted: usize, index: usize) -> usize {
    if index <= start {
        index
    } else if removed > 0 && index <= start + removed {
        // The text the caret was in was replaced: it belongs at the end of the replacement.
        start + inserted
    } else {
        index + inserted - removed
    }
}

fn floor_char_boundary(text: &str, mut index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profile_address_accepts_host_and_port() {
        let mut profile = ServerProfile {
            host: "notes.example.com".into(),
            port: 7331,
            ..Default::default()
        };
        assert_eq!(profile.address(), "notes.example.com:7331");
        profile.host = "10.0.0.4:9000".into();
        assert_eq!(profile.address(), "10.0.0.4:9000");
        profile.host = "::1".into();
        assert_eq!(profile.address(), "[::1]:7331");
    }

    #[test]
    fn test_diff_finds_typing_at_the_end() {
        assert_eq!(diff("hello", "hello world"), (5, 0, 6));
        assert_eq!(diff("hello world", "hello"), (5, 6, 0));
        assert_eq!(diff("abc", "axc"), (1, 1, 1));
        assert_eq!(diff("", "hi"), (0, 0, 2));
    }

    #[test]
    fn test_diff_never_splits_a_multibyte_character() {
        let (start, removed, inserted) = diff("héllo", "héllo wörld");
        assert_eq!(&"héllo wörld"[start..start + inserted], " wörld");
        assert_eq!(removed, 0);
    }

    #[test]
    fn test_map_position_follows_remote_inserts() {
        // Someone typed "XY" before our caret: our position in the untouched tail moves along.
        assert_eq!(map_position("abc", "XYabc", 2), 4);
        // Someone typed after us: our position stays.
        assert_eq!(map_position("abc", "abcXY", 1), 1);
        // Someone deleted before us: we shift back.
        assert_eq!(map_position("abcde", "ade", 4), 2);
    }

    #[test]
    fn test_carried_caret_stays_with_its_own_text() {
        // A line inserted above the caret pushes it down by exactly that line.
        assert_eq!(carry_caret(10, 0, 5, 20), 25);
        // Typing after the caret leaves it alone.
        assert_eq!(carry_caret(30, 0, 3, 20), 20);
        // Inserted at the caret: the new text is theirs, so the caret stays in front of it.
        assert_eq!(carry_caret(20, 0, 4, 20), 20);
        // A line deleted above the caret brings it back up with its text.
        assert_eq!(carry_caret(10, 6, 0, 20), 14);
        // The surrounding text was replaced: the caret lands at the end of the replacement.
        assert_eq!(carry_caret(10, 5, 2, 12), 12);
    }

    #[test]
    fn test_color_for_is_stable_and_inside_the_palette() {
        assert_eq!(color_for("nova"), color_for("nova"));
        assert!(PEER_COLORS.contains(&color_for("nova")));
        assert!(PEER_COLORS.contains(&color_for("other person")));
    }
}
