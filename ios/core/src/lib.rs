//! What the iPhone app asks of EloraNotes' own code: the notes in the app's folder, the shared
//! library, search and links. The screens are SwiftUI; everything that decides what a note *is*
//! stays in the same Rust the desktop app and the server use.
//!
//! There is nothing here about AI apps. The phone app does not set them up, and does not show
//! what they did.

use elora_notes::collab::ServerProfile;
use elora_notes::graph_layout::{GraphNode, GraphView};
use elora_notes::live::LiveNote;
use elora_notes::mcp_shared::{is_shared, meta_with, note_from_meta, NewNote, Shared};
use elora_notes::net::NoteMeta;
use elora_notes::storage::{extract_wiki_links, AppSettings, Note, NoteStorage};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::time::Duration;

uniffi::setup_scaffolding!();

/// Something that went wrong, in words that can be shown to a person.
#[derive(Debug, uniffi::Error)]
pub enum CoreError {
    Failed { message: String },
}

impl std::fmt::Display for CoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoreError::Failed { message } => f.write_str(message),
        }
    }
}

impl std::error::Error for CoreError {}

fn failed(message: impl ToString) -> CoreError {
    CoreError::Failed {
        message: message.to_string(),
    }
}

/// A note as a row in a list.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct NoteSummary {
    pub id: String,
    pub title: String,
    /// The note's own icon, by Tabler Icons name (`flare`, `device-gamepad`), when it has one.
    pub icon: Option<String>,
    pub folder: Option<String>,
    pub tags: Vec<String>,
    pub pinned: bool,
    /// Lives in the shared library rather than in this phone's own folder.
    pub shared: bool,
    /// Unix seconds.
    pub updated: i64,
    pub snippet: String,
}

/// One `[[link]]` in a note, and the note it leads to (none when there is no such note yet).
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct NoteLink {
    pub text: String,
    pub id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct NoteDetail {
    pub summary: NoteSummary,
    pub body: String,
    pub links: Vec<NoteLink>,
    /// Notes whose text links here. Shared notes only count once they have been opened.
    pub backlinks: Vec<NoteSummary>,
}

/// What a listing found, with the reason the shared library is missing when it is.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct NoteList {
    pub notes: Vec<NoteSummary>,
    pub shared_error: Option<String>,
    /// The shared library's own name, once connected: the title of its folder.
    pub server_name: Option<String>,
}

/// How to reach a shared library.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct ServerSettings {
    pub host: String,
    pub port: u16,
    pub password: String,
    /// The certificate fingerprint the person confirmed; none until they have.
    pub fingerprint: Option<String>,
    pub display_name: String,
    pub identifier: String,
}

impl ServerSettings {
    fn app_settings(&self) -> AppSettings {
        AppSettings {
            server: ServerProfile {
                host: self.host.clone(),
                port: self.port,
                password: self.password.clone(),
                fingerprint: self.fingerprint.clone(),
            },
            display_name: self.display_name.clone(),
            identifier: self.identifier.clone(),
            ..AppSettings::default()
        }
    }
}

/// One note in the graph, where it sits and which cluster it belongs to.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct GraphPoint {
    pub id: String,
    pub title: String,
    pub x: f32,
    pub y: f32,
    /// The cluster, largest first; none for a note nothing links to.
    pub group: Option<u32>,
    /// How many notes it is linked with.
    pub degree: u32,
    pub shared: bool,
}

/// A link between two notes, as positions in `GraphData::points`.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct GraphEdge {
    pub from: u32,
    pub to: u32,
}

/// The graph, laid out by the same code the desktop app draws it with: linked notes in clusters
/// and the notes nothing links to on an arc below.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct GraphData {
    pub points: Vec<GraphPoint>,
    pub edges: Vec<GraphEdge>,
    pub shared_error: Option<String>,
}

/// Somebody else with a shared note open: who, in what colour, and where their caret is.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct LivePerson {
    pub name: String,
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    /// Their caret, in bytes into the note's text.
    pub cursor: Option<u32>,
    pub typing: bool,
}

/// What a look at an open shared note found.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct LiveUpdate {
    pub connected: bool,
    pub error: Option<String>,
    /// The whole text, when somebody else changed it (and the first time it is known).
    pub text: Option<String>,
    pub people: Vec<LivePerson>,
}

/// A shared note held open for live editing. Closed when it is dropped.
#[derive(uniffi::Object)]
pub struct LiveSession {
    note: LiveNote,
}

