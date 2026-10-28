//! Two people editing one shared note at once, through `LiveNote`: a real `elora-server` on a
//! loopback port, and two clients that each hold the note open the way the phone app does.

use elora_notes::collab::ServerProfile;
use elora_notes::live::{LiveNote, LiveSnapshot};
use elora_notes::net::ServerConfig;
use elora_notes::people;
use elora_notes::server::Server;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn key(name: &str) -> String {
    format!("{name}-0123456789abcdef0123456789abcdef")
}

struct Harness {
    dir: std::path::PathBuf,
    profile: ServerProfile,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn start_server() -> Harness {
    let dir = std::env::temp_dir().join(format!(
        "elora-live-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    // SAFETY: this file holds a single test, so nothing else reads the environment meanwhile.
    unsafe { std::env::set_var("ELORA_SERVER_DIR", dir.join("server")) };
    let notes = dir.join("server-notes");
    std::fs::create_dir_all(&notes).unwrap();
    let owner_key = people::public_key(&key("alice"));
    std::fs::write(
        notes.join("shared-0001.md"),
        format!(
            "---\ntitle: \"Shared\"\npinned: false\ntrashed: false\ntags: []\n\
             owner: \"alice\"\nowner_key: \"{owner_key}\"\n\
             created_at: 2026-01-01T00:00:00+00:00\n\
             updated_at: 2026-01-01T00:00:00+00:00\n---\nhello world"
        ),
    )
    .unwrap();
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let config = ServerConfig {
        name: "Test Vault".into(),
        port,
        password: "letmein".into(),
        notes_dir: notes,
        bind: "127.0.0.1".into(),
    };
    let server = Arc::new(Server::new(config.clone()).expect("server"));
    let fingerprint = elora_notes::tls::read_fingerprint(&config).expect("fingerprint");
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
    Harness {
        dir,
        profile: ServerProfile {
            host: "127.0.0.1".into(),
            port,
            password: "letmein".into(),
            fingerprint: Some(fingerprint),
        },
    }
}

/// Keeps asking until `done` is satisfied by what has been seen, collecting the text and the last
/// look at the peers on the way.
struct Watcher<'a> {
    note: &'a LiveNote,
    text: Option<String>,
    last: LiveSnapshot,
}

impl<'a> Watcher<'a> {
    fn new(note: &'a LiveNote) -> Self {
        Self {
            note,
            text: None,
            last: LiveSnapshot::default(),
        }
    }

    fn until(&mut self, what: &str, mut done: impl FnMut(&Watcher) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let look = self.note.poll();
            if let Some(text) = look.text.clone() {
                self.text = Some(text);
            }
            self.last = look;
            if done(self) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "gave up waiting for {what}; text {:?}, last look {:?}",
                self.text,
                self.last
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[test]
fn test_two_people_edit_one_shared_note_and_see_each_others_carets() {
    let harness = start_server();
    let alice = LiveNote::open(
        harness.profile.clone(),
        "alice".into(),
        key("alice"),
        "shared-0001".into(),
    );
    let bob = LiveNote::open(
        harness.profile.clone(),
        "bob".into(),
        key("bob"),
        "shared-0001".into(),
    );
    let (mut a, mut b) = (Watcher::new(&alice), Watcher::new(&bob));

    // Each gets the text, and each sees the other as somebody with the note open.
    a.until("alice's first text", |w| w.text.is_some());
    b.until("bob's first text", |w| w.text.is_some());
    assert_eq!(a.text.as_deref(), Some("hello world"));
    assert_eq!(b.text.as_deref(), Some("hello world"));
    a.until("bob to show up for alice", |w| {
        w.last.peers.iter().any(|p| p.name == "bob")
    });
    b.until("alice to show up for bob", |w| {
        w.last.peers.iter().any(|p| p.name == "alice")
    });
    assert!(a.last.connected && b.last.error.is_none());

    // One person's typing reaches the other, and is not handed back to the one who typed it.
    alice.set_text("hello brave world");
    b.until("alice's words", |w| {
        w.text.as_deref() == Some("hello brave world")
    });
    assert_eq!(alice.poll().text, None, "own typing is not an update");

    // Carets travel with a name and a colour.
    alice.set_cursor(6);
    b.until("alice's caret", |w| {
        w.last
            .peers
            .iter()
            .any(|p| p.name == "alice" && p.cursor == Some(6))
    });
    let seen = b.last.peers.iter().find(|p| p.name == "alice").unwrap();
    assert_ne!(seen.color, [0, 0, 0]);

    // Both type at the same moment, each unaware of the other: neither loses a word.
    alice.set_text("Alice says: hello brave world");
    bob.set_text("hello brave world, said Bob");
    let both = |text: &str| text.contains("Alice says: ") && text.contains(", said Bob");
    a.until("alice to see both", |w| w.text.as_deref().is_some_and(both));
    b.until("bob to see both", |w| w.text.as_deref().is_some_and(both));
    assert_eq!(a.text, b.text, "both end on the same text");

    // The server really holds it.
    let on_disk = std::fs::read_to_string(harness.dir.join("server-notes/shared-0001.md")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saved = on_disk;
    while !both(&saved) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        saved = std::fs::read_to_string(harness.dir.join("server-notes/shared-0001.md")).unwrap();
    }
    assert!(both(&saved), "{saved}");

    // Closing one side takes its caret away from the other.
    drop(bob);
    a.until("bob to leave", |w| {
        w.last.peers.iter().all(|p| p.name != "bob")
    });
}
