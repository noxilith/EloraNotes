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
    self, ClientMsg, Field, NoteMeta, PresenceUser, ServerConfig, ServerMsg, ATTACHMENT_ASK,
    ATTACHMENT_DATA, ATTACHMENT_MISSING, ATTACHMENT_SEND, FRAME_ATTACHMENT, FRAME_AWARENESS,
    FRAME_JSON, FRAME_SYNC, MAX_ATTACHMENT, PROTOCOL_VERSION,
};
use crate::people::{self, Registry};
use crate::storage::Note;
use chrono::{DateTime, TimeZone, Utc};
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
/// How often one connection looks for notes that are due to be written. Every connection doing
/// it on its own loop meant the library lock was taken a hundred times a second per person.
const FLUSH_TICK: Duration = Duration::from_millis(100);
/// How long a silent connection is kept before it is dropped.
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

pub struct Server {
    config: ServerConfig,
    tls: Arc<rustls::ServerConfig>,
    library: Mutex<Library>,
    /// Which display name belongs to which secret key. The keys themselves are never kept.
    people: Mutex<Registry>,
    clients: Mutex<HashMap<u64, Arc<Client>>>,
    /// The presence list last handed out, and the connections that already have it. Who is in a
    /// note changes when somebody opens or leaves one; the awareness frames that arrive for every
    /// caret move carry no such news, so they must not turn into a full list per person.
    presence: Mutex<PresenceBroadcast>,
    next_id: AtomicU64,
}

/// Everything the connection thread needs to hand work to other connections.
struct Client {
    id: u64,
    name: Mutex<String>,
    /// This connection's public key: the digest of the identifier it sent at the handshake. It
    /// is what every note is decided by — the name beside it is only a label — and it is safe
    /// for anybody to read, since the identifier behind it is what grants access.
    key: Mutex<String>,
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
    /// When each note was made. A save must not change it, and finding it back out by reading the
    /// note's own file cost a read and a parse of the whole note on every save — while the
    /// library lock was held, which is time every other connection spent waiting to type.
    created: HashMap<String, DateTime<Utc>>,
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

/// The presence list as the connections have it: the snapshot, and who was already sent it.
#[derive(Default)]
struct PresenceBroadcast {
    list: Vec<(String, Vec<PresenceUser>)>,
    sent: HashSet<u64>,
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
        let people = Registry::load(ServerConfig::data_dir().join("people.json"))?;
        Ok(Self {
            config,
            tls,
            library: Mutex::new(library),
            people: Mutex::new(people),
            clients: Mutex::new(HashMap::new()),
            presence: Mutex::new(PresenceBroadcast::default()),
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
            key: Mutex::new(String::new()),
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
        let mut last_flush = Instant::now();

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
                } else if kind == FRAME_ATTACHMENT {
                    self.handle_attachment(client, &payload);
                } else {
                    self.handle_doc_frame(client, kind, payload);
                }
            }

            if last_flush.elapsed() >= FLUSH_TICK {
                self.flush_dirty();
                last_flush = Instant::now();
            }