#[uniffi::export]
impl LiveSession {
    /// What has changed since the last look: cheap enough to ask every few hundredths of a second.
    pub fn poll(&self) -> LiveUpdate {
        let look = self.note.poll();
        LiveUpdate {
            connected: look.connected,
            error: look.error,
            text: look.text,
            people: look
                .peers
                .into_iter()
                .map(|peer| LivePerson {
                    name: peer.name,
                    red: peer.color[0],
                    green: peer.color[1],
                    blue: peer.color[2],
                    cursor: peer.cursor.map(|byte| byte as u32),
                    typing: peer.typing,
                })
                .collect(),
        }
    }

    /// The editor's whole text after a change.
    pub fn set_text(&self, text: String) {
        self.note.set_text(&text);
    }

    /// Where the caret is, in bytes into the text.
    pub fn set_cursor(&self, byte: u32) {
        self.note.set_cursor(byte as usize);
    }

    pub fn close(&self) {
        self.note.close();
    }
}

type Job = Box<dyn FnOnce(&Shared) + Send>;

/// The shared library's client is not `Send`, so it lives on a thread of its own and calls are
/// handed to it. One connection serves every call instead of one being made for each.
struct Worker {
    jobs: Mutex<mpsc::Sender<Job>>,
}

impl Worker {
    fn new() -> Self {
        let (jobs, inbox) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("elora-shared".to_string())
            .spawn(move || {
                let shared = Shared::new();
                while let Ok(job) = inbox.recv() {
                    job(&shared);
                }
            })
            .ok();
        Self {
            jobs: Mutex::new(jobs),
        }
    }

    fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Shared) -> T + Send + 'static,
    ) -> Result<T, CoreError> {
        let (reply, answer) = mpsc::channel();
        let job: Job = Box::new(move |shared| {
            let _ = reply.send(work(shared));
        });
        lock(&self.jobs)
            .send(job)
            .map_err(|_| failed("the shared library connection stopped"))?;
        answer
            .recv()
            .map_err(|_| failed("the shared library connection stopped"))
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct Local {
    storage: NoteStorage,
    notes: Vec<Note>,
}

/// The notes a person has on this phone and, when one is set up, in the shared library.
#[derive(uniffi::Object)]
pub struct Vault {
    local: Mutex<Local>,
    server: Mutex<Option<ServerSettings>>,
    worker: Worker,
    server_name: Mutex<Option<String>>,
    /// The text of shared notes already read, by id, with the version it was read at, so
    /// backlinks and search can use it without going back to the server.
    read: Mutex<HashMap<String, (i64, String)>>,
}

fn summary_of(note: &Note) -> NoteSummary {
    NoteSummary {
        id: note.id.clone(),
        title: note.display_title(),
        icon: note.icon.clone(),
        folder: note.folder.clone(),
        tags: note.all_tags(),
        pinned: note.pinned,
        shared: is_shared(note),
        updated: note.updated_at.timestamp(),
        snippet: if is_shared(note) && note.body.is_empty() {
            String::new()
        } else {
            note.snippet()
        },
    }
}

fn newest_first(notes: &mut [NoteSummary]) {
    notes.sort_by(|a, b| {
        b.pinned
            .cmp(&a.pinned)
            .then(b.updated.cmp(&a.updated))
            .then(a.title.cmp(&b.title))
    });
}

impl Vault {
    fn settings(&self) -> Option<AppSettings> {
        lock(&self.server)
            .as_ref()
            .map(ServerSettings::app_settings)
    }

    /// The shared library's notes: the ones in the trash when `trashed`, the others when not.
    fn shared_notes(&self, trashed: bool) -> Result<Vec<Note>, String> {
        let Some(settings) = self.settings() else {
            return Ok(Vec::new());
        };
        let (library, name) = self
            .worker
            .run(move |shared| (shared.library(&settings), shared.server_name()))
            .map_err(|e| e.to_string())?;
        let library = library?;
        *lock(&self.server_name) = name;
        Ok(library
            .unwrap_or_default()
            .iter()
            .filter(|meta| meta.trashed == trashed)
            .map(|meta| {
                let body = lock(&self.read)
                    .get(&meta.id)
                    .filter(|(stamp, _)| *stamp == meta.updated_at)
                    .map(|(_, text)| text.clone())
                    .unwrap_or_default();
                note_from_meta(meta, body)
            })
            .collect())
    }

    fn all_notes(&self) -> (Vec<Note>, Option<String>) {
        let mut notes: Vec<Note> = lock(&self.local)
            .notes
            .iter()
            .filter(|n| !n.trashed)
            .cloned()
            .collect();
        match self.shared_notes(false) {
            Ok(shared) => {
                notes.extend(shared);
                (notes, None)
            }
            Err(why) => (notes, Some(why)),
        }
    }

    fn meta_of(&self, id: &str) -> Result<NoteMeta, CoreError> {
        let settings = self
            .settings()
            .ok_or_else(|| failed("no shared library is set up"))?;
        let wanted = id.to_string();
        let library = self
            .worker
            .run(move |shared| shared.library(&settings))?
            .map_err(failed)?
            .unwrap_or_default();
        library
            .into_iter()
            .find(|meta| meta.id == wanted)
            .ok_or_else(|| failed("that note is not in the shared library"))
    }
}

#[uniffi::export]
impl Vault {
    /// Opens (and makes, if need be) the folder the phone's own notes live in.
    #[uniffi::constructor]
    pub fn open(directory: String) -> Result<Arc<Self>, CoreError> {
        let storage = NoteStorage::new(PathBuf::from(directory)).map_err(failed)?;
        let notes = storage.load_all().map_err(failed)?;
        Ok(Arc::new(Self {
            local: Mutex::new(Local { storage, notes }),
            server: Mutex::new(None),
            worker: Worker::new(),
            server_name: Mutex::new(None),
            read: Mutex::new(HashMap::new()),
        }))
    }

    /// Reads the phone's folder again, for when another app (Files, iCloud) changed it.
    pub fn reload(&self) -> Result<(), CoreError> {
        let mut local = lock(&self.local);
        local.notes = local.storage.load_all().map_err(failed)?;
        Ok(())
    }

    /// Sets, or with `None` forgets, the shared library to use.
    pub fn set_server(&self, settings: Option<ServerSettings>) {
        *lock(&self.server) = settings;
        *lock(&self.server_name) = None;
        lock(&self.read).clear();
    }

    /// The fingerprint of a server's certificate, to show a person before they trust it.
    pub fn probe_server(
        &self,
        host: String,
        port: u16,
        password: String,
        display_name: String,
        identifier: String,
    ) -> Result<String, CoreError> {
        let profile = ServerProfile {
            host,
            port,
            password,
            fingerprint: None,
        };
        elora_notes::mcp_shared::probe_fingerprint(&profile, &display_name, &identifier)
            .map_err(failed)
    }

    /// Every note that is not in the trash, pinned and newest first.
    pub fn list_notes(&self) -> NoteList {
        let (notes, shared_error) = self.all_notes();
        let mut notes: Vec<NoteSummary> = notes.iter().map(summary_of).collect();
        newest_first(&mut notes);
        NoteList {
            notes,
            shared_error,
            server_name: lock(&self.server_name).clone(),
        }
    }

    /// A note with its text, the notes it links to and the notes that link to it.
    pub fn read_note(&self, id: String) -> Result<NoteDetail, CoreError> {
        let (mut notes, _) = self.all_notes();
        let position = notes
            .iter()
            .position(|n| n.id == id)
            .ok_or_else(|| failed("that note no longer exists"))?;
        if is_shared(&notes[position]) {
            let settings = self
                .settings()
                .ok_or_else(|| failed("no shared library is set up"))?;
            let wanted = id.clone();
            let body = self
                .worker
                .run(move |shared| shared.read_body(&settings, &wanted))?
                .map_err(failed)?;
            lock(&self.read).insert(
                id.clone(),
                (notes[position].updated_at.timestamp(), body.clone()),
            );
            notes[position].body = body;
        }
        let note = &notes[position];
        let links = extract_wiki_links(&note.body)
            .into_iter()
            .map(|text| {
                let found = notes.iter().find(|n| {
                    n.title.eq_ignore_ascii_case(&text)
                        || n.display_title().eq_ignore_ascii_case(&text)
                });
                NoteLink {
                    id: found.map(|n| n.id.clone()),
                    text,
                }
            })
            .collect();
        let title = note.display_title();
        let mut backlinks: Vec<NoteSummary> = notes
            .iter()
            .filter(|other| other.id != note.id)
            .filter(|other| {
                extract_wiki_links(&other.body).iter().any(|link| {
                    link.eq_ignore_ascii_case(&title) || link.eq_ignore_ascii_case(&note.title)
                })
            })
            .map(summary_of)
            .collect();
        newest_first(&mut backlinks);
        Ok(NoteDetail {
            summary: summary_of(note),
            body: note.body.clone(),
            links,
            backlinks,
        })
    }

    /// Opens a shared note for live editing: its text, and everybody else's carets, as they move.
    pub fn open_live(&self, id: String) -> Result<Arc<LiveSession>, CoreError> {
        let server = lock(&self.server)
            .clone()
            .ok_or_else(|| failed("no shared library is set up"))?;
        if server.fingerprint.is_none() {
            return Err(failed(
                "confirm the server's certificate in Settings before editing shared notes",
            ));
        }
        let settings = server.app_settings();
        Ok(Arc::new(LiveSession {
            note: LiveNote::open(
                settings.server,
                settings.display_name,
                settings.identifier,
                id,
            ),
        }))
    }

    /// Replaces a note's text with `body` and returns the note as it is now.
    pub fn save_note(&self, id: String, body: String) -> Result<NoteSummary, CoreError> {
        {
            let mut local = lock(&self.local);
            if let Some(index) = local.notes.iter().position(|n| n.id == id) {
                local.notes[index].body = body;
                local.notes[index].updated_at = chrono::Utc::now();
                let note = local.notes[index].clone();
                local.storage.save_note(&note).map_err(failed)?;
                return Ok(summary_of(&note));
            }
        }
        let settings = self
            .settings()
            .ok_or_else(|| failed("that note is not on this phone"))?;
        let meta = self.meta_of(&id)?;
        let (wanted, text) = (id.clone(), body.clone());
        let saved = self
            .worker
            .run(move |shared| shared.edit_body(&settings, &wanted, |_| text))?
            .map_err(failed)?;
        lock(&self.read).insert(id, (meta.updated_at, saved.clone()));
        Ok(summary_of(&note_from_meta(&meta, saved)))
    }

    /// Makes a note, on the phone or in the shared library.
    pub fn create_note(
        &self,
        title: String,
        folder: Option<String>,
        body: String,
        shared: bool,
    ) -> Result<NoteSummary, CoreError> {
        let title = title.trim().to_string();
        if title.is_empty() {
            return Err(failed("a note needs a title"));
        }
        if shared {
            let settings = self
                .settings()
                .ok_or_else(|| failed("no shared library is set up"))?;
            let meta = self
                .worker
                .run(move |shared| {
                    shared.create(
                        &settings,
                        &NewNote {
                            title: &title,
                            folder: folder.as_deref(),
                            tags: &[],
                            body: &body,
                            pinned: false,
                            icon: None,
                        },
                    )
                })?
                .map_err(failed)?;
            return Ok(summary_of(&note_from_meta(&meta, String::new())));
        }
        let mut local = lock(&self.local);
        let mut note = Note::new(title, body, local.storage.root_dir());
        note.folder = folder.and_then(|f| elora_notes::storage::normalize_folder(&f));
        local.storage.save_note(&note).map_err(failed)?;
        let made = summary_of(&note);
        local.notes.push(note);
        Ok(made)
    }

    /// Moves a note to the trash.
    pub fn trash_note(&self, id: String) -> Result<(), CoreError> {
        {
            let mut local = lock(&self.local);
            if let Some(index) = local.notes.iter().position(|n| n.id == id) {
                local.notes[index].trashed = true;
                let note = local.notes[index].clone();
                local.storage.save_note(&note).map_err(failed)?;
                return Ok(());
            }
        }
        let settings = self
            .settings()
            .ok_or_else(|| failed("that note is not on this phone"))?;
        let mut meta = self.meta_of(&id)?;
        meta.trashed = true;
        self.worker
            .run(move |shared| shared.update(&settings, &meta).map(|_| ()))?
            .map_err(failed)
    }

    /// The graph of every note. A shared note's links are only on the server until it is read, so
    /// they are read here (and kept, so the next graph does not read them again).
    pub fn graph(&self) -> GraphData {
        let (notes, mut shared_error) = self.all_notes();
        let unread: Vec<(String, i64)> = notes
            .iter()
            .filter(|n| is_shared(n) && n.body.is_empty())
            .map(|n| (n.id.clone(), n.updated_at.timestamp()))
            .collect();
        let mut bodies: HashMap<String, String> = HashMap::new();
        if let (false, Some(settings)) = (unread.is_empty(), self.settings()) {
            let ids: Vec<String> = unread.iter().map(|(id, _)| id.clone()).collect();
            let read = self.worker.run(move |shared| {
                let mut found = Vec::new();
                let mut failures = 0;
                for id in ids {
                    match shared.read_body(&settings, &id) {
                        Ok(body) => {
                            failures = 0;
                            found.push((id, body));
                        }
                        Err(why) => {
                            failures += 1;
                            if failures >= 3 {
                                return (found, Some(why));
                            }
                        }
                    }
                }
                (found, None)
            });
            match read {
                Ok((found, why)) => {
                    for (id, body) in found {
                        if let Some((_, stamp)) = unread.iter().find(|(other, _)| *other == id) {
                            lock(&self.read).insert(id.clone(), (*stamp, body.clone()));
                        }
                        bodies.insert(id, body);
                    }
                    shared_error = why.or(shared_error);
                }
                Err(why) => shared_error = Some(why.to_string()),
            }
        }

        let mut nodes: Vec<GraphNode> = notes
            .iter()
            .map(|n| GraphNode {
                id: n.id.clone(),
                label: n.display_title(),
                links: Vec::new(),
            })
            .collect();
        for (i, note) in notes.iter().enumerate() {
            let text = bodies.get(&note.id).unwrap_or(&note.body);
            for target in extract_wiki_links(text) {
                let found = notes.iter().position(|n| {
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

        let mut view = GraphView::new(nodes);
        view.settle(Duration::from_secs(4));
        let world = view.world_positions();
        GraphData {
            points: notes
                .iter()
                .enumerate()
                .map(|(i, note)| GraphPoint {
                    id: note.id.clone(),
                    title: note.display_title(),
                    x: world[i].x,
                    y: world[i].y,
                    group: view.group[i].map(|g| g as u32),
                    degree: view.degree[i] as u32,
                    shared: is_shared(note),
                })
                .collect(),
            edges: view
                .edges
                .iter()
                .map(|&(a, b)| GraphEdge {
                    from: a as u32,
                    to: b as u32,
                })
                .collect(),
            shared_error,
        }
    }

    /// Moves a note into a folder (or out of its folder with `None`).
    pub fn set_folder(&self, id: String, folder: Option<String>) -> Result<NoteSummary, CoreError> {
        let folder = folder.and_then(|name| elora_notes::storage::normalize_folder(&name));
        {
            let mut local = lock(&self.local);
            if let Some(index) = local.notes.iter().position(|n| n.id == id) {
                local.notes[index].folder = folder;
                local.notes[index].updated_at = chrono::Utc::now();
                let note = local.notes[index].clone();
                local.storage.save_note(&note).map_err(failed)?;
                return Ok(summary_of(&note));
            }
        }
        let settings = self
            .settings()
            .ok_or_else(|| failed("that note is not on this phone"))?;
        let meta = self.meta_of(&id)?;
        let mut note = note_from_meta(&meta, String::new());
        note.folder = folder;
        let wanted = meta_with(&meta, &note);
        let saved = self
            .worker
            .run(move |shared| shared.update(&settings, &wanted))?
            .map_err(failed)?;
        Ok(summary_of(&note_from_meta(&saved, String::new())))
    }

    /// Makes a copy on this phone: of a note here, or of a shared note, which brings it down from
    /// the server. The copy is named after the note and ends in "copy".
    pub fn copy_note(&self, id: String) -> Result<NoteSummary, CoreError> {
        let (notes, _) = self.all_notes();
        let source = notes
            .into_iter()
            .find(|n| n.id == id)
            .ok_or_else(|| failed("that note no longer exists"))?;
        let body = if is_shared(&source) {
            let settings = self
                .settings()
                .ok_or_else(|| failed("no shared library is set up"))?;
            let wanted = id.clone();
            self.worker
                .run(move |shared| shared.read_body(&settings, &wanted))?
                .map_err(failed)?
        } else {
            source.body.clone()
        };
        let mut local = lock(&self.local);
        let taken: Vec<String> = local
            .notes
            .iter()
            .map(|n| n.display_title().to_lowercase())
            .collect();
        let base = source.display_title();
        let mut title = format!("{base} copy");
        let mut number = 2;
        while taken.contains(&title.to_lowercase()) {
            title = format!("{base} copy {number}");
            number += 1;
        }
        let mut copy = Note::new(title, body, local.storage.root_dir());
        copy.folder = source.folder.clone();
        copy.explicit_tags = source.explicit_tags.clone();
        copy.icon = source.icon.clone();
        local.storage.save_note(&copy).map_err(failed)?;
        let made = summary_of(&copy);
        local.notes.push(copy);
        Ok(made)
    }

    /// The notes in the trash, on this phone and in the shared library, newest first.
    pub fn list_trash(&self) -> NoteList {
        let mut notes: Vec<Note> = lock(&self.local)
            .notes
            .iter()
            .filter(|n| n.trashed)
            .cloned()
            .collect();
        let mut shared_error = None;
        match self.shared_notes(true) {
            Ok(shared) => notes.extend(shared),
            Err(why) => shared_error = Some(why),
        }
        let mut notes: Vec<NoteSummary> = notes.iter().map(summary_of).collect();
        newest_first(&mut notes);
        NoteList {
            notes,
            shared_error,
            server_name: lock(&self.server_name).clone(),
        }
    }

    /// Takes a note out of the trash.
    pub fn restore_note(&self, id: String) -> Result<(), CoreError> {
        {
            let mut local = lock(&self.local);
            if let Some(index) = local.notes.iter().position(|n| n.id == id) {
                local.notes[index].trashed = false;
                let note = local.notes[index].clone();
                local.storage.save_note(&note).map_err(failed)?;
                return Ok(());
            }
        }
        let settings = self
            .settings()
            .ok_or_else(|| failed("that note is not on this phone"))?;
        let mut meta = self.meta_of(&id)?;
        meta.trashed = false;
        self.worker
            .run(move |shared| shared.update(&settings, &meta).map(|_| ()))?
            .map_err(failed)
    }

    /// Deletes a note for good. A note in the shared library is gone for everybody.
    pub fn delete_forever(&self, id: String) -> Result<(), CoreError> {
        {
            let mut local = lock(&self.local);
            if let Some(index) = local.notes.iter().position(|n| n.id == id) {
                let note = local.notes.remove(index);
                return local.storage.delete_note(&note).map_err(failed);
            }
        }
        let settings = self
            .settings()
            .ok_or_else(|| failed("that note is not on this phone"))?;
        let wanted = id.clone();
        self.worker
            .run(move |shared| shared.delete(&settings, &wanted))?
            .map_err(failed)?;
        lock(&self.read).remove(&id);
        Ok(())
    }

    /// Notes matching what was typed, best first. A shared note is matched on its title, tags
    /// and folder, and on its text once it has been opened.
    pub fn search(&self, query: String) -> Vec<NoteSummary> {
        let query = query.trim().to_string();
        let (notes, _) = self.all_notes();
        if query.is_empty() {
            return Vec::new();
        }
        let mut found: Vec<(usize, NoteSummary)> = notes
            .iter()
            .filter(|note| note.matches_filter(&query, None))
            .map(|note| (note.relevance_score(&query), summary_of(note)))
            .collect();
        found.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.updated.cmp(&a.1.updated)));
        found.into_iter().map(|(_, summary)| summary).collect()
    }
}

/// One of the formatting buttons above the keyboard: the same set the desktop editor has.
#[derive(Clone, Copy, Debug, PartialEq, uniffi::Enum)]
pub enum FormatButton {
    Bold,
    Italic,
    Strikethrough,
    Underline,
    InlineCode,
    Highlight { red: u8, green: u8, blue: u8 },
    TextColor { red: u8, green: u8, blue: u8 },
    WikiLink,
    Heading1,
    Heading2,
    Bullet,
    Checklist,
    Quote,
    CodeFence,
    ClearFormatting,
}

/// The text after a button was pressed, with the selection to put back. Positions are in UTF-16
/// units, which is how iOS counts text.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct FormattedText {
    pub text: String,
    pub start: u32,
    pub end: u32,
}

/// Which character of `text` the UTF-16 offset `offset` is at.
fn char_at_utf16(text: &str, offset: usize) -> usize {
    let mut units = 0;
    for (index, c) in text.chars().enumerate() {
        if units >= offset {
            return index;
        }
        units += c.len_utf16();
    }
    text.chars().count()
}

/// How many UTF-16 units the first `count` characters of `text` take.
fn utf16_of_chars(text: &str, count: usize) -> usize {
    text.chars().take(count).map(char::len_utf16).sum()
}

/// Presses a formatting button with the selection at `start..end` (UTF-16 units): the very change
/// the desktop's button makes, from the same code.
#[uniffi::export]
pub fn format_text(text: String, start: u32, end: u32, button: FormatButton) -> FormattedText {
    use elora_notes::format::{apply, Op};
    let op = match button {
        FormatButton::Bold => Op::Bold,
        FormatButton::Italic => Op::Italic,
        FormatButton::Strikethrough => Op::Strikethrough,
        FormatButton::Underline => Op::Underline,
        FormatButton::InlineCode => Op::InlineCode,
        FormatButton::Highlight { red, green, blue } => Op::Highlight([red, green, blue]),
        FormatButton::TextColor { red, green, blue } => Op::TextColor([red, green, blue]),
        FormatButton::WikiLink => Op::WikiLink,
        FormatButton::Heading1 => Op::Heading1,
        FormatButton::Heading2 => Op::Heading2,
        FormatButton::Bullet => Op::Bullet,
        FormatButton::Checklist => Op::Checklist,
        FormatButton::Quote => Op::Quote,
        FormatButton::CodeFence => Op::CodeFence,
        FormatButton::ClearFormatting => Op::ClearFormatting,
    };
    let range = (
        char_at_utf16(&text, start as usize),
        char_at_utf16(&text, end as usize),
    );
    let done = apply(op, &text, range);
    FormattedText {
        start: utf16_of_chars(&done.text, done.selection.0) as u32,
        end: utf16_of_chars(&done.text, done.selection.1) as u32,
        text: done.text,
    }
}

/// The character that draws a Tabler icon in the icon font, by the icon's name.
#[uniffi::export]
pub fn icon_glyph(name: String) -> Option<String> {
    use elora_notes::icon_names::TABLER_ICON_NAMES;
    TABLER_ICON_NAMES
        .binary_search_by_key(&name.as_str(), |(known, _)| known)
        .ok()
        .map(|at| TABLER_ICON_NAMES[at].1.to_string())
}

/// The version of this crate, for the About screen.
#[uniffi::export]
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault(name: &str) -> (Arc<Vault>, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "elora-mobile-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        (Vault::open(dir.display().to_string()).unwrap(), dir)
    }

    #[test]
    fn notes_are_made_listed_linked_searched_and_trashed() {
        let (vault, dir) = vault("local");
        // A folder with nothing in it starts with the same few starter notes the desktop app has.
        let starter = vault.list_notes().notes.len();
        assert!(starter > 0);
        let spec = vault
            .create_note("Game Spec".into(), None, "The plan.".into(), false)
            .unwrap();
        let ideas = vault
            .create_note(
                "Ideas".into(),
                Some("Games".into()),
                "See [[Game Spec]] and [[Missing]].".into(),
                false,
            )
            .unwrap();
        assert!(vault
            .create_note("  ".into(), None, String::new(), false)
            .is_err());

        let list = vault.list_notes();
        assert_eq!(list.notes.len(), starter + 2);
        assert_eq!(list.shared_error, None);
        assert!(list.notes.iter().all(|n| !n.shared));

        let read = vault.read_note(ideas.id.clone()).unwrap();
        assert_eq!(read.summary.folder.as_deref(), Some("Games"));
        assert_eq!(read.links.len(), 2);
        assert_eq!(read.links[0].id.as_deref(), Some(spec.id.as_str()));
        assert_eq!(read.links[1].id, None, "no note called Missing yet");
        let target = vault.read_note(spec.id.clone()).unwrap();
        assert_eq!(target.backlinks.len(), 1);
        assert_eq!(target.backlinks[0].id, ideas.id);

        // An edit is kept, and is there after the folder is read again from disk.
        vault
            .save_note(spec.id.clone(), "A new plan about slimes.".into())
            .unwrap();
        vault.reload().unwrap();
        assert!(vault
            .read_note(spec.id.clone())
            .unwrap()
            .body
            .contains("slimes"));
        assert_eq!(vault.search("slimes".into()).len(), 1);
        assert!(vault.search("   ".into()).is_empty());

        vault.trash_note(ideas.id.clone()).unwrap();
        assert_eq!(vault.list_notes().notes.len(), starter + 1);
        vault.reload().unwrap();
        assert_eq!(
            vault.list_notes().notes.len(),
            starter + 1,
            "trashing is kept too"
        );
        assert!(vault.read_note(ideas.id).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_shared_library_that_cannot_be_reached_leaves_the_phones_notes_working() {
        let (vault, dir) = vault("offline");
        let starter = vault.list_notes().notes.len();
        vault
            .create_note("Mine".into(), None, "text".into(), false)
            .unwrap();
        vault.set_server(Some(ServerSettings {
            host: "127.0.0.1".into(),
            port: 1,
            password: "secret".into(),
            fingerprint: None,
            display_name: "Me".into(),
            identifier: "a-long-enough-identifier".into(),
        }));
        let list = vault.list_notes();
        assert_eq!(list.notes.len(), starter + 1);
        assert!(list.shared_error.is_some_and(|why| !why.is_empty()));
        assert!(vault
            .create_note("Remote".into(), None, String::new(), true)
            .is_err());
        // With the server forgotten it is as before.
        vault.set_server(None);
        assert_eq!(vault.list_notes().shared_error, None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_graph_clusters_linked_notes_and_sets_the_rest_below() {
        let (vault, dir) = vault("graph");
        for hub in ["Alpha", "Beta"] {
            let spokes: String = (0..6).map(|i| format!("[[{hub} {i}]] ")).collect();
            vault.create_note(hub.into(), None, spokes, false).unwrap();
            for i in 0..6 {
                vault
                    .create_note(
                        format!("{hub} {i}"),
                        None,
                        format!("Back to [[{hub}]]"),
                        false,
                    )
                    .unwrap();
            }
        }
        for i in 0..10 {
            vault
                .create_note(
                    format!("Loose {i}"),
                    None,
                    "nothing links here".into(),
                    false,
                )
                .unwrap();
        }
        let graph = vault.graph();
        assert_eq!(graph.shared_error, None);
        let loose: Vec<&GraphPoint> = graph
            .points
            .iter()
            .filter(|p| p.title.starts_with("Loose"))
            .collect();
        assert_eq!(loose.len(), 10);
        assert!(loose.iter().all(|p| p.group.is_none() && p.degree == 0));
        let alpha = graph.points.iter().find(|p| p.title == "Alpha").unwrap();
        let beta = graph.points.iter().find(|p| p.title == "Beta").unwrap();
        assert!(alpha.degree >= 6 && alpha.group.is_some());
        assert_ne!(alpha.group, beta.group, "two topics, two colours");
        let linked_y: f32 = graph
            .points
            .iter()
            .filter(|p| p.group.is_some())
            .map(|p| p.y)
            .sum::<f32>()
            / graph.points.iter().filter(|p| p.group.is_some()).count() as f32;
        let loose_y: f32 = loose.iter().map(|p| p.y).sum::<f32>() / loose.len() as f32;
        assert!(
            loose_y > linked_y,
            "the unlinked notes sit below the linked ones"
        );
        assert!(graph.edges.iter().all(
            |e| (e.from as usize) < graph.points.len() && (e.to as usize) < graph.points.len()
        ));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn live_editing_needs_a_confirmed_server() {
        let (vault, dir) = vault("live");
        assert!(vault.open_live("anything".into()).is_err());
        vault.set_server(Some(ServerSettings {
            host: "127.0.0.1".into(),
            port: 1,
            password: "secret".into(),
            fingerprint: None,
            display_name: "Me".into(),
            identifier: "a-long-enough-identifier".into(),
        }));
        assert!(vault.open_live("anything".into()).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn notes_move_between_folders_are_copied_and_come_back_from_the_trash() {
        let (vault, dir) = vault("manage");
        let note = vault
            .create_note("Plan".into(), None, "the plan [[Other]]".into(), false)
            .unwrap();

        // Into a folder, out of it again.
        let moved = vault
            .set_folder(note.id.clone(), Some("  Games ".into()))
            .unwrap();
        assert_eq!(moved.folder.as_deref(), Some("Games"));
        assert_eq!(
            vault.set_folder(note.id.clone(), None).unwrap().folder,
            None
        );
        vault
            .set_folder(note.id.clone(), Some("Games".into()))
            .unwrap();

        // A copy keeps the text and the folder, and does not clash with the note or an earlier copy.
        let copy = vault.copy_note(note.id.clone()).unwrap();
        assert_eq!(copy.title, "Plan copy");
        assert_eq!(copy.folder.as_deref(), Some("Games"));
        assert_ne!(copy.id, note.id);
        assert_eq!(
            vault.read_note(copy.id.clone()).unwrap().body,
            "the plan [[Other]]"
        );
        assert_eq!(
            vault.copy_note(note.id.clone()).unwrap().title,
            "Plan copy 2"
        );

        // Deleting puts it in the trash, where it can be brought back or deleted for good.
        let before = vault.list_notes().notes.len();
        vault.trash_note(copy.id.clone()).unwrap();
        assert_eq!(vault.list_notes().notes.len(), before - 1);
        let trash = vault.list_trash();
        assert_eq!(trash.notes.len(), 1);
        assert_eq!(trash.notes[0].id, copy.id);
        vault.restore_note(copy.id.clone()).unwrap();
        assert!(vault.list_trash().notes.is_empty());
        assert_eq!(vault.list_notes().notes.len(), before);

        vault.trash_note(copy.id.clone()).unwrap();
        vault.delete_forever(copy.id.clone()).unwrap();
        assert!(vault.list_trash().notes.is_empty());
        vault.reload().unwrap();
        assert!(vault.read_note(copy.id).is_err(), "gone from the disk too");
        // The note that was copied is untouched.
        assert_eq!(vault.read_note(note.id).unwrap().body, "the plan [[Other]]");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn formatting_buttons_count_text_the_way_ios_does() {
        // An emoji is two UTF-16 units, so "word" starts at offset 4 here, not 3.
        let done = format_text("😀 word".into(), 3, 7, FormatButton::Bold);
        assert_eq!(done.text, "😀 **word**");
        assert_eq!((done.start, done.end), (5, 9));
        let again = format_text(done.text, done.start, done.end, FormatButton::Bold);
        assert_eq!(again.text, "😀 word");
        assert_eq!((again.start, again.end), (3, 7));
        let heading = format_text(String::new(), 0, 0, FormatButton::Heading1);
        assert_eq!(heading.text, "\n# Heading 1\n");
        let glow = format_text(
            "hi".into(),
            0,
            2,
            FormatButton::Highlight {
                red: 255,
                green: 200,
                blue: 0,
            },
        );
        assert_eq!(glow.text, "=={#FFC800}hi==");
    }

    #[test]
    fn note_icons_are_found_by_name() {
        assert_eq!(icon_glyph("feather".into()).as_deref(), Some("\u{ee8b}"));
        assert_eq!(icon_glyph("no-such-icon".into()), None);
        let (vault, dir) = vault("icons");
        let made = vault
            .create_note("Plain".into(), None, String::new(), false)
            .unwrap();
        assert_eq!(made.icon, None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_version_is_the_crates() {
        assert_eq!(core_version(), env!("CARGO_PKG_VERSION"));
    }
}
