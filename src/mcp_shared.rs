//! The shared library, as `elora --mcp` reaches it.
//!
//! The app talks to the shared library through a [`Collab`] and a window. This is the same client
//! with no window: the server settings come from the vault's own settings file, the connection is
//! made the first time an assistant asks for something, and each call waits for the server's
//! answer, because a tool call has to return a result. It connects as the person whose settings
//! they are, so the server's own rules about who may see and change a note apply unchanged.

use crate::collab::{Collab, Incoming};
use crate::net::NoteMeta;
use crate::people::MIN_IDENTIFIER;
use crate::storage::{AiMarks, AppSettings, Note};
use chrono::{TimeZone, Utc};
use std::cell::RefCell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// How long a connection may take to be accepted.
const CONNECT_WAIT: Duration = Duration::from_secs(10);
/// How long the server has to answer one request.
const ANSWER_WAIT: Duration = Duration::from_secs(8);
/// After a failed connection, how long before another is tried, so a server that is down does
/// not make every tool call wait out the timeout again.
const RETRY_AFTER: Duration = Duration::from_secs(30);

/// What a new shared note starts with.
pub struct NewNote<'a> {
    pub title: &'a str,
    pub folder: Option<&'a str>,
    pub tags: &'a [String],
    pub body: &'a str,
    pub pinned: bool,
    pub icon: Option<&'a str>,
}

pub struct Shared {
    state: RefCell<State>,
}

struct State {
    collab: Collab,
    /// When the last connection attempt failed, and why.
    failed: Option<(Instant, String)>,
}

impl Default for Shared {
    fn default() -> Self {
        Self::new()
    }
}

impl Shared {
    pub fn new() -> Self {
        Self {
            state: RefCell::new(State {
                collab: Collab::new(),
                failed: None,
            }),
        }
    }

    /// What a status line can say about the shared library, or `None` when there is none set up.
    pub fn status(&self, settings: &AppSettings) -> Option<String> {
        if !settings.server.is_configured() {
            return None;
        }
        Some(match self.library(settings) {
            Ok(Some(notes)) => format!(
                "connected to {} ({} notes)",
                self.server_name().unwrap_or_default(),
                notes.len()
            ),
            Ok(None) => return None,
            Err(why) => format!("unavailable: {why}"),
        })
    }

    fn server_name(&self) -> Option<String> {
        self.state.borrow().collab.server_name().map(str::to_string)
    }

    /// The notes in the shared library, or `Ok(None)` when no library is set up here. Connects
    /// first if it has not yet.
    pub fn library(&self, settings: &AppSettings) -> Result<Option<Vec<NoteMeta>>, String> {
        if !self.ensure(settings)? {
            return Ok(None);
        }
        let mut state = self.state.borrow_mut();
        state.collab.poll();
        Ok(Some(state.collab.notes().to_vec()))
    }

    /// Makes sure there is a live connection. `Ok(false)` means there is nothing to connect to.
    fn ensure(&self, settings: &AppSettings) -> Result<bool, String> {
        if !settings.server.is_configured() {
            return Ok(false);
        }
        let mut state = self.state.borrow_mut();
        if state.collab.is_connected() {
            return Ok(true);
        }
        if let Some((when, why)) = &state.failed {
            if when.elapsed() < RETRY_AFTER {
                return Err(why.clone());
            }
        }
        let name = settings.display_name.trim().to_string();
        let identifier = settings.identifier.clone();
        let problem = if name.is_empty() {
            Some("set your name in EloraNotes under Settings → Shared library".to_string())
        } else if identifier.trim().len() < MIN_IDENTIFIER {
            Some("the shared library needs your identifier; set it in EloraNotes".to_string())
        } else {
            None
        };
        if let Some(why) = problem {
            state.failed = Some((Instant::now(), why.clone()));
            return Err(why);
        }

        state
            .collab
            .connect(settings.server.clone(), name, identifier);
        let result = wait_for(&mut state.collab, CONNECT_WAIT, |event| match event {
            Incoming::Online { .. } => Some(Ok(())),
            Incoming::Unverified { .. } => Some(Err(
                "the server's certificate has not been trusted yet: connect to it once in \
                 EloraNotes first"
                    .to_string(),
            )),
            Incoming::Offline(reason) => Some(Err(reason.clone())),
            _ => None,
        });
        match result {
            Ok(()) => {
                state.failed = None;
                Ok(true)
            }
            Err(why) => {
                // The worker keeps retrying in the background; a failed attempt is over.
                state.collab.disconnect();
                state.failed = Some((Instant::now(), why.clone()));
                Err(why)
            }
        }
    }

    /// The text of a shared note, as the room holds it now.
    pub fn read_body(&self, settings: &AppSettings, id: &str) -> Result<String, String> {
        self.edit_body(settings, id, |old| old.to_string())
    }