            if last_contact.elapsed() > IDLE_TIMEOUT {
                return Ok(());
            }
            // The read timeout above is what paces this loop, so a finished frame goes to the
            // room at once instead of waiting out an extra sleep.
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
        let ClientMsg::Hello {
            v,
            name,
            id,
            password,
        } = message
        else {
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
        // The key is what private notes are decided by, and it is a secret: it is turned into a
        // digest at once so the server never holds the secret itself. The name is what others
        // see, and it has to be free — a name already bound to another key is refused here, or
        // somebody could join as a person a note was shared with.
        let identity = crate::net::normalize_identifier(&id);
        if identity.is_empty() {
            self.push(
                client,
                FRAME_JSON,
                json(&ServerMsg::Error {
                    message: "this app did not send its identifier; update it and try again"
                        .to_string(),
                }),
            );
            return Ok(false);
        }
        if identity.chars().count() < people::MIN_IDENTIFIER {
            self.push(
                client,
                FRAME_JSON,
                json(&ServerMsg::Error {
                    message: "this app's identifier is too short to keep anybody out; press \
                              New in Settings, or paste the one from your other computer"
                        .to_string(),
                }),
            );
            return Ok(false);
        }
        let key = people::public_key(&identity);
        {
            let mut people = self
                .people
                .lock()
                .map_err(|_| "the people list is poisoned")?;
            if let Err(message) = people.claim(&name, &key) {
                drop(people);
                self.push(client, FRAME_JSON, json(&ServerMsg::Error { message }));
                return Ok(false);
            }
        };
        let name = crate::net::normalize_identifier(&name);
        if let Ok(mut slot) = client.name.lock() {
            *slot = name.clone();
        }
        if let Ok(mut slot) = client.key.lock() {
            *slot = key;
        }
        client.authed.store(true, Ordering::Relaxed);
        eprintln!("{name} connected from the network");

        let notes = self
            .library
            .lock()
            .map_err(|_| "library poisoned")?
            .list_for(&self.key_of(client));
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
        // Access is decided by the key this connection proved; a note it takes ownership of is
        // written with that key and with the name it is going by right now, which is only the
        // label the sidebar prints.
        let key = self.key_of(client);
        let who = self.name_of(client);
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
                    library.create(&title, folder, tags, &body, pinned, icon, &who, &key)?
                };
                self.broadcast_library();
                self.push(client, FRAME_JSON, json(&ServerMsg::Created { note: meta }));
                Ok(())
            }
            ClientMsg::SetShare {
                id,
                private,
                shared_with,
            } => {
                {
                    let mut library = self.library.lock().map_err(|_| "library poisoned")?;
                    let people = self.resolve_people(&shared_with)?;
                    library.set_share(&id, &key, private, people)?;
                }
                // Closing a note while somebody is reading it has to take them out of it:
                // otherwise they keep receiving what is typed into it afterwards.
                self.revoke_members(&id);
                self.broadcast_library();
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
                    library.update(&id, &key, title, folder, tags, pinned, trashed, icon)?;
                }
                self.broadcast_library();
                Ok(())
            }
            ClientMsg::Delete { id, permanent } => {
                {
                    let mut library = self.library.lock().map_err(|_| "library poisoned")?;
                    library.delete(&id, &key, permanent)?;
                }
                self.broadcast_library();
                Ok(())
            }
        }
    }

    /// Takes everybody the note is no longer shared with out of its room, and leaves everybody
    /// the note is still for where they are.
    fn revoke_members(&self, id: &str) {
        let meta = self
            .library
            .lock()
            .ok()
            .and_then(|library| library.notes.get(id).cloned());
        let Some(meta) = meta else {
            return;
        };
        for client in self.connected() {
            let open = client.room.lock().ok().and_then(|slot| slot.clone());
            if open.as_deref() == Some(id) && !net::visible_to(&meta, &self.key_of(&client)) {
                self.leave_room(&client);
            }
        }
    }

    /// The public key a connection proved at the handshake, which is the only place it is set.
    /// Every note is decided by this, never by the name beside it.
    fn key_of(&self, client: &Arc<Client>) -> String {
        client
            .key
            .lock()
            .map(|slot| slot.clone())
            .unwrap_or_default()
    }

    /// Turns the names somebody typed into (name, public key) pairs, using the server's own list.
    /// A name nobody has claimed is refused rather than stored: a note that names a stranger
    /// reaches nobody, and whoever typed it should hear about it rather than wonder.
    fn resolve_people(&self, names: &[String]) -> Result<Vec<(String, String)>, String> {
        let people = self
            .people
            .lock()
            .map_err(|_| "the people list is poisoned")?;
        let mut resolved: Vec<(String, String)> = Vec::new();
        for raw in names {
            let name = net::normalize_identifier(raw);
            if name.is_empty() {
                continue;
            }
            let Some(key) = people.key_of(&name) else {
                return Err(format!(
                    "nobody on this server is called “{name}” yet. They have to connect once, \
                     then add them to the note again."
                ));
            };
            if resolved
                .iter()
                .any(|(kept, _)| kept.eq_ignore_ascii_case(&name))
            {
                continue;
            }
            resolved.push((name, key.to_string()));
        }
        Ok(resolved)
    }

    /// The name this connection is currently going by, for a note it takes ownership of.
    fn name_of(&self, client: &Arc<Client>) -> String {
        client
            .name
            .lock()
            .map(|slot| slot.clone())
            .unwrap_or_default()
    }

    fn open_room(&self, client: &Arc<Client>, id: &str) -> Result<(), String> {
        // Moving straight from one note to another must not leave this client subscribed to, or
        // shown as present in, the note it came from.
        self.leave_room(client);
        let key = self.key_of(client);
        let mut library = self.library.lock().map_err(|_| "library poisoned")?;
        let Some(meta) = library.notes.get(id) else {
            return Err(format!("no note with id {id}"));
        };
        // A note restricted to its owner is not opened for anybody the key behind the name is
        // not the one the note is for.
        if !crate::net::visible_to(meta, &key) {
            return Err(format!("the note {id} is not shared with you"));
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

    /// Pictures named by shared notes. A client sends one when it adds a picture to a note the
    /// server owns, and asks for one it has not seen before. The bytes live in the library
    /// folder beside the notes, so anybody with the library can see the picture and it stays
    /// there after the computer that added it goes away.
    fn handle_attachment(&self, client: &Arc<Client>, payload: &[u8]) {
        let Some((op, name, bytes)) = net::decode_attachment(payload) else {
            return;
        };
        if !net::valid_attachment_name(name) {
            self.push(
                client,
                FRAME_ATTACHMENT,
                net::encode_attachment(ATTACHMENT_MISSING, name, b""),
            );
            return;
        }

        match op {
            ATTACHMENT_SEND => {
                if bytes.is_empty() || bytes.len() > MAX_ATTACHMENT {
                    self.push(
                        client,
                        FRAME_ATTACHMENT,
                        net::encode_attachment(ATTACHMENT_MISSING, name, b""),
                    );
                    return;
                }
                let stored = self
                    .library
                    .lock()
                    .map_err(|_| "library poisoned".to_string())
                    .and_then(|library| library.write_attachment(name, bytes));
                match stored {
                    // An empty answer with the name is the acknowledgement: the picture is in
                    // the library now, so this client can tell the user it was shared.
                    Ok(()) => {
                        self.push(
                            client,
                            FRAME_ATTACHMENT,
                            net::encode_attachment(ATTACHMENT_DATA, name, b""),
                        );
                        // Everybody else who has the note open can now see the picture, and a
                        // client that asked for it a moment too early asks again.
                        self.broadcast_except(
                            0,
                            FRAME_JSON,
                            json(&ServerMsg::Attachment {
                                name: name.to_string(),
                            }),
                        );
                    }
                    Err(error) => {
                        eprintln!("could not store the picture {name}: {error}");
                        self.push(
                            client,
                            FRAME_ATTACHMENT,
                            net::encode_attachment(ATTACHMENT_MISSING, name, b""),
                        );
                    }
                }
            }
            ATTACHMENT_ASK => {
                let bytes = self
                    .library
                    .lock()
                    .ok()
                    .and_then(|library| library.read_attachment(name));
                let (op, bytes) = match bytes {
                    Some(bytes) => (ATTACHMENT_DATA, bytes),
                    None => (ATTACHMENT_MISSING, Vec::new()),
                };
                self.push(
                    client,
                    FRAME_ATTACHMENT,
                    net::encode_attachment(op, name, &bytes),
                );
            }
            _ => {}
        }
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
                // Only the people in this note see where this person's caret is. Relaying to
                // every connection told somebody editing one note that a person was in theirs,
                // and left a name behind in a note that person had already left. The state and
                // the list of who to send it to come from the same look at the library: two
                // locks per caret made every keystroke contend with every other connection.
                let recipients: Vec<u64> = {
                    let Ok(mut library) = self.library.lock() else {
                        return;
                    };
                    let Some(room) = library.rooms.get_mut(&note_id) else {
                        return;
                    };
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
                    room.subscribers
                        .iter()
                        .copied()
                        .filter(|id| *id != client.id)
                        .collect()
                };
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
            let Ok(decoded) = yrs::Update::decode_v1(&update) else {
                return;
            };
            // A removal adds no blocks, so an update made only of removals leaves the state
            // vector exactly where it was. Judging the update by that alone dropped every
            // backspace: the word came off the typist's screen and stayed on the note for
            // everybody else. For those the text is what tells a real removal from a stale one
            // that removes what somebody else already did.
            let removals_only =
                decoded.state_vector().is_empty() && !decoded.delete_set().is_empty();
            let before_text = removals_only.then(|| {
                let txn = room.doc.transact();
                room.text.get_string(&txn)
            });
            // A client answering our own state request sends its document even when it is
            // empty, and opening a note is not an edit: without this, merely clicking a shared
            // note moved it to the top of the list for everyone. Only an update that changes
            // something counts.
            let before = room.doc.transact().state_vector().encode_v1();
            {
                let mut txn = room.doc.transact_mut();
                if txn.apply_update(decoded).is_err() {
                    return;
                }
            }
            if room.doc.transact().state_vector().encode_v1() == before {
                let changed_nothing = before_text.is_none_or(|was| {
                    let txn = room.doc.transact();
                    room.text.get_string(&txn) == was
                });
                if changed_nothing {
                    return;
                }
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

    /// Every connected client, snapshot so no lock is held while they are written to.
    fn connected(&self) -> Vec<Arc<Client>> {
        self.clients
            .lock()
            .map(|clients| {
                clients
                    .values()
                    .filter(|client| client.authed.load(Ordering::Relaxed))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Sends each connection the library as that connection may see it. One list for everybody
    /// would hand a client the notes that were restricted away from it.
    fn broadcast_library(&self) {
        let clients = self.connected();
        // Working out who may see what needs the library; encoding each list for the wire does
        // not, and doing that under the lock held up every other connection's keystrokes for as
        // long as the whole library took to serialize.
        let lists: Vec<(Arc<Client>, Vec<NoteMeta>)> = {
            let Ok(library) = self.library.lock() else {
                return;
            };
            clients
                .into_iter()
                .map(|client| {
                    let key = self.key_of(&client);
                    let notes = library.list_for(&key);
                    (client, notes)
                })
                .collect()
        };
        for (client, notes) in lists {
            self.push(&client, FRAME_JSON, json(&ServerMsg::Notes { notes }));
        }
    }

    fn broadcast_presence(&self) {
        let clients = self.connected();
        let Ok(library) = self.library.lock() else {
            return;
        };
        let present = library.presence();
        let Ok(mut last) = self.presence.lock() else {
            return;
        };
        if last.list != present {
            last.list = present.clone();
            last.sent.clear();
        }
        for client in clients {
            // Already handed this exact list: a caret moving in one note does not tell anybody
            // else in the server something they can see.
            if !last.sent.insert(client.id) {
                continue;
            }
            let key = self.key_of(&client);
            let notes: Vec<(String, Vec<PresenceUser>)> = present
                .iter()
                .filter(|(id, _)| {
                    library
                        .notes
                        .get(id)
                        .is_some_and(|meta| net::visible_to(meta, &key))
                })
                .cloned()
                .collect();
            self.push(&client, FRAME_JSON, json(&ServerMsg::Presence { notes }));
        }
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
        let mut created = HashMap::new();
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
            notes.insert(id.clone(), meta_from_note(&note));
            created.insert(id, note.created_at);
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            notes,
            rooms: HashMap::new(),
            dirty: HashMap::new(),
            created,
        })
    }

    /// The notes one person may see: everything the library shares with the whole server, plus
    /// the notes they own and the ones somebody opened to them. `key` is their public key.
    fn list_for(&self, key: &str) -> Vec<NoteMeta> {
        self.list()
            .into_iter()
            .filter(|meta| net::visible_to(meta, key))
            .collect()
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

    #[allow(clippy::too_many_arguments)]
    fn create(
        &mut self,
        title: &str,
        folder: Option<String>,
        tags: Vec<String>,
        body: &str,
        pinned: bool,
        icon: Option<String>,
        owner: &str,
        owner_key: &str,
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
            // Recorded even though the note starts out open to the whole server: the key is
            // who may close the note later, and the name is what the sidebar prints as its
            // owner. The key is public on purpose — only the identifier behind it opens
            // anything.
            owner: Some(net::normalize_identifier(owner)).filter(|name| !name.is_empty()),
            owner_key: Some(owner_key.to_string()).filter(|key| !key.is_empty()),
            private: false,
            shared_with: Vec::new(),
            shared_keys: Vec::new(),
        };
        // Written straight to disk: there is no room for this note yet, and the first client to
        // open it starts from this body.
        let note = note_from_meta(&meta, body.to_string());
        let path = self.dir.join(format!("{id}.md"));
        std::fs::write(&path, note.to_markdown())
            .map_err(|e| format!("could not write {}: {e}", path.display()))?;
        self.created.insert(id.clone(), now);
        self.notes.insert(id, meta.clone());
        Ok(meta)
    }

    #[allow(clippy::too_many_arguments)]
    fn update(
        &mut self,
        id: &str,
        key: &str,
        title: Option<String>,
        folder: Field<String>,
        tags: Option<Vec<String>>,
        pinned: Option<bool>,
        trashed: Option<bool>,
        icon: Field<String>,
    ) -> Result<(), String> {
        let Some(meta) = self.notes.get(id) else {
            return Err(format!("no note with id {id}"));
        };
        if !net::visible_to(meta, key) {
            return Err(format!("the note {id} is not shared with you"));
        }
        let meta = self.notes.get_mut(id).expect("the note was just found");
        if let Some(title) = title {
            // The title is kept exactly as it was typed. Trimming it here used to eat the space
            // at the end of a half-typed word: the trimmed title came back to the typist, the
            // space vanished, and the next letter joined the previous word. Only a title with
            // nothing in it is refused, so a note never loses its name.
            if !title.trim().is_empty() {
                meta.title = title;
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

    fn delete(&mut self, id: &str, key: &str, permanent: bool) -> Result<(), String> {
        let Some(meta) = self.notes.get(id) else {
            return Err(format!("no note with id {id}"));
        };
        if !net::visible_to(meta, key) {
            return Err(format!("the note {id} is not shared with you"));
        }
        if permanent {
            self.notes.remove(id);
            self.rooms.remove(id);
            self.dirty.remove(id);
            self.created.remove(id);
            let path = self.dir.join(format!("{id}.md"));
            let _ = std::fs::remove_file(path);
            Ok(())
        } else {
            self.update(
                id,
                key,
                None,
                Field::Keep,
                None,
                None,
                Some(true),
                Field::Keep,
            )
        }
    }

    /// Closes a note to everybody but its owner, opens it to named people, or opens it to the
    /// whole server again. Only the owner decides: anybody else in the note is a guest.
    fn set_share(
        &mut self,
        id: &str,
        key: &str,
        private: bool,
        people: Vec<(String, String)>,
    ) -> Result<(), String> {
        let Some(meta) = self.notes.get_mut(id) else {
            return Err(format!("no note with id {id}"));
        };
        let owner = meta.owner.clone().unwrap_or_default();
        // The owner is the key the note carries, so changing a display name cannot take a note
        // over — and taking a name on the server does not either, because the note does not
        // care what anybody is called.
        match meta.owner_key.as_deref() {
            Some(owner_key) if !owner_key.eq_ignore_ascii_case(key) => {
                return Err(format!("only {owner} can change who sees this note"));
            }
            // Locked before notes carried keys: nobody can prove they are the owner, so the
            // server's own operator gives it one with `elora-server people adopt`.
            None => {
                return Err(format!(
                    "“{}” was locked before notes carried keys. On the server, run \
                     `elora-server people adopt --write` to give it an owner.",
                    if owner.is_empty() {
                        "this note"
                    } else {
                        &owner
                    }
                ));
            }
            _ => {}
        }
        if !net::visible_to(meta, key) {
            return Err(format!("the note {id} is not shared with you"));
        }
        let mut names: Vec<String> = Vec::new();
        let mut keys: Vec<String> = Vec::new();
        if private {
            for (name, person) in people {
                if name.eq_ignore_ascii_case(&owner)
                    || keys.iter().any(|kept| kept == &person)
                    || names.iter().any(|kept| kept.eq_ignore_ascii_case(&name))
                {
                    continue;
                }
                names.push(name);
                keys.push(person);
            }
        }
        // Nobody can see a note through a list that is ignored, so an open note keeps no list at
        // all: reopening it clears the names as well as the lock.
        meta.private = private;
        meta.shared_with = names;
        meta.shared_keys = keys;
        meta.updated_at = Utc::now().timestamp();
        self.write(id)
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

    /// Pictures the notes name with `![alt](attachments/…)`. They sit in the library folder
    /// beside the notes, so the server's folder stays an ordinary EloraNotes library that can be
    /// copied or backed up whole, and a picture outlives whoever added it.
    fn attachments_dir(&self) -> PathBuf {
        self.dir.join("attachments")
    }

    fn attachment_path(&self, name: &str) -> Option<PathBuf> {
        net::valid_attachment_name(name).then(|| self.attachments_dir().join(name))
    }

    fn write_attachment(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        let Some(path) = self.attachment_path(name) else {
            return Err(format!("{name} is not a picture name"));
        };
        std::fs::create_dir_all(self.attachments_dir())
            .map_err(|e| format!("could not create the pictures folder: {e}"))?;
        std::fs::write(&path, bytes).map_err(|e| format!("could not save {name}: {e}"))
    }

    fn read_attachment(&self, name: &str) -> Option<Vec<u8>> {
        let path = self.attachment_path(name)?;
        let size = std::fs::metadata(&path).ok()?.len() as usize;
        if size == 0 || size > MAX_ATTACHMENT {
            return None;
        }
        std::fs::read(path).ok()
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
        // Keep the original creation time: a save is an edit of the note, not a new one.
        let created_at = *self.created.entry(id.to_string()).or_insert_with(Utc::now);
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
        owner: note.owner.clone(),
        owner_key: note.owner_key.clone(),
        private: note.private,
        shared_with: note.shared_with.clone(),
        shared_keys: note.shared_keys.clone(),
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
        owner: meta.owner.clone(),
        owner_key: meta.owner_key.clone(),
        private: meta.private,
        shared_with: meta.shared_with.clone(),
        shared_keys: meta.shared_keys.clone(),
        created_at: updated,
        updated_at: updated,
        file_path: PathBuf::new(),
    }
}

/// One note `adopt_note_keys` looked at: what it changed, or why it could not.
#[derive(Debug, Clone, PartialEq)]
pub struct AdoptedNote {
    pub title: String,
    pub id: String,
    pub changes: Vec<String>,
    /// True when the note still names somebody the server has never seen, so it cannot be
    /// handed to a key yet.
    pub waiting: bool,
}

/// Gives the notes that were locked before notes carried keys their owner's and guests' keys,
/// taken from the server's own name list.
///
/// This is the one step that cannot be automatic. A note written before keys existed says only
/// which *name* owned it, and a name is not proof of anything — that was the whole problem. The
/// person running the server checks `people` first, fixes any name that was claimed by the wrong
/// key with `people release`, and only then runs this. It reports every note it touches, and
/// writes nothing unless `write` is set, so it can be looked over first.
pub fn adopt_note_keys(
    notes_dir: &Path,
    people: &Registry,
    write: bool,
) -> Result<Vec<AdoptedNote>, String> {
    let mut library = Library::load(notes_dir)?;
    let mut report = Vec::new();
    let ids: Vec<String> = library.notes.keys().cloned().collect();
    for id in ids {
        let Some(meta) = library.notes.get(&id).cloned() else {
            continue;
        };
        if !meta.private {
            continue;
        }
        let mut changes = Vec::new();
        let mut waiting = false;

        // The owner: their name is the only clue the note carries, so the server's own list is
        // what turns it into a key.
        let mut owner_key = meta.owner_key.clone();
        match meta.owner.as_deref().map(|name| people.key_of(name)) {
            Some(Some(key)) => {
                if owner_key.as_deref() != Some(key) {
                    changes.push(format!(
                        "owner “{}” → {}",
                        meta.owner.clone().unwrap_or_default(),
                        short_key(key)
                    ));
                    owner_key = Some(key.to_string());
                }
            }
            Some(None) => {
                waiting = true;
                changes.push(format!(
                    "its owner “{}” has not connected yet",
                    meta.owner.clone().unwrap_or_default()
                ));
            }
            None => {
                waiting = true;
                changes.push("it has no owner name".to_string());
            }
        }

        // The guests: a name the server knows keeps its place in the list, and a name it has
        // never seen is left out and reported rather than guessed at.
        let mut names = Vec::new();
        let mut keys = Vec::new();
        for name in &meta.shared_with {
            match people.key_of(name) {
                Some(key) => {
                    if !meta.shared_keys.iter().any(|known| known == key) {
                        changes.push(format!("{name} → {}", short_key(key)));
                    }
                    names.push(name.clone());
                    keys.push(key.to_string());
                }
                None => {
                    waiting = true;
                    changes.push(format!(
                        "{name} has not connected yet, so they are left out"
                    ));
                }
            }
        }

        let changed =
            owner_key != meta.owner_key || names != meta.shared_with || keys != meta.shared_keys;
        if !changed && !waiting {
            continue;
        }
        if changed && write {
            if let Some(slot) = library.notes.get_mut(&id) {
                slot.owner_key = owner_key;
                slot.shared_with = names;
                slot.shared_keys = keys;
            }
            library.write(&id)?;
        }
        report.push(AdoptedNote {
            title: meta.title.clone(),
            id,
            changes,
            waiting,
        });
    }
    Ok(report)
}

/// The first eight characters of a key, enough to tell two people apart on screen.
fn short_key(key: &str) -> String {
    key.chars().take(8).collect()
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

    /// The public key a test person's identifier turns into. The tests never need the digest
    /// itself, so they write one as `key-nox` and it stands for the same thing everywhere.
    fn key_of(name: &str) -> String {
        format!("key-{name}")
    }

    /// A guest list entry: the label a sidebar prints, and the key that actually lets them in.
    fn guest(name: &str) -> (String, String) {
        (name.to_string(), key_of(name))
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("elora-server-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_pictures_live_in_the_library_folder() {
        let dir = temp_dir("attachments");
        let library = Library::load(&dir).unwrap();
        assert!(library.read_attachment("cat-1234abcd.png").is_none());

        library
            .write_attachment("cat-1234abcd.png", b"\x89PNG picture")
            .unwrap();
        assert_eq!(
            library.read_attachment("cat-1234abcd.png").as_deref(),
            Some(&b"\x89PNG picture"[..])
        );
        // An ordinary file beside the notes, so copying or backing up the folder keeps it.
        assert!(dir.join("attachments/cat-1234abcd.png").is_file());

        // Nothing can be stored outside the pictures folder.
        assert!(library.write_attachment("../escape.png", b"x").is_err());
        assert!(!dir.join("escape.png").exists());
        assert!(library.write_attachment("notes.md", b"x").is_err());
        // An empty file is not a picture and is never handed out.
        assert!(library.write_attachment("empty-00000000.png", b"").is_ok());
        assert!(library.read_attachment("empty-00000000.png").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_library_creates_and_persists_markdown() {
        let dir = temp_dir("library");
        let mut library = Library::load(&dir).unwrap();
        let meta = library
            .create(
                "Meeting notes",
                None,
                Vec::new(),
                "",
                false,
                None,
                "nox",
                &key_of("nox"),
            )
            .unwrap();
        assert!(meta.id.starts_with("meeting-notes-"));
        assert_eq!(meta.owner.as_deref(), Some("nox"));
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
                "nox",
                &key_of("nox"),
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
            .create(
                "Shared",
                None,
                Vec::new(),
                "",
                false,
                None,
                "nox",
                &key_of("nox"),
            )
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
            .create(
                "Tags",
                None,
                Vec::new(),
                "",
                false,
                None,
                "nox",
                &key_of("nox"),
            )
            .unwrap();
        library
            .update(
                &meta.id,
                &key_of("nox"),
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
            .update(
                &meta.id,
                &key_of("nox"),
                None,
                Field::Clear,
                None,
                None,
                None,
                Field::Clear,
            )
            .unwrap();
        let reloaded = Library::load(&dir).unwrap();
        let note = reloaded.notes.get(&meta.id).unwrap();
        assert_eq!(note.folder, None, "the folder should have been cleared");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A title is typed one character at a time, so the server keeps it exactly as it arrives.
    /// Trimming it used to swallow the space at the end of a half-typed word and send the
    /// trimmed title back, which joined the next word onto the last one.
    #[test]
    fn test_a_typing_title_keeps_the_space_at_its_end() {
        let dir = temp_dir("title-space");
        let mut library = Library::load(&dir).unwrap();
        let meta = library
            .create(
                "Untitled",
                None,
                Vec::new(),
                "",
                false,
                None,
                "nox",
                &key_of("nox"),
            )
            .unwrap();

        for typed in ["Meeting", "Meeting ", "Meeting n", "Meeting notes "] {
            library
                .update(
                    &meta.id,
                    &key_of("nox"),
                    Some(typed.to_string()),
                    Field::Keep,
                    None,
                    None,
                    None,
                    Field::Keep,
                )
                .unwrap();
            assert_eq!(
                library.notes[&meta.id].title, typed,
                "the title is kept as it was typed"
            );
        }
        // And it is still there after a restart, trailing space and all.
        drop(library);
        let reloaded = Library::load(&dir).unwrap();
        assert_eq!(reloaded.notes[&meta.id].title, "Meeting notes ");

        // A title with nothing in it is not a title: the note keeps the one it had.
        let mut library = reloaded;
        library
            .update(
                &meta.id,
                &key_of("nox"),
                Some("   ".to_string()),
                Field::Keep,
                None,
                None,
                None,
                Field::Keep,
            )
            .unwrap();
        assert_eq!(library.notes[&meta.id].title, "Meeting notes ");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_password_compare_is_exact() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreT"));
        assert!(!constant_time_eq(b"secret", b"secret "));
    }

    /// Opening a note hands its text around, and a client with nothing to add still answers the
    /// server's state request. That answer is not an edit: it must not restamp the note, which
    /// is what sorted the sidebar by it and pushed a note to the top on a mere click.
    #[test]
    fn test_an_update_that_adds_nothing_is_not_an_edit() {
        let _guard = crate::tls::server_dir_lock();
        let dir = temp_dir("noop-update");
        let notes = dir.join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let id = "quiet-0001".to_string();
        std::fs::write(
            notes.join(format!("{id}.md")),
            "---\ntitle: \"Quiet\"\npinned: false\ntrashed: false\ntags: []\n\
             created_at: 2020-01-01T00:00:00+00:00\nupdated_at: 2020-01-01T00:00:00+00:00\n---\n\
             unchanged",
        )
        .unwrap();
        // SAFETY: the guard keeps the other tests that read this variable out of the way.
        unsafe { std::env::set_var("ELORA_SERVER_DIR", dir.join("server")) };
        let config = ServerConfig {
            name: "Quiet".into(),
            port: 0,
            password: "letmein".into(),
            notes_dir: notes,
            bind: "127.0.0.1".into(),
        };
        let server = Server::new(config).expect("server");
        {
            let mut library = server.library.lock().unwrap();
            library.open(&id).expect("open");
        }

        // A note waiting to be written out is the server's own record of "this changed", and
        // it is what restamps the note and tells everyone to reorder their list.
        let pending_write = || server.library.lock().unwrap().dirty.contains_key(&id);

        // Exactly what a client with an empty document sends back after being asked for its
        // state.
        let empty = {
            let library = server.library.lock().unwrap();
            let room = &library.rooms[&id];
            let txn = room.doc.transact();
            txn.encode_state_as_update_v1(&StateVector::default())
        };
        server.apply_update(&id, 7, empty);
        assert!(
            !pending_write(),
            "opening a note must not count as a change to it"
        );

        // A real edit still counts, so the list keeps following the text.
        let edited = {
            let doc = Doc::new();
            let text = doc.get_or_insert_text("body");
            {
                let mut txn = doc.transact_mut();
                text.insert(&mut txn, 0, "and now a word");
            }
            let txn = doc.transact();
            txn.encode_state_as_update_v1(&StateVector::default())
        };
        server.apply_update(&id, 9, edited);
        assert!(pending_write(), "an edit is a change to the note");
        let _ = std::fs::remove_dir_all(&dir);
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
            id: "older app".into(),
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

    /// The whole point of the keys: a note follows the key, not the name. Somebody who connects
    /// under the same display name as the owner — because the name was free, or because they got
    /// into the server's name list — is a different person to the note.
    #[test]
    fn test_taking_somebodys_name_does_not_open_their_notes() {
        let dir = temp_dir("name-theft");
        let mut library = Library::load(&dir).unwrap();
        let meta = library
            .create(
                "Diary",
                None,
                Vec::new(),
                "",
                false,
                None,
                "skorp",
                &key_of("skorp"),
            )
            .unwrap();
        library
            .set_share(&meta.id, &key_of("skorp"), true, Vec::new())
            .unwrap();
        assert!(library
            .list_for(&key_of("skorp"))
            .iter()
            .any(|note| note.id == meta.id));

        // The impostor's key is not the owner's, so the note is not theirs to see or to change,
        // however they introduce themselves.
        assert!(!library
            .list_for(&key_of("impostor"))
            .iter()
            .any(|note| note.id == meta.id));
        let refused = library.set_share(&meta.id, &key_of("impostor"), false, Vec::new());
        assert!(refused.is_err(), "a name is not a key: {refused:?}");
        assert!(library.notes[&meta.id].private, "the note stays closed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A note that was closed to everybody but its owner is not in anybody else's library, and
    /// naming somebody on it lets exactly that one person in.
    #[test]
    fn test_a_restricted_note_is_listed_only_for_the_people_on_it() {
        let dir = temp_dir("share-acl");
        let mut library = Library::load(&dir).unwrap();
        let meta = library
            .create(
                "Diary",
                None,
                Vec::new(),
                "",
                false,
                None,
                "nox",
                &key_of("nox"),
            )
            .unwrap();
        let listed = |library: &Library, who: &str| {
            library
                .list_for(&key_of(who))
                .iter()
                .any(|note| note.id == meta.id)
        };
        // A new note belongs to whoever made it but starts out open to the server.
        assert_eq!(meta.owner.as_deref(), Some("nox"));
        assert!(!meta.private);
        assert!(listed(&library, "nox"));
        assert!(listed(&library, "mira"));

        library
            .set_share(&meta.id, &key_of("nox"), true, Vec::new())
            .unwrap();
        assert!(listed(&library, "nox"), "the owner keeps it");
        assert!(!listed(&library, "mira"), "only my eyes means only mine");

        // Names are kept as they were typed, without the owner and without a repeat that only
        // differs in case.
        library
            .set_share(
                &meta.id,
                &key_of("nox"),
                true,
                vec![guest("MirA"), guest("nox"), guest("mira"), guest("MIRA")],
            )
            .unwrap();
        assert_eq!(
            library.notes[&meta.id].shared_with,
            vec!["MirA".to_string()]
        );
        assert_eq!(library.notes[&meta.id].shared_keys, vec![key_of("MirA")]);
        assert!(listed(&library, "mira"), "a named person sees it");
        assert!(!listed(&library, "stranger"));

        // The restriction is in the note's own frontmatter, so it survives a restart.
        let reloaded = Library::load(&dir).unwrap();
        let note = &reloaded.notes[&meta.id];
        assert!(note.private);
        assert_eq!(note.shared_with, vec!["MirA".to_string()]);
        assert!(!reloaded
            .list_for(&key_of("stranger"))
            .iter()
            .any(|note| note.id == meta.id));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The owner decides who sees a note. Somebody the note was shared with may still write in
    /// it, but cannot lock the owner out, and loses even that when the note is closed again.
    #[test]
    fn test_only_the_owner_changes_who_sees_a_note() {
        let dir = temp_dir("share-owner");
        let mut library = Library::load(&dir).unwrap();
        let meta = library
            .create(
                "Plan",
                None,
                Vec::new(),
                "",
                false,
                None,
                "nox",
                &key_of("nox"),
            )
            .unwrap();
        library
            .update(
                &meta.id,
                &key_of("mira"),
                Some("Plan v2".into()),
                Field::Keep,
                None,
                None,
                None,
                Field::Keep,
            )
            .expect("an open note is everybody's to edit");

        let refused = library.set_share(&meta.id, &key_of("mira"), true, Vec::new());
        assert!(
            matches!(&refused, Err(error) if error.contains("only nox")),
            "a guest must not be able to close the note: {refused:?}"
        );
        assert!(!library.notes[&meta.id].private);

        library
            .set_share(&meta.id, &key_of("nox"), true, Vec::new())
            .unwrap();
        assert!(library
            .update(
                &meta.id,
                &key_of("mira"),
                Some("hijack".into()),
                Field::Keep,
                None,
                None,
                None,
                Field::Keep,
            )
            .is_err());
        assert!(library.delete(&meta.id, &key_of("mira"), false).is_err());
        library.open(&meta.id).unwrap();
        assert_eq!(library.notes[&meta.id].title, "Plan v2");

        // Opening it again takes it back to the whole server.
        library
            .set_share(&meta.id, &key_of("nox"), false, Vec::new())
            .unwrap();
        assert!(library.notes[&meta.id].shared_with.is_empty());
        assert!(library
            .list_for(&key_of("mira"))
            .iter()
            .any(|n| n.id == meta.id));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A note locked before notes carried keys names only its owner's name, which proves
    /// nothing. It stays shut until the server's operator adopts it, and adopting gives it the
    /// key that name belongs to now — with a preview first, so nothing is written blindly.
    #[test]
    fn test_notes_locked_before_keys_existed_are_adopted_from_the_name_list() {
        let dir = temp_dir("adopt");
        let notes = dir.join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let id = "legacy-0001".to_string();
        std::fs::write(
            notes.join(format!("{id}.md")),
            "---\ntitle: \"Old diary\"\npinned: false\ntrashed: false\ntags: []\n\
             owner: \"skorp\"\nprivate: true\n\
             created_at: 2020-01-01T00:00:00+00:00\nupdated_at: 2020-01-01T00:00:00+00:00\n---\n\
             secret",
        )
        .unwrap();

        let secret = "skorp-identifier-for-the-test";
        let skorp = people::public_key(secret);
        let mut registry = Registry::default();
        registry.claim("skorp", &skorp).unwrap();

        // Before adoption the note belongs to nobody a connection can prove it is.
        let library = Library::load(&notes).unwrap();
        assert!(library.notes[&id].owner_key.is_none());
        assert!(!net::visible_to(&library.notes[&id], &skorp));

        // The preview says what it would do and writes nothing.
        let preview = adopt_note_keys(&notes, &registry, false).unwrap();
        assert_eq!(preview.len(), 1, "{preview:?}");
        assert!(!preview[0].waiting);
        assert!(!preview[0].changes.is_empty(), "it should say what changes");
        assert!(Library::load(&notes).unwrap().notes[&id]
            .owner_key
            .is_none());

        // `--write` gives the note its owner's key, and the holder of the identifier is the
        // only one who can open it.
        adopt_note_keys(&notes, &registry, true).unwrap();
        let adopted = Library::load(&notes).unwrap();
        assert_eq!(
            adopted.notes[&id].owner_key.as_deref(),
            Some(skorp.as_str())
        );
        assert!(net::visible_to(&adopted.notes[&id], &skorp));
        let stranger = people::public_key("somebody-elses-identifier");
        assert!(!net::visible_to(&adopted.notes[&id], &stranger));

        // A name nobody has claimed cannot be adopted: the report says so and leaves the note.
        let other = dir.join("other");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(
            other.join("orphan-0001.md"),
            "---\ntitle: \"Orphan\"\npinned: false\ntrashed: false\ntags: []\n\
             owner: \"nobody-here\"\nprivate: true\n\
             created_at: 2020-01-01T00:00:00+00:00\nupdated_at: 2020-01-01T00:00:00+00:00\n---\n\
             secret",
        )
        .unwrap();
        let report = adopt_note_keys(&other, &registry, true).unwrap();
        assert_eq!(report.len(), 1, "{report:?}");
        assert!(report[0].waiting, "it has to say the owner is unknown");
        assert!(Library::load(&other).unwrap().notes["orphan-0001"]
            .owner_key
            .is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Even with the right note id, a room only opens for the people the note is meant for.
    #[test]
    fn test_a_restricted_note_does_not_open_for_a_stranger() {
        let _guard = crate::tls::server_dir_lock();
        let dir = temp_dir("share-room");
        let notes = dir.join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let id = "closed-0001".to_string();
        std::fs::write(
            notes.join(format!("{id}.md")),
            "---\ntitle: \"Closed\"\npinned: false\ntrashed: false\ntags: []\n\
             owner: \"nox\"\nowner_key: \"key-nox\"\nprivate: true\n\
             shared_with: [\"Mira\"]\nshared_keys: [\"key-mira\"]\n\
             created_at: 2020-01-01T00:00:00+00:00\nupdated_at: 2020-01-01T00:00:00+00:00\n---\n\
             secret",
        )
        .unwrap();
        // SAFETY: the guard keeps the other tests that read this variable out of the way.
        unsafe { std::env::set_var("ELORA_SERVER_DIR", dir.join("server")) };
        let config = ServerConfig {
            name: "Closed".into(),
            port: 0,
            password: "letmein".into(),
            notes_dir: notes,
            bind: "127.0.0.1".into(),
        };
        let server = Server::new(config).expect("server");
        let client = |identity: &str| {
            Arc::new(Client {
                id: 1,
                name: Mutex::new(identity.to_string()),
                key: Mutex::new(format!("key-{identity}")),
                room: Mutex::new(None),
                awareness: Mutex::new(HashSet::new()),
                authed: AtomicBool::new(true),
                outbox: Mutex::new(Vec::new()),
            })
        };

        let stranger = client("stranger");
        let refused = server.open_room(&stranger, &id).unwrap_err();
        assert!(refused.contains("not shared"), "{refused}");
        assert!(server.library.lock().unwrap().rooms.is_empty());

        let named = client("mira");
        server
            .clients
            .lock()
            .unwrap()
            .insert(named.id, Arc::clone(&named));
        server
            .open_room(&named, &id)
            .expect("a named person gets in");
        let subscribed = |server: &Server| {
            server
                .library
                .lock()
                .unwrap()
                .rooms
                .get(&id)
                .is_some_and(|room| room.subscribers.contains(&1))
        };
        assert!(subscribed(&server));

        // The owner closes the note again while it is open for the other person: they are taken
        // out of it, so nothing typed after that reaches them.
        server
            .library
            .lock()
            .unwrap()
            .set_share(&id, &key_of("nox"), true, Vec::new())
            .unwrap();
        server.revoke_members(&id);
        assert!(!subscribed(&server), "a closed note has no guests in it");
        assert!(named.room.lock().unwrap().is_none());

        let owner = client("nox");
        server.open_room(&owner, &id).expect("the owner gets in");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
