//! The EloraNotes collaboration server.
//!
//! One thread per connection, each owning its TLS stream. A connection loop reads whatever the
//! socket has (with a short read timeout), handles the frames it got, then drains its outbox, so
//! broadcasts never need to write to a socket another thread is reading.
//!
//! Note bodies are Yjs documents ([`yrs`]) which is what makes two people typing at once merge
//! instead of overwrite. The document is materialised back into the note's `.md` file, so the
//! server's folder stays an ordinary EloraNotes library.

use crate::net::{
    self, ClientMsg, Field, NoteMeta, PresenceUser, ServerConfig, ServerMsg, FRAME_AWARENESS,
    FRAME_JSON, FRAME_SYNC, PROTOCOL_VERSION,
};
use crate::storage::Note;
use chrono::{TimeZone, Utc};
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use yrs::sync::awareness::AwarenessUpdateEntry;
use yrs::sync::{AwarenessUpdate, Message as YMessage, SyncMessage};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{ClientID, Doc, GetString, ReadTxn, StateVector, Text, TextRef, Transact};

/// Read timeout for a connection: short enough that queued broadcasts go out promptly.
const READ_TIMEOUT: Duration = Duration::from_millis(8);
/// A room is written to disk at most this often while people are typing.
const WRITE_DEBOUNCE: Duration = Duration::from_millis(400);
/// How long a silent connection is kept before it is dropped.
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

pub struct Server {
    config: ServerConfig,
    tls: Arc<rustls::ServerConfig>,
    library: Mutex<Library>,
    clients: Mutex<HashMap<u64, Arc<Client>>>,
    next_id: AtomicU64,
}

/// Everything the connection thread needs to hand work to other connections.
struct Client {
    id: u64,
    name: Mutex<String>,
    room: Mutex<Option<String>>,
    /// The awareness ids this connection has published, so leaving the room removes exactly
    /// those states. Matching on the name would make two people who happen to use the same
    /// account name disappear together.
    awareness: Mutex<HashSet<u64>>,
    authed: AtomicBool,
    outbox: Mutex<Vec<Frame>>,
}

type Frame = (u8, Vec<u8>);

struct Library {
    dir: PathBuf,
    notes: HashMap<String, NoteMeta>,
    rooms: HashMap<String, Room>,
    /// Notes whose `.md` is behind the document, and when it was last written.
    dirty: HashMap<String, Instant>,
}

struct Room {
    doc: Doc,
    text: TextRef,
    /// Awareness client id -> what that client last published (name, colour, cursor) and the
    /// clock it published under. The server only relays these; it never needs a Yjs client
    /// identity of its own, but it does keep the clock so it can tell the others that somebody
    /// left with an update they will accept.
    awareness: HashMap<u64, PeerState>,
    subscribers: HashSet<u64>,
}

/// One person's presence in a room, as they last published it.
struct PeerState {
    clock: u32,
    json: String,
}

impl Server {
    pub fn new(config: ServerConfig) -> Result<Self, String> {
        std::fs::create_dir_all(&config.notes_dir)
            .map_err(|e| format!("could not create {}: {e}", config.notes_dir.display()))?;
        let tls = crate::tls::server_config(&config)?;
        let library = Library::load(&config.notes_dir)?;
        Ok(Self {
            config,
            tls,
            library: Mutex::new(library),
            clients: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        })
    }

    pub fn config(&self) -> &ServerConfig {
        &self.config
    }

