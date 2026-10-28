//! One shared note held open for live editing, without a window.
//!
//! The desktop app talks to the shared library through a [`Collab`] and an egui window; the phone
//! has no such window, so this is the same client behind a small interface: hand it the text as
//! it changes and the caret as it moves, and ask it what changed on the other side. It keeps its
//! own connection, so a note being edited never waits behind a list being loaded, and it asks for
//! the note again by itself when the connection comes back.
//!
//! Everything that makes concurrent typing work (placing each change where it was meant, however
//! far behind the other side is) is the code the desktop uses; this only carries it.

use crate::collab::{Collab, Incoming, ServerProfile};
use std::sync::mpsc::{channel, Sender, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// Somebody else with the note open.
#[derive(Clone, Debug, PartialEq)]
pub struct LivePeer {
    pub name: String,
    pub color: [u8; 3],
    /// Their caret, in bytes into the note's text, when they have one.
    pub cursor: Option<usize>,
    /// True while they are typing.
    pub typing: bool,
}

/// What a look at the note found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiveSnapshot {
    /// True while the connection is up and the note is open on it.
    pub connected: bool,
    /// Why it is not, or what the server refused.
    pub error: Option<String>,
    /// The note's whole text, when it has changed since the last look: the first time it is
    /// known, and after anybody else's typing. Never set for text this side just sent.
    pub text: Option<String>,
    pub peers: Vec<LivePeer>,
}

enum Command {
    /// The editor's text, and a signal for when it has been handed on.
    Text(String, Sender<()>),
    Cursor(usize),
    Close,
}

#[derive(Default)]
struct State {
    connected: bool,
    error: Option<String>,
    text: Option<String>,
    peers: Vec<LivePeer>,
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A shared note open for live editing. Dropping it closes the connection.
pub struct LiveNote {
    commands: Sender<Command>,
    state: Arc<Mutex<State>>,
}

impl LiveNote {
    /// Connects and opens `note_id`. Returns at once; [`LiveNote::poll`] says when the text is
    /// there. The profile's certificate fingerprint has to be one the person already confirmed.
    pub fn open(profile: ServerProfile, name: String, identifier: String, note_id: String) -> Self {
        let (commands, inbox) = channel();
        let state = Arc::new(Mutex::new(State::default()));
        let shared = state.clone();
        std::thread::Builder::new()
            .name("elora-live".to_string())
            .spawn(move || run(profile, name, identifier, note_id, inbox, shared))
            .ok();
        Self { commands, state }
    }

    /// Hands over the editor's whole text after a change. Only what changed goes to the server.
    /// Returns once the text has been taken in, so a look at the note afterwards can never hold
    /// an answer that is older than this text.
    pub fn set_text(&self, text: &str) {
        let (done, taken) = channel();
        if self
            .commands
            .send(Command::Text(text.to_string(), done))
            .is_ok()
        {
            let _ = taken.recv_timeout(Duration::from_millis(500));
        }
    }

    /// Where the caret is, in bytes into the text.
    pub fn set_cursor(&self, byte: usize) {
        let _ = self.commands.send(Command::Cursor(byte));
    }

    /// What has changed since the last look.
    pub fn poll(&self) -> LiveSnapshot {
        let mut state = lock(&self.state);
        LiveSnapshot {
            connected: state.connected,
            error: state.error.clone(),
            text: state.text.take(),
            peers: state.peers.clone(),
        }
    }

    pub fn close(&self) {
        let _ = self.commands.send(Command::Close);
    }
}

impl Drop for LiveNote {
    fn drop(&mut self) {
        self.close();
    }
}

fn run(
    profile: ServerProfile,
    name: String,
    identifier: String,
    note: String,
    inbox: std::sync::mpsc::Receiver<Command>,
    shared: Arc<Mutex<State>>,
) {
    let mut collab = Collab::new();
    collab.connect(profile, name, identifier);
    let mut open = false;
    loop {
        loop {
            match inbox.try_recv() {
                Ok(Command::Text(text, done)) => {
                    if open {
                        collab.send_body(&note, &text);
                        // Anything waiting to be shown was worked out before this text. If it
                        // matters, the merge of the two comes back as a fresh answer.
                        lock(&shared).text = None;
                    }
                    let _ = done.send(());
                }
                Ok(Command::Cursor(byte)) => {
                    if open {
                        collab.send_cursor(&note, byte);
                    }
                }
                Ok(Command::Close) | Err(TryRecvError::Disconnected) => {
                    collab.close_note();
                    collab.disconnect();
                    return;
                }
                Err(TryRecvError::Empty) => break,
            }
        }

        for event in collab.poll() {
            let mut state = lock(&shared);
            match event {
                Incoming::Online { .. } => {
                    // Also after a reconnect: the new connection has nothing open yet.
                    collab.open_note(&note, "");
                    open = true;
                    state.connected = true;
                    state.error = None;
                }
                Incoming::Body { id, text } if id == note => {
                    collab.note_body_applied(&id, &text);
                    state.text = Some(text);
                }
                Incoming::Refused(why) => state.error = Some(why),
                Incoming::Offline(why) => {
                    open = false;
                    state.connected = false;
                    state.error = Some(why);
                }
                Incoming::Unverified { .. } => {
                    open = false;
                    state.connected = false;
                    state.error = Some(
                        "The server's certificate has not been confirmed yet: connect to it from Settings first."
                            .to_string(),
                    );
                }
                _ => {}
            }
        }
        lock(&shared).peers = collab
            .peers()
            .iter()
            .map(|peer| LivePeer {
                name: peer.name.clone(),
                color: peer.color,
                cursor: peer.cursor,
                typing: peer.typing,
            })
            .collect();
        std::thread::sleep(Duration::from_millis(15));
    }
}
