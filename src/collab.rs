//! The client side of the shared library.
//!
//! A worker thread owns the TLS connection and the Yjs document for whichever note is open, so
//! the UI never blocks on the network. The UI keeps the text it draws (as it always has) and
//! hands every local change to the worker; the worker diffs it against what the document holds
//! and ships only the changed range, so two people typing at once merge instead of overwriting.
//!
//! Events come back through a channel and are drained once per frame by [`Collab::poll`].

use crate::net::{
    self, ClientMsg, Field, NoteMeta, PresenceUser, ServerMsg, ATTACHMENT_ASK, ATTACHMENT_DATA,
    ATTACHMENT_SEND, FRAME_ATTACHMENT, FRAME_AWARENESS, FRAME_JSON, FRAME_SYNC, PROTOCOL_VERSION,
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

/// How long the encrypted handshake may wait for the server's answer. It needs at least one
/// round trip, which is tens of milliseconds to a server across the internet; the short polling
/// interval below used to apply here too and made every such connection fail.
const HANDSHAKE_READ: Duration = Duration::from_secs(10);
/// How long a read waits before the worker looks at its command channel again while the
/// connection is busy.
const POLL: Duration = Duration::from_millis(8);
/// The same wait once the connection has been quiet for [`IDLE_AFTER`]. Anything the server
/// sends wakes a read at once, so only a local change waits for this, and the first one brings
/// the fast interval back.
const IDLE_POLL: Duration = Duration::from_millis(60);
/// How long nothing may happen before the worker slows its polling down.
const IDLE_AFTER: Duration = Duration::from_secs(1);
/// First retry delay after a dropped connection; doubles up to [`MAX_RETRY`].
const FIRST_RETRY: Duration = Duration::from_secs(2);
const MAX_RETRY: Duration = Duration::from_secs(15);
/// Cursor updates are throttled: a caret that moves every keystroke does not need every frame.
/// Whatever the throttle holds back is sent as soon as it allows, so the newest caret always
/// ends up on the other side.
const CURSOR_INTERVAL: Duration = Duration::from_millis(60);
/// How often a quiet connection tells the server it is still there. A note nobody types in sends
/// nothing at all, and the server closes a connection that has not spoken in a couple of minutes
/// — which took everybody in that note offline together, and read as the connection randomly
/// dropping.
const PING_INTERVAL: Duration = Duration::from_secs(20);
/// How long a ping goes unanswered before the link is treated as gone. A connection killed by a
/// router or a sleeping laptop does not close cleanly: without this, the app would sit on a dead
/// socket until the next keystroke happened to fail.
const PONG_TIMEOUT: Duration = Duration::from_secs(50);
/// How many versions of the text the window may have been left showing. A window draws once a
/// frame, so one or two is the honest lag; keeping a few more costs little and covers a stalled
/// frame.
const RECENT_TEXTS: usize = 4;
/// How long an address is given to answer when the connection is opened. A port that is
/// filtered rather than refused (the usual result of a missing port forward) would otherwise
/// leave the app in "Connecting…" for minutes, which looks like it has hung.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

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
    /// A picture the library sent, because this client asked for it.
    Attachment {
        name: String,
        bytes: Vec<u8>,
    },
    /// The library does not have that picture (or refused to keep one this client sent).
    AttachmentMissing {
        name: String,
    },
    /// A picture this client sent is in the library now, so everybody can see it.
    AttachmentStored {
        name: String,
    },
    /// Somebody else put a picture in the library; a request made before it arrived is worth
    /// making again.
    AttachmentAdded {
        name: String,
    },
    /// The server turned one request down: a note that is not yours to change, or one that is
    /// not shared with you. The connection is still up, so this is a message, not an outage.
    Refused(String),
    Offline(String),
    /// A server with no pinned certificate yet presented this fingerprint. Nothing was sent to
    /// it, and the worker has stopped: the password and identifier only go to a server whose
    /// certificate the user has confirmed, so the next step is theirs.
    Unverified {
        fingerprint: String,
    },
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
    /// The body is what the window is showing as the note opens, so the worker can tell a stale
    /// copy from a real edit made before the note's own text arrives.
    Open {
        id: String,
        body: String,
    },
    Close,
    /// Text for a specific note. The id matters: a body queued for the note just left must never
    /// be written into the one that replaced it.
    Body {
        id: String,
        text: String,
    },
    Cursor {
        id: String,
        byte: usize,
    },
    /// The name other people see for this connection.
    SetName(String),
    /// The colour this person's caret is drawn in on everybody else's screen. `None` goes back to
    /// a colour picked from the name.
    SetColor(Option<[u8; 3]>),
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
    /// Who may see one note: only its owner and the names listed, or the whole server.
    Share {
        id: String,
        private: bool,
        shared_with: Vec<String>,
    },
    Delete {
        id: String,
        permanent: bool,
    },
    /// A picture for a shared note, sent to the library.
    UploadAttachment {
        name: String,
        bytes: Vec<u8>,
    },
    /// A picture named by a shared note that this computer has not seen yet.
    FetchAttachment {
        name: String,
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
    /// Why the last connection attempt failed, for Settings to show while it retries.
    last_error: Option<String>,
    /// Called by the worker when a message arrives, to redraw the window straight away.
    wake: Option<Waker>,
    /// The colour this person chose to be drawn in on other people's screens, if they chose one.
    /// Kept here rather than in the session so a connection made later already knows it.
    color: Option<[u8; 3]>,
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
            last_error: None,
            wake: None,
            color: None,
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

    /// Why the last connection attempt failed, when it did.
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    pub fn pending_fingerprint(&self) -> Option<&str> {
        self.pending_fingerprint.as_deref()
    }

    pub fn take_fingerprint(&mut self) -> Option<String> {
        self.pending_fingerprint.take()
    }

    /// Starts a worker thread for `profile`. Any previous connection is dropped first.
    /// Opens a connection as one person: `username` is the display name everybody sees, and
    /// `identifier` is the secret key only the server reads. Both come from Settings.
    pub fn connect(&mut self, profile: ServerProfile, username: String, identifier: String) {
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
        let color = self.color;
        std::thread::Builder::new()
            .name("elora-collab".to_string())
            .spawn(move || worker(profile, username, identifier, color, command_rx, notify))
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
                self.last_error = None;
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
            Incoming::Refused(message) => self.last_error = Some(message.clone()),
            Incoming::Offline(reason) => {
                self.state = ConnectionState::Offline;
                self.last_error = Some(reason.clone());
                self.peers.clear();
                self.presence.clear();
            }
            Incoming::Unverified { fingerprint } => {
                self.state = ConnectionState::Offline;
                self.commands = None;
                self.last_error =
                    Some("Confirm the server's certificate code to finish connecting".to_string());
                self.pending_fingerprint = Some(fingerprint.clone());
            }
            Incoming::Body { .. }
            | Incoming::Created(_)
            | Incoming::Attachment { .. }
            | Incoming::AttachmentMissing { .. }
            | Incoming::AttachmentStored { .. }
            | Incoming::AttachmentAdded { .. } => {}
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
        // No guard on the note already open: after a reconnect the worker has nothing open and
        // has to be told again, while the window still thinks it is showing that note. Asking
        // twice costs one ignored message; the worker ignores a repeat for the note it has.
        self.open = Some(id.to_string());
        self.sent_body = Some((id.to_string(), body.to_string()));
        self.peers.clear();
        self.send(Command::Open {
            id: id.to_string(),
            body: body.to_string(),
        });
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
            id: id.to_string(),
            text: body.to_string(),
        });
    }

    /// Changes the name shown next to this client's caret, without reconnecting.
    pub fn set_name(&mut self, name: &str) {
        if !self.is_connected() {
            return;
        }
        self.send(Command::SetName(name.to_string()));
    }

    /// The colour this person is drawn in on everybody else's screen: one they chose, or `None`
    /// to let the name decide. Remembered for the next connection too.
    pub fn set_color(&mut self, color: Option<[u8; 3]>) {
        self.color = color;
        if self.is_connected() {
            self.send(Command::SetColor(color));
        }
    }

    /// Sends a caret position for `note_id`, in bytes.
    pub fn send_cursor(&mut self, note_id: &str, byte_index: usize) {
        if self.is_connected() {
            self.send(Command::Cursor {
                id: note_id.to_string(),
                byte: byte_index,
            });
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

    /// Restricts a shared note, opens it again, or changes who is named on it. The server checks
    /// that the note is the caller's before it answers, and the new library arrives as
    /// [`Incoming::Notes`].
    pub fn set_share(&mut self, id: &str, private: bool, shared_with: Vec<String>) {
        self.send(Command::Share {
            id: id.to_string(),
            private,
            shared_with,
        });
    }

    /// Sends a picture to the library, so everyone with the note can see it and it stays after
    /// this computer is gone. The answer arrives as [`Incoming::AttachmentStored`] or
    /// [`Incoming::AttachmentMissing`].
    pub fn upload_attachment(&mut self, name: &str, bytes: Vec<u8>) {
        self.send(Command::UploadAttachment {
            name: name.to_string(),
            bytes,
        });
    }

    /// Asks the library for a picture this computer does not have; it arrives as
    /// [`Incoming::Attachment`].
    pub fn fetch_attachment(&mut self, name: &str) {
        self.send(Command::FetchAttachment {
            name: name.to_string(),
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
    /// True from asking for a note until its state arrives. Typing in that window is held back:
    /// the editor is showing the note from the last time it was open, so sending it now would
    /// write that whole text into the room and duplicate what is already there.
    awaiting_room_state: bool,
    /// Text the window was showing when the note was opened. Anything it sends that is still
    /// exactly this is a leftover copy, not something the person typed.
    opened_with: String,
    deferred_body: Option<String>,
    buffer: Vec<u8>,
    last_cursor: Instant,
    last_cursor_sent: Option<usize>,
    /// A caret the throttle is holding back. It is published the moment the interval allows,
    /// even when no further keystroke arrives to carry it out.
    pending_cursor: Option<usize>,
    /// Versions the window may still be showing, newest first. A keystroke typed before the
    /// window drew the newest text was made on one of these, and placing it on the right one is
    /// what stops a frame's worth of somebody else's words from being wiped out.
    recent: Vec<String>,
    /// The text the window last sent. It sends the whole text every frame, so the same text again
    /// is not an edit — and must not be applied as one.
    window: String,
    /// When the last keepalive ping went out, and when the one still unanswered was sent.
    last_ping: Instant,
    pending_pong: Option<Instant>,
    name: String,
    color: [u8; 3],
}

/// Opens the socket to the server, trying each address it resolves to and giving up on one that
/// never answers. The error says what a failed attempt usually means, because the app has no
/// other way to tell somebody their router is not forwarding the port.
fn connect_socket(address: &str, timeout: Duration) -> Result<std::net::TcpStream, String> {
    use std::net::ToSocketAddrs;
    let candidates: Vec<std::net::SocketAddr> = address
        .to_socket_addrs()
        .map_err(|e| format!("could not look up {address}: {e}"))?
        .collect();
    if candidates.is_empty() {
        return Err(format!("{address} did not resolve to an address"));
    }
    let mut last = None;
    for candidate in candidates.into_iter().take(4) {
        match std::net::TcpStream::connect_timeout(&candidate, timeout) {
            Ok(socket) => return Ok(socket),
            Err(error) => last = Some(error),
        }
    }
    Err(format!(
        "could not reach {address}: {}. Check that the server is running, that its port is \
         allowed through its firewall, and that your router forwards that port to it.",
        last.map(|e| e.to_string()).unwrap_or_default()
    ))
}

fn worker(
    profile: ServerProfile,
    name: String,
    identifier: String,
    mut color: Option<[u8; 3]>,
    commands: Receiver<Command>,
    events: Notify,
) {
    let mut delay = FIRST_RETRY;
    loop {
        if matches!(
            commands.try_recv(),
            Ok(Command::Stop) | Err(TryRecvError::Disconnected)
        ) {
            return;
        }
        // Nothing pinned yet: learn the certificate, hand it to the user, and stop. The
        // handshake below sends the password and identifier, so it waits for a pinned key.
        if profile.fingerprint.is_none() {
            match probe_fingerprint(&profile) {
                Ok(fingerprint) => {
                    events.send(Incoming::Unverified { fingerprint });
                    return;
                }
                Err(error) => events.send(Incoming::Offline(error)),
            }
        } else {
            match Session::start(&profile, &name, color, &identifier, &events, &commands) {
                Ok(mut session) => {
                    delay = FIRST_RETRY;
                    let reason = session.run(&commands, &events);
                    events.send(Incoming::Offline(reason));
                }
                Err(error) => {
                    events.send(Incoming::Offline(error));
                }
            }
        }

        // Wait before retrying, but stay responsive to Disconnect.
        let deadline = Instant::now() + delay;
        while Instant::now() < deadline {
            match commands.recv_timeout(Duration::from_millis(50)) {
                Ok(Command::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                // A colour picked while the connection is down belongs to the next attempt, not
                // to the one that already ended.
                Ok(Command::SetColor(chosen)) => color = chosen,
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        delay = (delay * 2).min(MAX_RETRY);
    }
}

/// Runs only the TLS handshake with a server and returns the fingerprint of the certificate it
/// presented. No Elora message is written, so nothing secret reaches a server that has not been
/// confirmed yet.
fn probe_fingerprint(profile: &ServerProfile) -> Result<String, String> {
    let address = profile.address();
    let mut socket = connect_socket(&address, CONNECT_TIMEOUT)?;
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    socket
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    let (config, seen) = crate::tls::client_config(None);
    let server_name = rustls::pki_types::ServerName::try_from(profile.host.trim().to_string())
        .unwrap_or_else(|_| {
            rustls::pki_types::ServerName::try_from("elora-server").expect("valid name")
        });
    let mut connection = rustls::ClientConnection::new(config, server_name)
        .map_err(|e| format!("could not start TLS: {e}"))?;
    while connection.is_handshaking() {
        connection
            .complete_io(&mut socket)
            .map_err(|e| format!("could not reach {address} over TLS: {e}"))?;
    }
    connection.send_close_notify();
    let _ = connection.complete_io(&mut socket);
    seen.lock()
        .ok()
        .and_then(|slot| slot.clone())
        .ok_or_else(|| "the server presented no certificate".to_string())
}

impl Session {
    fn start(
        profile: &ServerProfile,
        name: &str,
        color: Option<[u8; 3]>,
        identifier: &str,
        events: &Notify,
        commands: &Receiver<Command>,
    ) -> Result<Session, String> {
        let address = profile.address();
        let socket = connect_socket(&address, CONNECT_TIMEOUT)?;
        socket.set_nodelay(true).ok();
        socket
            .set_read_timeout(Some(HANDSHAKE_READ))
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

        // Two different things: the name is what everybody sees, the identifier is the key only
        // the server reads. Sending the name twice, as older builds did, would make the key as
        // public as the name.
        let hello = ClientMsg::Hello {
            v: PROTOCOL_VERSION,
            id: identifier.to_string(),
            name: name.to_string(),
            password: profile.password.clone(),
        };
        let payload = serde_json::to_vec(&hello).map_err(|e| e.to_string())?;
        net::write_frame(&mut stream, FRAME_JSON, &payload)
            .and_then(|_| stream.flush())
            .map_err(|e| format!("could not send the handshake: {e}"))?;
        // The encrypted handshake finished inside that write; from here the worker polls.
        stream
            .sock
            .set_read_timeout(Some(POLL))
            .map_err(|e| e.to_string())?;

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
            opened_with: String::new(),
            deferred_body: None,
            buffer,
            last_cursor: Instant::now(),
            last_cursor_sent: None,
            pending_cursor: None,
            recent: Vec::new(),
            window: String::new(),
            last_ping: Instant::now(),
            pending_pong: None,
            name: name.to_string(),
            color: color.unwrap_or_else(|| default_peer_color(name)),
        };
        session.publish_awareness(events, true);
        Ok(session)
    }

    fn run(&mut self, commands: &Receiver<Command>, events: &Notify) -> String {
        let mut last_activity = Instant::now();
        let mut polling = POLL;
        loop {
            loop {
                match commands.try_recv() {
                    Ok(Command::Stop) | Err(TryRecvError::Disconnected) => {
                        return "disconnected".to_string()
                    }
                    Ok(command) => {
                        last_activity = Instant::now();
                        if let Err(error) = self.handle(command, events) {
                            return error;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                }
            }

            if let Err(error) = self.keep_alive(events) {
                return error;
            }

            // A quiet connection looks at its commands less often, which is most of the time
            // the app is open; the fast interval returns with the next change or frame.
            let wanted = poll_interval(last_activity.elapsed());
            if wanted != polling {
                polling = wanted;
                self.stream.sock.set_read_timeout(Some(polling)).ok();
            }

            match read_frames(&mut self.stream, &mut self.buffer) {
                Ok(Some(frames)) => {
                    last_activity = Instant::now();
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
            Command::Open { id, body } => {
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
                // Until the note's own text arrives, the window is still showing what it had, so
                // that copy is what a keystroke made in the meantime is based on.
                self.recent = vec![body.clone()];
                self.window = body.clone();
                self.opened_with = body;
                self.deferred_body = None;
                self.send_msg(&ClientMsg::Open { id: id.clone() })?;
                self.open = Some(id);
                self.last_cursor_sent = None;
                self.pending_cursor = None;
                // Announce ourselves in the room so the presence list has a name to show even
                // before this person moves their caret.
                self.publish_awareness(events, true);
            }
            Command::Close => {
                self.open = None;
                self.awaiting_room_state = false;
                self.deferred_body = None;
                self.recent.clear();
                self.window.clear();
                self.mirror.clear();
                self.clear_awareness();
                self.send_msg(&ClientMsg::Close)?;
                events.send(Incoming::Peers(Vec::new()));
            }
            Command::Body { id, text } => {
                // The window may still be sending the note it had open a moment ago.
                if self.open.as_deref() != Some(id.as_str()) {
                    return Ok(());
                }
                if self.awaiting_room_state {
                    self.deferred_body = Some(text);
                    return Ok(());
                }
                self.apply_local(&text, events)?;
            }
            Command::SetName(name) => {
                self.name = name;
                // Sent as an ordinary awareness change, so the people in this note see the new
                // name without the connection being made again.
                self.publish_awareness(events, false);
                return Ok(());
            }
            Command::SetColor(color) => {
                // A fresh caret position is not news, so the published state keeps the last one.
                self.color = color.unwrap_or_else(|| default_peer_color(&self.name));
                self.publish_awareness(events, false);
                return Ok(());
            }
            Command::Cursor { id, byte } => {
                if self.open.as_deref() != Some(id.as_str()) {
                    return Ok(());
                }
                // The window's caret counts bytes into the text it is showing, which can be a
                // frame behind the document another person is already typing into.
                let byte = map_position(&self.mirror, &self.doc_text(), byte);
                if self.last_cursor_sent == Some(byte) {
                    self.pending_cursor = None;
                    return Ok(());
                }
                self.pending_cursor = Some(byte);
                // The first caret goes out immediately; after that it is throttled so typing
                // does not turn into a stream of awareness frames. A held-back one is not
                // dropped: `keep_alive` sends the newest as soon as the interval is up.
                if self.last_cursor_sent.is_none() || self.last_cursor.elapsed() >= CURSOR_INTERVAL
                {
                    self.publish_cursor(events);
                }
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
                // A note with no folder or icon says so explicitly: `Keep` would leave the old
                // one on the server, which is how a shared note could be filed but never
                // taken out of its folder again.
                folder: meta.folder.map(Field::Set).unwrap_or(Field::Clear),
                tags: Some(meta.tags),
                pinned: Some(meta.pinned),
                trashed: Some(meta.trashed),
                icon: meta.icon.map(Field::Set).unwrap_or(Field::Clear),
            })?,
            Command::Share {
                id,
                private,
                shared_with,
            } => self.send_msg(&ClientMsg::SetShare {
                id,
                private,
                shared_with,
            })?,
            Command::Delete { id, permanent } => {
                self.send_msg(&ClientMsg::Delete { id, permanent })?
            }
            Command::UploadAttachment { name, bytes } => self.send_frame(
                FRAME_ATTACHMENT,
                &net::encode_attachment(ATTACHMENT_SEND, &name, &bytes),
            )?,
            Command::FetchAttachment { name } => self.send_frame(
                FRAME_ATTACHMENT,
                &net::encode_attachment(ATTACHMENT_ASK, &name, b""),
            )?,
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
                    ServerMsg::Attachment { name } => {
                        events.send(Incoming::AttachmentAdded { name })
                    }
                    // A refusal is not a reason to drop the connection: the library is still
                    // there, and the note that was turned down simply is not this client's.
                    ServerMsg::Error { message } => events.send(Incoming::Refused(message)),
                    // The link is alive in both directions: nothing is waiting to be resent and
                    // the server has not gone away.
                    ServerMsg::Pong => self.pending_pong = None,
                    _ => {}
                }
            }
            FRAME_SYNC => {
                // Every sync frame names its note. One that arrives after a switch belongs to the
                // note just left, and applying it would put that note's text into this one.
                let Some((note_id, message)) = crate::net::decode_sync(&payload) else {
                    return Ok(());
                };
                if self.open.as_deref() != Some(note_id) {
                    return Ok(());
                }
                let Ok(message) = YMessage::decode_v1(message) else {
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
                        let tagged = crate::net::encode_sync(note_id, &reply);
                        self.send_frame(FRAME_SYNC, &tagged)?;
                    }
                    YMessage::Sync(SyncMessage::SyncStep2(update)) => {
                        // The room's own state, sent as soon as the note opens.
                        self.awaiting_room_state = false;
                        self.apply_remote(update)?;
                        // Hand the window what the room holds even when that is what the
                        // connection already had: the app's copy of the note can be out of date
                        // from an earlier session, and this is what puts it right.
                        self.emit_body_now(events);
                        // Now that the note's own text is here, an edit made while waiting can be
                        // sent as the small change it is. Text that is simply the copy the note
                        // was opened with is not an edit: it is whatever the window was showing
                        // before, and the room's own text has just replaced it.
                        if let Some(text) = self.deferred_body.take() {
                            if text != self.opened_with {
                                self.apply_local(&text, events)?;
                            }
                        }
                    }
                    YMessage::Sync(SyncMessage::Update(update)) => {
                        self.apply_remote(update)?;
                        self.emit_body(events);
                    }
                    _ => {}
                }
            }
            FRAME_ATTACHMENT => {
                let Some((op, name, bytes)) = net::decode_attachment(&payload) else {
                    return Ok(());
                };
                match op {
                    ATTACHMENT_DATA if bytes.is_empty() => {
                        events.send(Incoming::AttachmentStored {
                            name: name.to_string(),
                        })
                    }
                    ATTACHMENT_DATA => events.send(Incoming::Attachment {
                        name: name.to_string(),
                        bytes: bytes.to_vec(),
                    }),
                    net::ATTACHMENT_MISSING => events.send(Incoming::AttachmentMissing {
                        name: name.to_string(),
                    }),
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
    fn apply_remote(&mut self, update: Vec<u8>) -> Result<(), String> {
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
        Ok(())
    }

    /// Hands the window the document's text even when it has not changed here, so opening a
    /// note always replaces whatever the app was showing for it.
    fn emit_body_now(&mut self, events: &Notify) {
        let Some(id) = self.open.clone() else {
            return;
        };
        let text = self.doc_text();
        self.hand_off(&text);
        events.send(Incoming::Body { id, text });
    }

    /// Applies what the user typed to the shared document. Only the changed range is sent, and
    /// the position is mapped into the document so a remote insert elsewhere does not make the
    /// local text land in the wrong place.
    ///
    /// The document is the authority: whatever it holds afterwards is handed back to the window
    /// whenever the two disagree. Leaving the window with text the document never received is
    /// what made a note show words nobody else had — and miss words it did have.
    fn apply_local(&mut self, text: &str, events: &Notify) -> Result<(), String> {
        if self.open.is_none() {
            return Ok(());
        }
        // The window hands over its whole text every frame. The same text again is not an edit,
        // however far the document has moved on in the meantime.
        if text == self.window {
            return Ok(());
        }
        self.window = text.to_string();
        // The window can be a frame behind: somebody's words arrived, and it has not drawn them
        // yet. What this person typed belongs on top of the text it is showing, not on top of the
        // text it has not seen — taking its text as the whole truth would wipe those words out.
        let Some((base, age)) = self.base_for(text) else {
            return Ok(());
        };
        // Nothing older than the version it typed on can still be on screen.
        self.recent.truncate(age);
        let current = self.doc_text();
        let Some((at, removed, inserted)) = local_patch(&base, &current, text) else {
            // Nothing to write: the text sent is the version it was already given.
            if age == 0 {
                self.mirror = text.to_string();
            }
            return Ok(());
        };
        let update = self.write_range(at, removed, inserted);
        self.send_update(update)?;

        let doc = self.doc_text();
        if doc == text {
            self.mirror = doc;
        } else if let Some(id) = self.open.clone() {
            // The document had to be worked around what arrived meanwhile, so it is the document
            // that the window should be showing.
            self.hand_off(&doc);
            events.send(Incoming::Body { id, text: doc });
        }
        Ok(())
    }

    /// The text out of what the window has been handed that `text` was most likely typed on:
    /// the one it is the smallest change from. The version it is on, and how old that is.
    fn base_for(&self, text: &str) -> Option<(String, usize)> {
        let mut best: Option<(String, usize, usize)> = None;
        for (age, candidate) in std::iter::once(&self.mirror)
            .chain(self.recent.iter())
            .enumerate()
        {
            let (_, removed, inserted) = diff(candidate, text);
            let cost = removed + inserted;
            if best.as_ref().is_none_or(|(_, _, kept)| cost < *kept) {
                best = Some((candidate.clone(), age, cost));
            }
        }
        best.map(|(base, age, _)| (base, age))
    }

    /// Records that the window is being shown `text`, keeping what it was showing before in case
    /// a keystroke was made before it drew the new text.
    fn hand_off(&mut self, text: &str) {
        if !self.mirror.is_empty() && self.mirror != text {
            let previous = std::mem::take(&mut self.mirror);
            self.recent.insert(0, previous);
            self.recent.truncate(RECENT_TEXTS);
        }
        self.mirror = text.to_string();
    }

    /// Applies one local change and returns the Yjs update describing it. The update has to be
    /// encoded inside the transaction that made the change, otherwise it carries nothing.
    fn write_range(&mut self, at: usize, removed: usize, inserted: &str) -> Vec<u8> {
        let mut txn = self.awareness.doc_mut().transact_mut();
        // Every position here was meant for this document, but a range that runs off its end
        // would take the worker thread down with it, so it is pulled back inside first.
        let len = self.text.len(&txn) as usize;
        let at = at.min(len);
        let removed = removed.min(len - at);
        if removed > 0 {
            self.text.remove_range(&mut txn, at as u32, removed as u32);
        }
        if !inserted.is_empty() {
            self.text.insert(&mut txn, at as u32, inserted);
        }
        txn.encode_update_v1()
    }

    fn send_update(&mut self, update: Vec<u8>) -> Result<(), String> {
        let Some(id) = self.open.clone() else {
            return Ok(());
        };
        if update.is_empty() {
            return Ok(());
        }
        let frame = YMessage::Sync(SyncMessage::Update(update)).encode_v1();
        let tagged = crate::net::encode_sync(&id, &frame);
        self.send_frame(FRAME_SYNC, &tagged)
    }

    fn emit_body(&mut self, events: &Notify) {
        let Some(id) = self.open.clone() else {
            return;
        };
        let text = self.doc_text();
        if text == self.mirror {
            return;
        }
        self.hand_off(&text);
        events.send(Incoming::Body { id, text });
    }

    fn doc_text(&self) -> String {
        let txn = self.awareness.doc().transact();
        self.text.get_string(&txn)
    }

    /// The work that has to happen on a quiet connection too: the caret the throttle held back,
    /// and the ping that keeps the session open. Returns the reason the link is gone when it no
    /// longer answers.
    fn keep_alive(&mut self, events: &Notify) -> Result<(), String> {
        if self.pending_cursor.is_some() && self.last_cursor.elapsed() >= CURSOR_INTERVAL {
            self.publish_cursor(events);
        }
        match keepalive_action(
            self.last_ping.elapsed(),
            self.pending_pong.map(|s| s.elapsed()),
        ) {
            KeepAlive::Nothing => {}
            KeepAlive::Ping => {
                self.last_ping = Instant::now();
                self.pending_pong.get_or_insert_with(Instant::now);
                self.send_msg(&ClientMsg::Ping)?;
            }
            KeepAlive::Gone => {
                return Err("the server stopped answering; reconnecting".to_string());
            }
        }
        Ok(())
    }

    /// Sends the caret the throttle has been holding, now that it may.
    fn publish_cursor(&mut self, events: &Notify) {
        let Some(byte) = self.pending_cursor.take() else {
            return;
        };
        self.last_cursor = Instant::now();
        self.last_cursor_sent = Some(byte);
        self.publish_awareness(events, false);
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

/// The colour a name is drawn in when its person has not chosen one: a stable pick out of the
/// shared palette, so the same name is the same colour on every screen.
pub fn default_peer_color(name: &str) -> [u8; 3] {
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

/// How long the worker waits on the socket before it looks at its commands again, given how
/// long ago anything last happened on the connection.
fn poll_interval(quiet_for: Duration) -> Duration {
    if quiet_for >= IDLE_AFTER {
        IDLE_POLL
    } else {
        POLL
    }
}

/// What a connection that has heard nothing should do about it.
#[derive(Debug, PartialEq)]
enum KeepAlive {
    Nothing,
    /// Say it is still there, so the server does not close a note nobody is typing in.
    Ping,
    /// A ping went unanswered for long enough that the link is not carrying anything any more.
    Gone,
}

fn keepalive_action(since_ping: Duration, unanswered: Option<Duration>) -> KeepAlive {
    if unanswered.is_some_and(|waited| waited >= PONG_TIMEOUT) {
        KeepAlive::Gone
    } else if since_ping >= PING_INTERVAL {
        KeepAlive::Ping
    } else {
        KeepAlive::Nothing
    }
}

/// What to write into the document so the text the window typed lands on top of what the
/// document already holds: `(position, removed, inserted)`. `None` when there is nothing to
/// write, which is the case both for an unchanged window and for one that caught up on its own.
fn local_patch<'a>(base: &str, current: &str, text: &'a str) -> Option<(usize, usize, &'a str)> {
    if text == base || current == text {
        return None;
    }
    let (start, removed, inserted) = diff(base, text);
    let (at, removable) = place_change(base, current, start, removed);
    let at = floor_char_boundary(current, at.min(current.len()));
    // Deleting is only safe where the text being deleted is still the text that was meant: when
    // somebody else has just changed that spot, the words stay and the new text goes in beside
    // them, which loses no keystroke and can be fixed with a second backspace.
    let keep = removable
        && removed > 0
        && at + removed <= current.len()
        && current[at..].starts_with(&base[start..start + removed]);
    Some((
        at,
        if keep { removed } else { 0 },
        &text[start..start + inserted],
    ))
}

/// Maps a byte position in `base` onto the matching position in `current`, where the two differ
/// only by edits made elsewhere. A position inside text somebody else replaced lands at the
/// nearest unchanged end, so a caret is never left pointing into the middle of a word.
fn map_position(base: &str, current: &str, position: usize) -> usize {
    place_change(base, current, position, 0).0
}

/// Where a change made against `base` belongs in `current`, as `(position, may_delete)`.
///
/// The one case it cannot place honestly is a change landing inside text somebody else has just
/// replaced: it answers with the nearest unchanged end and says the deletion must not happen,
/// because the words there are no longer the ones the window meant to remove.
fn place_change(base: &str, current: &str, start: usize, removed: usize) -> (usize, bool) {
    let base_bytes = base.as_bytes();
    let current_bytes = current.as_bytes();
    let limit = base_bytes.len().min(current_bytes.len());

    let mut prefix = 0;
    while prefix < limit && base_bytes[prefix] == current_bytes[prefix] {
        prefix += 1;
    }
    while prefix > 0 && (!base.is_char_boundary(prefix) || !current.is_char_boundary(prefix)) {
        prefix -= 1;
    }

    let mut suffix = 0;
    let room = limit - prefix;
    while suffix < room
        && base_bytes[base_bytes.len() - 1 - suffix]
            == current_bytes[current_bytes.len() - 1 - suffix]
    {
        suffix += 1;
    }
    while suffix > 0
        && (!base.is_char_boundary(base.len() - suffix)
            || !current.is_char_boundary(current.len() - suffix))
    {
        suffix -= 1;
    }

    // The head and the tail of the two texts still agree; whatever changed is between them.
    let base_tail = base.len() - suffix;
    let current_tail = current.len() - suffix;
    if start + removed <= prefix {
        // Everything up to the change is untouched, so the position is its own.
        (start, true)
    } else if start >= base_tail {
        // The change is in the shared tail: it moves with it.
        (current_tail + (start - base_tail), true)
    } else if start - prefix <= base_tail - start {
        (prefix, false)
    } else {
        (current_tail, false)
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
    fn test_a_silent_address_is_given_up_on_with_a_reason() {
        // TEST-NET-1 is reserved and never answers, which is the shape of a port that is
        // filtered because nothing forwards it.
        let error = connect_socket("192.0.2.1:7331", Duration::from_millis(300)).unwrap_err();
        assert!(error.contains("could not reach 192.0.2.1:7331"), "{error}");
        assert!(error.contains("firewall"), "{error}");
        assert!(error.contains("router"), "{error}");
    }

    #[test]
    fn test_a_refused_port_says_so_at_once() {
        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        let error =
            connect_socket(&format!("127.0.0.1:{port}"), Duration::from_secs(2)).unwrap_err();
        assert!(
            error.contains(&format!("could not reach 127.0.0.1:{port}")),
            "{error}"
        );
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
    fn test_a_local_edit_lands_on_the_documents_own_text() {
        // Typing in the middle while somebody else appends at the end: both survive, in place.
        let (at, removed, inserted) = local_patch("abc", "abcZZ", "aXbc").unwrap();
        assert_eq!((at, removed, inserted), (1, 0, "X"));

        // A backspace while somebody else added a line above: the letter the person meant goes,
        // and the line above stays.
        let (at, removed, inserted) = local_patch("one\ntwo", "new\none\ntwo", "one\nwo").unwrap();
        assert_eq!((at, removed, inserted), (8, 1, ""));
        let mut current = String::from("new\none\ntwo");
        current.replace_range(at..at + removed, inserted);
        assert_eq!(current, "new\none\nwo");

        // Nothing was typed, or the window already shows the document: nothing to write.
        assert_eq!(local_patch("abc", "abc", "abc"), None);
        assert_eq!(local_patch("abc", "abcdef", "abcdef"), None);
        assert_eq!(local_patch("", "", "hello").unwrap(), (0, 0, "hello"));
    }

    #[test]
    fn test_a_replacement_is_not_deleted_out_of_text_somebody_else_just_wrote() {
        // The window means to turn "world" into "there", but the other person has just put
        // "earth" where those letters were. Their words stay, and no keystroke is thrown away.
        let (at, removed, inserted) = local_patch("say world", "say earth", "say there").unwrap();
        assert_eq!(removed, 0, "text that is not ours must not be deleted");
        assert_eq!(inserted, "there");
        let mut current = String::from("say earth");
        current.insert_str(at, inserted);
        assert!(
            current.contains("earth") && current.contains("there"),
            "{current:?}"
        );
        assert!(current.is_char_boundary(at));
    }

    #[test]
    fn test_a_quiet_connection_pings_before_it_gives_up() {
        assert_eq!(
            keepalive_action(Duration::from_secs(1), None),
            KeepAlive::Nothing
        );
        assert_eq!(
            keepalive_action(PING_INTERVAL, None),
            KeepAlive::Ping,
            "a note nobody types in still has to speak before the server drops it"
        );
        assert_eq!(
            keepalive_action(PING_INTERVAL, Some(Duration::from_secs(1))),
            KeepAlive::Ping,
            "a ping already waiting for its answer is not sent again"
        );
        assert_eq!(
            keepalive_action(PING_INTERVAL, Some(PONG_TIMEOUT)),
            KeepAlive::Gone,
            "an unanswered ping means the link is not carrying anything"
        );
    }

    #[test]
    fn test_a_quiet_connection_polls_slowly_and_a_busy_one_quickly() {
        assert_eq!(poll_interval(Duration::ZERO), POLL);
        assert_eq!(poll_interval(IDLE_AFTER - Duration::from_millis(1)), POLL);
        assert_eq!(poll_interval(IDLE_AFTER), IDLE_POLL);
        assert_eq!(poll_interval(Duration::from_secs(3600)), IDLE_POLL);
        // The slow interval must stay well inside what typing can notice, and the keep-alive
        // and cursor throttle are both coarser than it.
        assert!(IDLE_POLL <= CURSOR_INTERVAL);
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
        assert_eq!(default_peer_color("nova"), default_peer_color("nova"));
        assert!(PEER_COLORS.contains(&default_peer_color("nova")));
        assert!(PEER_COLORS.contains(&default_peer_color("other person")));
    }
}