    /// Accepts connections until the listener fails.
    pub fn run(self: Arc<Self>) -> Result<(), String> {
        let addr = format!("{}:{}", self.config.bind, self.config.port);
        let listener =
            TcpListener::bind(&addr).map_err(|e| format!("could not listen on {addr}: {e}"))?;
        eprintln!(
            "elora-server \"{}\" listening on {addr} — {} note(s) in {}",
            self.config.name,
            self.library.lock().map(|l| l.notes.len()).unwrap_or(0),
            self.config.notes_dir.display()
        );
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let server = Arc::clone(&self);
                    std::thread::spawn(move || {
                        if let Err(error) = server.serve(stream) {
                            eprintln!("connection ended: {error}");
                        }
                    });
                }
                Err(error) => eprintln!("accept failed: {error}"),
            }
        }
        Ok(())
    }

    fn serve(self: Arc<Self>, stream: TcpStream) -> Result<(), String> {
        stream
            .set_read_timeout(Some(READ_TIMEOUT))
            .map_err(|e| e.to_string())?;
        stream
            .set_write_timeout(Some(Duration::from_secs(10)))
            .map_err(|e| e.to_string())?;
        stream.set_nodelay(true).ok();

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let client = Arc::new(Client {
            id,
            name: Mutex::new(String::new()),
            room: Mutex::new(None),
            awareness: Mutex::new(HashSet::new()),
            authed: AtomicBool::new(false),
            outbox: Mutex::new(Vec::new()),
        });
        self.clients
            .lock()
            .map_err(|_| "server state poisoned".to_string())?
            .insert(id, Arc::clone(&client));

        let conn = rustls::ServerConnection::new(Arc::clone(&self.tls))
            .map_err(|e| format!("TLS handshake failed: {e}"))?;
        let mut tls = rustls::StreamOwned::new(conn, stream);
        let result = self.connection_loop(&client, &mut tls);
        self.disconnect(&client);
        result
    }

    fn connection_loop(
        &self,
        client: &Arc<Client>,
        tls: &mut rustls::StreamOwned<rustls::ServerConnection, TcpStream>,
    ) -> Result<(), String> {
        let mut buffer: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 16 * 1024];
        let mut last_contact = Instant::now();

        loop {
            // Hand over anything other connections queued for us.
            self.drain_outbox(client, tls)?;

            match tls.read(&mut chunk) {
                Ok(0) => return Ok(()),
                Ok(n) => {
                    last_contact = Instant::now();
                    buffer.extend_from_slice(&chunk[..n]);
                }
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        || error.kind() == std::io::ErrorKind::TimedOut => {}
                // A client that vanishes without saying goodbye (a killed app, a dropped
                // network) is an ordinary disconnect, not something to log as an error.
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => return Err(format!("read failed: {error}")),
            }

            for (kind, payload) in net::take_frames(&mut buffer) {
                if !client.authed.load(Ordering::Relaxed) {
                    if kind != FRAME_JSON {
                        return Err("the first message must be the handshake".to_string());
                    }
                    if !self.handshake(client, &payload)? {
                        // The reply explains why the connection is going away (an older client,
                        // a wrong password). Dropping the socket here would leave the other
                        // side with nothing but "disconnected".
                        self.drain_outbox(client, tls)?;
                        return Ok(());
                    }
                    continue;
                }
                if kind == FRAME_JSON {
                    if let Err(error) = self.handle_control(client, &payload) {
                        self.push(
                            client,
                            FRAME_JSON,
                            json(&ServerMsg::Error {
                                message: error.to_string(),
                            }),
                        );
                    }
                } else {
                    self.handle_doc_frame(client, kind, payload);
                }
            }

            self.flush_dirty();

            if last_contact.elapsed() > IDLE_TIMEOUT {
                return Ok(());
            }
            // Nothing waiting: sleep a little so the loop does not spin on an idle socket.
            if buffer.is_empty() {
                std::thread::sleep(Duration::from_millis(4));
            }
        }
    }

    fn drain_outbox(
        &self,
        client: &Arc<Client>,
        tls: &mut rustls::StreamOwned<rustls::ServerConnection, TcpStream>,
    ) -> Result<(), String> {
        let frames: Vec<Frame> = {
            let mut outbox = client.outbox.lock().map_err(|_| "outbox poisoned")?;
            if outbox.is_empty() {
                return Ok(());
            }
            std::mem::take(&mut *outbox)
        };
        for (kind, payload) in frames {
            net::write_frame(tls, kind, &payload).map_err(|e| format!("write failed: {e}"))?;
        }
        tls.flush().map_err(|e| format!("flush failed: {e}"))
    }

    fn handshake(&self, client: &Arc<Client>, payload: &[u8]) -> Result<bool, String> {
        let message: ClientMsg = serde_json::from_slice(payload)
            .map_err(|e| format!("could not read the handshake: {e}"))?;
        let ClientMsg::Hello { v, name, password } = message else {
            return Err("the first message must be the handshake".to_string());
        };
        if v != PROTOCOL_VERSION {
            self.push(
                client,
                FRAME_JSON,
                json(&ServerMsg::Error {
                    message: format!(
                        "this server speaks version {PROTOCOL_VERSION}; the client speaks {v}. \
                         Update whichever side is older."
                    ),
                }),
            );
            return Ok(false);
        }
        if !constant_time_eq(password.as_bytes(), self.config.password.as_bytes()) {
            self.push(
                client,
                FRAME_JSON,
                json(&ServerMsg::Error {
                    message: "wrong password".to_string(),
                }),
            );
            return Ok(false);
        }
        let name = if name.trim().is_empty() {
            "guest".to_string()
        } else {
            name.trim().to_string()
        };
        if let Ok(mut slot) = client.name.lock() {
            *slot = name.clone();
        }
        client.authed.store(true, Ordering::Relaxed);
        eprintln!("{name} connected from the network");

        let notes = self.library.lock().map_err(|_| "library poisoned")?.list();
        self.push(
            client,
            FRAME_JSON,
            json(&ServerMsg::Welcome {
                v: PROTOCOL_VERSION,
                server: self.config.name.clone(),
                you: name,
                notes,
            }),
        );
        Ok(true)
    }

    fn handle_control(&self, client: &Arc<Client>, payload: &[u8]) -> Result<(), String> {
        let message: ClientMsg = serde_json::from_slice(payload)
            .map_err(|e| format!("could not read the message: {e}"))?;
        match message {
            ClientMsg::Hello { .. } => Err("already connected".to_string()),
            ClientMsg::Ping => {
                self.push(client, FRAME_JSON, json(&ServerMsg::Pong));
                Ok(())
            }
            ClientMsg::Open { id } => self.open_room(client, &id),
            ClientMsg::Close => {
                self.leave_room(client);
                Ok(())
            }
            ClientMsg::Create {
                title,
                folder,
                tags,
                body,
                pinned,
                icon,
            } => {
                let meta = {
                    let mut library = self.library.lock().map_err(|_| "library poisoned")?;
                    library.create(&title, folder, tags, &body, pinned, icon)?
                };
                self.broadcast_library();
                self.push(client, FRAME_JSON, json(&ServerMsg::Created { note: meta }));
                Ok(())
            }
            ClientMsg::Update {
                id,
                title,
                folder,
                tags,
                pinned,
                trashed,
                icon,
            } => {
                {
                    let mut library = self.library.lock().map_err(|_| "library poisoned")?;
                    library.update(&id, title, folder, tags, pinned, trashed, icon)?;
                }
                self.broadcast_library();
                Ok(())
            }
            ClientMsg::Delete { id, permanent } => {
                {
                    let mut library = self.library.lock().map_err(|_| "library poisoned")?;
                    library.delete(&id, permanent)?;
                }
                self.broadcast_library();
                Ok(())
            }
        }
    }

    fn open_room(&self, client: &Arc<Client>, id: &str) -> Result<(), String> {
        // Moving straight from one note to another must not leave this client subscribed to, or
        // shown as present in, the note it came from.
        self.leave_room(client);
        let mut library = self.library.lock().map_err(|_| "library poisoned")?;
        if !library.notes.contains_key(id) {
            return Err(format!("no note with id {id}"));
        }
        library.open(id)?;
        let Some(room) = library.rooms.get_mut(id) else {
            return Err("the note could not be opened".to_string());
        };
        room.subscribers.insert(client.id);
        // Send everything the client is missing (the server does not know its state yet, so
        // that is the whole document), then ask for what the server is missing.
        let state_vector = room.doc.transact().state_vector();
        let diff = room
            .doc
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        let step1 = YMessage::Sync(SyncMessage::SyncStep1(state_vector));
        let step2 = YMessage::Sync(SyncMessage::SyncStep2(diff));
        // Awareness is only relayed as it changes, so somebody opening a note has to be told
        // about the people already in it — otherwise their names and carets stay invisible until
        // those people happen to move.
        let present = AwarenessUpdate {
            clients: room
                .awareness
                .iter()
                .map(|(awareness_id, state)| {
                    (
                        ClientID::new(*awareness_id),
                        AwarenessUpdateEntry {
                            clock: state.clock,
                            json: state.json.as_str().into(),
                        },
                    )
                })
                .collect(),
        };
        drop(library);

        // Every sync frame carries the note it belongs to, so a frame still in flight when the
        // client switches notes cannot be taken for the note it just opened.
        self.push(client, FRAME_SYNC, net::encode_sync(id, &step1.encode_v1()));
        self.push(client, FRAME_SYNC, net::encode_sync(id, &step2.encode_v1()));
        // Our own step 1 asks the client for its state; SyncStep2 above already carries ours.
        let server_sv = YMessage::Sync(SyncMessage::SyncStep1(StateVector::default()));
        self.push(
            client,
            FRAME_SYNC,
            net::encode_sync(id, &server_sv.encode_v1()),
        );
        if !present.clients.is_empty() {
            self.push(client, FRAME_AWARENESS, present.encode_v1());
        }
        if let Ok(mut slot) = client.room.lock() {
            *slot = Some(id.to_string());
        }
        self.broadcast_presence();
        Ok(())
    }

    fn leave_room(&self, client: &Arc<Client>) {
        let previous = client.room.lock().ok().and_then(|mut slot| slot.take());
        let Some(id) = previous else {
            return;
        };
        let mut gone: Vec<(u64, u32)> = Vec::new();
        let mut remaining: Vec<u64> = Vec::new();
        let published: Vec<u64> = client
            .awareness
            .lock()
            .map(|mut ids| {
                let published = ids.iter().copied().collect();
                // The ids belonged to the room just left; a new note announces itself with a new
                // state, so keeping the old set would only grow it for the whole session.
                ids.clear();
                published
            })
            .unwrap_or_default();
        if let Ok(mut library) = self.library.lock() {
            if let Some(room) = library.rooms.get_mut(&id) {
                room.subscribers.remove(&client.id);
                for awareness_id in published {
                    if let Some(state) = room.awareness.remove(&awareness_id) {
                        gone.push((awareness_id, state.clock));
                    }
                }
                remaining = room.subscribers.iter().copied().collect();
            }
        }
        // The people still in the room have that state in their own awareness from when it was
        // relayed to them, and only an update removes it again: without this the name and caret
        // of somebody who switched notes or quit would stay on the note for good.
        if !gone.is_empty() {
            let update = AwarenessUpdate {
                clients: gone
                    .into_iter()
                    .map(|(awareness_id, clock)| {
                        (
                            ClientID::new(awareness_id),
                            AwarenessUpdateEntry {
                                clock,
                                json: "null".into(),
                            },
                        )
                    })
                    .collect(),
            };
            let payload = update.encode_v1();
            for subscriber in remaining {
                self.push_to(subscriber, FRAME_AWARENESS, payload.clone());
            }
        }
        self.broadcast_presence();
    }

    fn handle_doc_frame(&self, client: &Arc<Client>, kind: u8, payload: Vec<u8>) {
        let note_id = client.room.lock().ok().and_then(|slot| slot.clone());
        let Some(note_id) = note_id else {
            return;
        };

        match kind {
            FRAME_SYNC => {
                let Some((frame_note, message)) = crate::net::decode_sync(&payload) else {
                    return;
                };
                // A frame for a note this connection no longer has open is dropped: it would
                // otherwise write the note just left into the one now open.
                if frame_note != note_id {
                    return;
                }
                let Ok(message) = YMessage::decode_v1(message) else {
                    return;
                };
                match message {
                    YMessage::Sync(SyncMessage::SyncStep1(sv)) => {
                        if let Ok(library) = self.library.lock() {
                            if let Some(room) = library.rooms.get(&note_id) {
                                let diff = room.doc.transact().encode_state_as_update_v1(&sv);
                                let reply =
                                    YMessage::Sync(SyncMessage::SyncStep2(diff)).encode_v1();
                                drop(library);
                                let tagged = crate::net::encode_sync(&note_id, &reply);
                                self.push(client, FRAME_SYNC, tagged);
                            }
                        }
                    }
                    YMessage::Sync(SyncMessage::SyncStep2(update))
                    | YMessage::Sync(SyncMessage::Update(update)) => {
                        self.apply_update(&note_id, client.id, update);
                    }
                    _ => {}
                }
            }
            FRAME_AWARENESS => {
                let Ok(update) = yrs::sync::AwarenessUpdate::decode_v1(&payload) else {
                    return;
                };
                if let Ok(mut library) = self.library.lock() {
                    if let Some(room) = library.rooms.get_mut(&note_id) {
                        for (awareness_id, entry) in update.clients.iter() {
                            let awareness_id = awareness_id.get();
                            if entry.json.trim() == "null" {
                                room.awareness.remove(&awareness_id);
                                if let Ok(mut mine) = client.awareness.lock() {
                                    mine.remove(&awareness_id);
                                }
                            } else {
                                if let Ok(mut mine) = client.awareness.lock() {
                                    mine.insert(awareness_id);
                                }
                                if room
                                    .awareness
                                    .get(&awareness_id)
                                    .is_none_or(|state| state.clock <= entry.clock)
                                {
                                    room.awareness.insert(
                                        awareness_id,
                                        PeerState {
                                            clock: entry.clock,
                                            json: entry.json.to_string(),
                                        },
                                    );
                                }
                            }
                        }
                    }
                }
                // Only the people in this note see where this person's caret is. Relaying to
                // every connection told somebody editing one note that a person was in theirs,
                // and left a name behind in a note that person had already left.
                let recipients: Vec<u64> = self
                    .library
                    .lock()
                    .ok()
                    .and_then(|library| {
                        library
                            .rooms
                            .get(&note_id)
                            .map(|room| room.subscribers.clone())
                    })
                    .map(|subscribers| {
                        subscribers
                            .into_iter()
                            .filter(|id| *id != client.id)
                            .collect()
                    })
                    .unwrap_or_default();
                for recipient in recipients {
                    self.push_to(recipient, FRAME_AWARENESS, payload.clone());
                }
                self.broadcast_presence();
            }
            _ => {}
        }
    }

    fn apply_update(&self, note_id: &str, from: u64, update: Vec<u8>) {
        if yrs::Update::decode_v1(&update).is_err() {
            return;
        }
        let applied = {
            let Ok(mut library) = self.library.lock() else {
                return;
            };
            let Some(room) = library.rooms.get_mut(note_id) else {
                return;
            };
            let mut txn = room.doc.transact_mut();
            let Ok(decoded) = yrs::Update::decode_v1(&update) else {
                return;
            };
            if txn.apply_update(decoded).is_err() {
                return;
            }
            room.subscribers.clone()
        };
        library_mark_dirty(&self.library, note_id);
        // Relayed as a Yjs sync message, which is what the other clients decode, tagged with the
        // note it belongs to.
        let relayed = YMessage::Sync(SyncMessage::Update(update)).encode_v1();
        let tagged = net::encode_sync(note_id, &relayed);
        let recipients: Vec<u64> = applied.into_iter().filter(|id| *id != from).collect();
        for id in recipients {
            self.push_to(id, FRAME_SYNC, tagged.clone());
        }
    }

    fn push(&self, client: &Arc<Client>, kind: u8, payload: Vec<u8>) {
        if let Ok(mut outbox) = client.outbox.lock() {
            outbox.push((kind, payload));
        }
    }

    fn push_to(&self, client_id: u64, kind: u8, payload: Vec<u8>) {
        let client = self
            .clients
            .lock()
            .ok()
            .and_then(|clients| clients.get(&client_id).cloned());
        if let Some(client) = client {
            self.push(&client, kind, payload);
        }
    }

    fn broadcast_except(&self, skip: u64, kind: u8, payload: Vec<u8>) {
        let recipients: Vec<Arc<Client>> = self
            .clients
            .lock()
            .map(|clients| {
                clients
                    .values()
                    .filter(|c| c.id != skip && c.authed.load(Ordering::Relaxed))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        for client in recipients {
            self.push(&client, kind, payload.clone());
        }
    }

    fn broadcast_library(&self) {
        let notes = match self.library.lock() {
            Ok(library) => library.list(),
            Err(_) => return,
        };
        self.broadcast_except(0, FRAME_JSON, json(&ServerMsg::Notes { notes }));
    }

    fn broadcast_presence(&self) {
        let notes: Vec<(String, Vec<PresenceUser>)> = match self.library.lock() {
            Ok(library) => library.presence(),
            Err(_) => return,
        };
        self.broadcast_except(0, FRAME_JSON, json(&ServerMsg::Presence { notes }));
    }

    fn flush_dirty(&self) {
        let Ok(mut library) = self.library.lock() else {
            return;
        };
        let now = Instant::now();
        let due: Vec<String> = library
            .dirty
            .iter()
            .filter(|(_, at)| now.duration_since(**at) >= WRITE_DEBOUNCE)
            .map(|(id, _)| id.clone())
            .collect();
        let mut wrote = false;
        for id in due {
            match library.flush(&id) {
                Ok(()) => wrote = true,
                Err(error) => eprintln!("could not save note {id}: {error}"),
            }
        }
        drop(library);
        if wrote {
            // The modification time people see in the sidebar moved on.
            self.broadcast_library();
        }
    }

    fn disconnect(&self, client: &Arc<Client>) {
        if let Ok(name) = client.name.lock() {
            if client.authed.load(Ordering::Relaxed) {
                eprintln!("{name} disconnected");
            }
        }
        self.leave_room(client);
        if let Ok(mut clients) = self.clients.lock() {
            clients.remove(&client.id);
        }
        self.broadcast_presence();
    }
}

fn library_mark_dirty(library: &Mutex<Library>, id: &str) {
    if let Ok(mut library) = library.lock() {
        // The body changed, so the note is the most recently edited one in the sidebar.
        if let Some(meta) = library.notes.get_mut(id) {
            meta.updated_at = Utc::now().timestamp();
        }
        library.dirty.insert(id.to_string(), Instant::now());
    }
}

impl Library {
    fn load(dir: &Path) -> Result<Self, String> {
        let mut notes = HashMap::new();
        let entries =
            std::fs::read_dir(dir).map_err(|e| format!("could not read {}: {e}", dir.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Some(id) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
            else {
                continue;
            };
            let Ok(raw) = std::fs::read_to_string(&path) else {
                continue;
            };
            let note = Note::from_markdown(id.clone(), &raw, path.clone());
            notes.insert(id, meta_from_note(&note));
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            notes,
            rooms: HashMap::new(),
            dirty: HashMap::new(),
        })
    }

    fn list(&self) -> Vec<NoteMeta> {
        let mut notes: Vec<NoteMeta> = self.notes.values().cloned().collect();
        notes.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then(b.updated_at.cmp(&a.updated_at))
        });
        notes
    }

    fn create(
        &mut self,
        title: &str,
        folder: Option<String>,
        tags: Vec<String>,
        body: &str,
        pinned: bool,
        icon: Option<String>,
    ) -> Result<NoteMeta, String> {
        let title = title.trim();
        let title = if title.is_empty() { "Untitled" } else { title };
        let now = Utc::now();
        let id = unique_id(&self.dir, title);
        let meta = NoteMeta {
            id: id.clone(),
            title: title.to_string(),
            folder: folder.filter(|f| !f.trim().is_empty()),
            tags: tags
                .into_iter()
                .filter_map(|t| {
                    let tag = crate::storage::normalize_tag(&t);
                    if tag.is_empty() {
                        None
                    } else {
                        Some(tag)
                    }
                })
                .collect(),
            pinned,
            trashed: false,
            icon: icon.filter(|i| !i.trim().is_empty()),
            updated_at: now.timestamp(),
        };
        // Written straight to disk: there is no room for this note yet, and the first client to
        // open it starts from this body.
        let note = note_from_meta(&meta, body.to_string());
        let path = self.dir.join(format!("{id}.md"));
        std::fs::write(&path, note.to_markdown())
            .map_err(|e| format!("could not write {}: {e}", path.display()))?;
        self.notes.insert(id, meta.clone());
        Ok(meta)
    }

    #[allow(clippy::too_many_arguments)]
    fn update(
        &mut self,
        id: &str,
        title: Option<String>,
        folder: Field<String>,
        tags: Option<Vec<String>>,
        pinned: Option<bool>,
        trashed: Option<bool>,
        icon: Field<String>,
    ) -> Result<(), String> {
        let Some(meta) = self.notes.get_mut(id) else {
            return Err(format!("no note with id {id}"));
        };
        if let Some(title) = title {
            let title = title.trim();
            if !title.is_empty() {
                meta.title = title.to_string();
            }
        }
        match folder {
            Field::Keep => {}
            Field::Clear => meta.folder = None,
            Field::Set(folder) => meta.folder = Some(folder).filter(|f| !f.trim().is_empty()),
        }
        if let Some(tags) = tags {
            meta.tags = tags
                .into_iter()
                .map(|t| crate::storage::normalize_tag(&t))
                .filter(|t| !t.is_empty())
                .collect();
        }
        if let Some(pinned) = pinned {
            meta.pinned = pinned;
        }
        if let Some(trashed) = trashed {
            meta.trashed = trashed;
        }
        match icon {
            Field::Keep => {}
            Field::Clear => meta.icon = None,
            Field::Set(icon) => meta.icon = Some(icon).filter(|i| !i.trim().is_empty()),
        }
        meta.updated_at = Utc::now().timestamp();
        self.write(id)
    }

    fn delete(&mut self, id: &str, permanent: bool) -> Result<(), String> {
        if permanent {
            self.notes.remove(id);
            self.rooms.remove(id);
            self.dirty.remove(id);
            let path = self.dir.join(format!("{id}.md"));
            let _ = std::fs::remove_file(path);
            Ok(())
        } else {
            self.update(id, None, Field::Keep, None, None, Some(true), Field::Keep)
        }
    }

    /// Opens the Yjs document for a note, loading the body from disk the first time.
    fn open(&mut self, id: &str) -> Result<(), String> {
        if self.rooms.contains_key(id) {
            return Ok(());
        }
        let path = self.dir.join(format!("{id}.md"));
        let raw = std::fs::read_to_string(&path).unwrap_or_default();
        let note = Note::from_markdown(id.to_string(), &raw, path);
        let body = note.body.clone();
        let doc = Doc::new();
        let text = doc.get_or_insert_text("body");
        {
            let mut txn = doc.transact_mut();
            if !body.is_empty() {
                text.insert(&mut txn, 0, &body);
            }
        }
        self.rooms.insert(
            id.to_string(),
            Room {
                doc,
                text,
                awareness: HashMap::new(),
                subscribers: HashSet::new(),
            },
        );
        Ok(())
    }

    fn body(&self, id: &str) -> String {
        match self.rooms.get(id) {
            Some(room) => {
                let txn = room.doc.transact();
                room.text.get_string(&txn)
            }
            None => {
                let path = self.dir.join(format!("{id}.md"));
                std::fs::read_to_string(path)
                    .ok()
                    .map(|raw| Note::from_markdown(id.to_string(), &raw, PathBuf::new()).body)
                    .unwrap_or_default()
            }
        }
    }

    fn write(&mut self, id: &str) -> Result<(), String> {
        let Some(meta) = self.notes.get(id).cloned() else {
            return Ok(());
        };
        let body = self.body(id);
        let path = self.dir.join(format!("{id}.md"));
        // Keep the original creation time when the note already exists on disk.
        let created_at = std::fs::read_to_string(&path)
            .ok()
            .map(|raw| Note::from_markdown(id.to_string(), &raw, path.clone()).created_at)
            .unwrap_or_else(Utc::now);
        let mut note = note_from_meta(&meta, body);
        note.created_at = created_at;
        std::fs::write(&path, note.to_markdown())
            .map_err(|e| format!("could not write {}: {e}", path.display()))?;
        self.dirty.remove(id);
        Ok(())
    }

    fn flush(&mut self, id: &str) -> Result<(), String> {
        self.write(id)
    }

    fn presence(&self) -> Vec<(String, Vec<PresenceUser>)> {
        let mut out = Vec::new();
        for (id, room) in &self.rooms {
            if room.subscribers.is_empty() {
                continue;
            }
            let mut users: Vec<PresenceUser> = Vec::new();
            for state in room.awareness.values() {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&state.json) else {
                    continue;
                };
                let name = value
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("someone")
                    .to_string();
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
                    .unwrap_or([140, 170, 150]);
                if !users.iter().any(|u| u.name == name) {
                    users.push(PresenceUser { name, color });
                }
            }
            users.sort_by(|a, b| a.name.cmp(&b.name));
            out.push((id.clone(), users));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

fn meta_from_note(note: &Note) -> NoteMeta {
    NoteMeta {
        id: note.id.clone(),
        title: note.title.clone(),
        folder: note.folder.clone(),
        tags: note.explicit_tags.clone(),
        pinned: note.pinned,
        trashed: note.trashed,
        icon: note.icon.clone(),
        updated_at: note.updated_at.timestamp(),
    }
}

fn note_from_meta(meta: &NoteMeta, body: String) -> Note {
    let updated = Utc
        .timestamp_opt(meta.updated_at, 0)
        .single()
        .unwrap_or_else(Utc::now);
    Note {
        id: meta.id.clone(),
        title: meta.title.clone(),
        body,
        pinned: meta.pinned,
        trashed: meta.trashed,
        explicit_tags: meta.tags.clone(),
        icon: meta.icon.clone(),
        folder: meta.folder.clone(),
        created_at: updated,
        updated_at: updated,
        file_path: PathBuf::new(),
    }
}

/// `title-slug-a1b2c3`, unique inside the folder.
fn unique_id(dir: &Path, title: &str) -> String {
    let slug: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug.trim_matches('-').to_string();
    let slug = if slug.is_empty() {
        "note".to_string()
    } else {
        slug
    };
    let slug: String = slug.chars().take(40).collect();
    for attempt in 0.. {
        let suffix = if attempt == 0 {
            format!("{:04x}", rand::random::<u16>())
        } else {
            format!("{:04x}{attempt}", rand::random::<u16>())
        };
        let id = format!("{slug}-{suffix}");
        if !dir.join(format!("{id}.md")).exists() {
            return id;
        }
    }
    unreachable!()
}

fn json(message: &ServerMsg) -> Vec<u8> {
    message
        .to_text()
        .unwrap_or_else(|_| "{\"type\":\"error\",\"message\":\"encoding failed\"}".to_string())
        .into_bytes()
}

/// Compares without leaking where the first difference is.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("elora-server-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_library_creates_and_persists_markdown() {
        let dir = temp_dir("library");
        let mut library = Library::load(&dir).unwrap();
        let meta = library
            .create("Meeting notes", None, Vec::new(), "", false, None)
            .unwrap();
        assert!(meta.id.starts_with("meeting-notes-"));
        let path = dir.join(format!("{}.md", meta.id));
        assert!(path.is_file());
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("title: \"Meeting notes\""));
        assert!(raw.contains("pinned: false"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_library_creates_with_folder_tags_and_body() {
        let dir = temp_dir("create");
        let mut library = Library::load(&dir).unwrap();
        let meta = library
            .create(
                "Plan",
                Some("Team".to_string()),
                vec!["#plan".to_string(), "  ".to_string()],
                "written by the client",
                false,
                Some("book".to_string()),
            )
            .unwrap();
        assert_eq!(meta.folder.as_deref(), Some("Team"));
        assert_eq!(meta.tags, vec!["plan".to_string()]);
        assert_eq!(meta.icon.as_deref(), Some("book"));
        let raw = std::fs::read_to_string(dir.join(format!("{}.md", meta.id))).unwrap();
        assert!(raw.contains("folder: \"Team\""), "frontmatter: {raw:?}");
        assert!(raw.contains("tags: [plan]"), "frontmatter: {raw:?}");
        assert!(raw.contains("icon: book"), "frontmatter: {raw:?}");
        assert!(raw.contains("written by the client"), "body: {raw:?}");
        // A note moved into the shared library keeps its icon across a restart too.
        let reloaded = Library::load(&dir).unwrap();
        assert_eq!(reloaded.notes[&meta.id].icon.as_deref(), Some("book"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_document_round_trips_through_the_server() {
        let dir = temp_dir("doc");
        let mut library = Library::load(&dir).unwrap();
        let meta = library
            .create("Shared", None, Vec::new(), "", false, None)
            .unwrap();
        library.open(&meta.id).unwrap();
        {
            let room = library.rooms.get_mut(&meta.id).unwrap();
            let mut txn = room.doc.transact_mut();
            room.text
                .insert(&mut txn, 0, "# Shared\n\nhello from one client");
        }
        library.dirty.insert(meta.id.clone(), Instant::now());
        library.flush(&meta.id).unwrap();
        let raw = std::fs::read_to_string(dir.join(format!("{}.md", meta.id))).unwrap();
        assert!(raw.contains("hello from one client"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_metadata_update_survives_reload() {
        let dir = temp_dir("meta");
        let mut library = Library::load(&dir).unwrap();
        let meta = library
            .create("Tags", None, Vec::new(), "", false, None)
            .unwrap();
        library
            .update(
                &meta.id,
                Some("Tags!".into()),
                Field::Set("Study".into()),
                Some(vec!["#alpha".into(), "Beta".into()]),
                Some(true),
                None,
                Field::Keep,
            )
            .unwrap();
        let reloaded = Library::load(&dir).unwrap();
        let note = reloaded.notes.get(&meta.id).unwrap();
        assert_eq!(note.title, "Tags!");
        assert_eq!(note.folder.as_deref(), Some("Study"));
        assert_eq!(note.tags, vec!["alpha".to_string(), "beta".to_string()]);
        assert!(note.pinned);

        // Taking the folder off again has to stick too. A cleared folder used to reach the
        // server as `null`, which it read as "leave it alone", so the note stayed in the
        // folder for good.
        library
            .update(&meta.id, None, Field::Clear, None, None, None, Field::Clear)
            .unwrap();
        let reloaded = Library::load(&dir).unwrap();
        let note = reloaded.notes.get(&meta.id).unwrap();
        assert_eq!(note.folder, None, "the folder should have been cleared");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_password_compare_is_exact() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreT"));
        assert!(!constant_time_eq(b"secret", b"secret "));
    }

    /// A client that speaks an older protocol is told why, instead of being dropped like a
    /// broken connection: the two sides of a shared library can be updated days apart.
    #[test]
    fn test_older_client_is_told_which_side_to_update() {
        let _guard = crate::tls::server_dir_lock();
        let dir = temp_dir("older-client");
        let notes = dir.join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        // SAFETY: the guard keeps the other tests that read this variable out of the way.
        unsafe { std::env::set_var("ELORA_SERVER_DIR", dir.join("server")) };

        let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        let config = ServerConfig {
            name: "Version Check".into(),
            port,
            password: "letmein".into(),
            notes_dir: notes,
            bind: "127.0.0.1".into(),
        };
        let server = Arc::new(Server::new(config).expect("server"));
        std::thread::spawn(move || {
            let _ = server.run();
        });

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline
            && std::net::TcpStream::connect(("127.0.0.1", port)).is_err()
        {
            std::thread::sleep(Duration::from_millis(20));
        }

        let (config, _seen) = crate::tls::client_config(None);
        let socket = std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect");
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let name = rustls::pki_types::ServerName::try_from("127.0.0.1").unwrap();
        let connection = rustls::ClientConnection::new(config, name).expect("client tls");
        let mut stream = rustls::StreamOwned::new(connection, socket);

        let hello = ClientMsg::Hello {
            v: PROTOCOL_VERSION - 1,
            name: "older app".into(),
            password: "letmein".into(),
        };
        let payload = serde_json::to_vec(&hello).unwrap();
        net::write_frame(&mut stream, FRAME_JSON, &payload).expect("send hello");
        stream.flush().expect("flush");

        let mut buffer = Vec::new();
        let mut chunk = [0u8; 4096];
        let reply = loop {
            let n = stream
                .read(&mut chunk)
                .expect("a reply, not a closed socket");
            assert!(n > 0, "the server closed without explaining why");
            buffer.extend_from_slice(&chunk[..n]);
            if let Some((kind, payload)) = net::take_frames(&mut buffer).first() {
                assert_eq!(*kind, FRAME_JSON);
                break serde_json::from_slice::<ServerMsg>(payload).expect("a readable reply");
            }
        };
        match reply {
            ServerMsg::Error { message } => {
                assert!(
                    message.contains(&format!("speaks version {PROTOCOL_VERSION}")),
                    "the reply should name the server's version: {message}"
                );
                assert!(
                    message.contains("Update whichever side is older"),
                    "the reply should say what to do: {message}"
                );
            }
            other => panic!("expected an error, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