    /// Changes a shared note's text: `change` is given what the room holds and returns what it
    /// should hold. Returns the new text.
    pub fn edit_body(
        &self,
        settings: &AppSettings,
        id: &str,
        change: impl FnOnce(&str) -> String,
    ) -> Result<String, String> {
        if !self.ensure(settings)? {
            return Err("no shared library is set up".to_string());
        }
        let mut state = self.state.borrow_mut();
        state.collab.open_note(id, "");
        let target = id.to_string();
        let current = wait_for(&mut state.collab, ANSWER_WAIT, |event| match event {
            Incoming::Body { id, text } if *id == target => Some(Ok(text.clone())),
            Incoming::Refused(why) => Some(Err(why.clone())),
            _ => None,
        });
        let current = match current {
            Ok(text) => text,
            Err(why) => {
                state.collab.close_note();
                return Err(why);
            }
        };
        let new = change(&current);
        if new != current {
            state.collab.send_body(id, &new);
        }
        // Commands reach the worker in order, so the change is sent before the room is left.
        state.collab.close_note();
        Ok(new)
    }

    /// Makes a note in the shared library.
    pub fn create(&self, settings: &AppSettings, note: &NewNote) -> Result<NoteMeta, String> {
        if !self.ensure(settings)? {
            return Err("no shared library is set up".to_string());
        }
        let mut state = self.state.borrow_mut();
        state.collab.create_note(
            note.title,
            note.folder,
            note.tags,
            note.body,
            note.pinned,
            note.icon,
        );
        wait_for(&mut state.collab, ANSWER_WAIT, |event| match event {
            Incoming::Created(meta) => Some(Ok(meta.clone())),
            Incoming::Refused(why) => Some(Err(why.clone())),
            _ => None,
        })
    }

    /// Changes a shared note's title, folder, tags, pin, trash or icon. The server's answer is
    /// the library with the change in it, and that note is returned.
    pub fn update(&self, settings: &AppSettings, meta: &NoteMeta) -> Result<NoteMeta, String> {
        if !self.ensure(settings)? {
            return Err("no shared library is set up".to_string());
        }
        let mut state = self.state.borrow_mut();
        state.collab.update_note(meta);
        let wanted = meta.clone();
        wait_for(&mut state.collab, ANSWER_WAIT, |event| match event {
            Incoming::Notes(notes) => notes
                .iter()
                .find(|n| n.id == wanted.id && same_properties(n, &wanted))
                .map(|n| Ok(n.clone())),
            Incoming::Refused(why) => Some(Err(why.clone())),
            _ => None,
        })
    }

    /// Deletes a shared note for good.
    pub fn delete(&self, settings: &AppSettings, id: &str) -> Result<(), String> {
        if !self.ensure(settings)? {
            return Err("no shared library is set up".to_string());
        }
        let mut state = self.state.borrow_mut();
        state.collab.delete_note(id, true);
        let target = id.to_string();
        wait_for(&mut state.collab, ANSWER_WAIT, |event| match event {
            Incoming::Notes(notes) if notes.iter().all(|n| n.id != target) => Some(Ok(())),
            Incoming::Refused(why) => Some(Err(why.clone())),
            _ => None,
        })
    }
}

/// True when two copies of a note agree on everything an update can change.
fn same_properties(a: &NoteMeta, b: &NoteMeta) -> bool {
    a.title == b.title
        && a.folder == b.folder
        && a.tags == b.tags
        && a.pinned == b.pinned
        && a.trashed == b.trashed
        && a.icon == b.icon
}

/// Reads what the connection reports until `pick` accepts something or the time is up.
fn wait_for<T>(
    collab: &mut Collab,
    timeout: Duration,
    mut pick: impl FnMut(&Incoming) -> Option<Result<T, String>>,
) -> Result<T, String> {
    let deadline = Instant::now() + timeout;
    loop {
        for event in collab.poll() {
            if let Some(found) = pick(&event) {
                return found;
            }
        }
        if Instant::now() >= deadline {
            return Err("the shared library did not answer in time".to_string());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// A shared note as the same type local notes have. It has no file here, which is how the two
/// are told apart, and no text until it is asked for.
pub fn note_from_meta(meta: &NoteMeta, body: String) -> Note {
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
        extra_frontmatter: String::new(),
        ai: AiMarks::default(),
        created_at: updated,
        updated_at: updated,
        file_path: PathBuf::new(),
    }
}

/// True for a note that lives in the shared library rather than in a file on this computer.
pub fn is_shared(note: &Note) -> bool {
    note.file_path.as_os_str().is_empty()
}

/// `meta` with the fields an update can change taken from `note`.
pub fn meta_with(meta: &NoteMeta, note: &Note) -> NoteMeta {
    NoteMeta {
        title: note.title.clone(),
        folder: note.folder.clone(),
        tags: note.explicit_tags.clone(),
        pinned: note.pinned,
        trashed: note.trashed,
        icon: note.icon.clone(),
        ..meta.clone()
    }
}
