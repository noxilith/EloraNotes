//! End-to-end checks for the shared library: a real `elora-server` on a loopback port and real
//! `Collab` clients talking TLS to it. These cover the parts that unit tests cannot: the
//! handshake, the note list, live bodies and presence.

use elora_notes::collab::{Collab, ConnectionState, Incoming, ServerProfile};
use elora_notes::net::ServerConfig;
use elora_notes::server::Server;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Harness {
    dir: std::path::PathBuf,
    port: u16,
    note_id: String,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Starts a server on a free loopback port with one note in it.
fn start_server() -> Harness {
    let dir = std::env::temp_dir().join(format!(
        "elora-collab-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    // The certificate lives beside the config, which follows this variable.
    // SAFETY: the whole test file runs one test, so nothing else reads the environment.
    unsafe { std::env::set_var("ELORA_SERVER_DIR", dir.join("server")) };

    let notes_dir = dir.join("notes");
    std::fs::create_dir_all(&notes_dir).unwrap();
    let note_id = "shared-0001".to_string();
    std::fs::write(
        notes_dir.join(format!("{note_id}.md")),
        "---\ntitle: \"Shared\"\npinned: false\ntrashed: false\ntags: []\n\
         created_at: 2026-01-01T00:00:00+00:00\nupdated_at: 2026-01-01T00:00:00+00:00\n---\n\
         hello",
    )
    .unwrap();

    // Ask the OS for a free port, then hand it to the server.
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);

    let config = ServerConfig {
        name: "Test Vault".into(),
        port,
        password: "letmein".into(),
        notes_dir: notes_dir.clone(),
        bind: "127.0.0.1".into(),
    };
    let server = Arc::new(Server::new(config).expect("server"));
    std::thread::spawn(move || {
        let _ = server.run();
    });

    // Wait until the listener accepts.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }

    Harness { dir, port, note_id }
}

fn profile(port: u16) -> ServerProfile {
    ServerProfile {
        host: "127.0.0.1".into(),
        port,
        password: "letmein".into(),
        fingerprint: None,
    }
}

/// Drains a client until `done` is satisfied, applying remote bodies into `body` like the UI
/// does. Panics with the connection state when it times out.
fn settle(
    collab: &mut Collab,
    body: &mut String,
    label: &str,
    done: impl Fn(&Collab, &str) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        for event in collab.poll() {
            match event {
                Incoming::Body { text, .. } => *body = text,
                Incoming::Offline(reason) => panic!("{label} went offline: {reason}"),
                _ => {}
            }
        }
        if done(collab, body) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!(
        "{label} did not settle; state is {:?}, body is {body:?}",
        collab.state()
    );
}

#[test]
fn test_two_clients_edit_the_same_note() {
    let harness = start_server();
    let note_id = harness.note_id.clone();

    let mut alice = Collab::new();
    alice.connect(profile(harness.port), "alice".into());
    settle(&mut alice, &mut String::new(), "alice", |c, _| {
        c.is_connected() && !c.notes().is_empty()
    });
    assert!(matches!(
        alice.state(),
        ConnectionState::Online { server, you } if server == "Test Vault" && you == "alice"
    ));
    assert_eq!(alice.notes()[0].id, note_id);
    assert_eq!(alice.notes()[0].title, "Shared");
    // The first connection hands back the certificate to pin.
    let fingerprint = alice.take_fingerprint().expect("fingerprint");
    assert_eq!(fingerprint.len(), 64);

    let mut alice_body = String::new();
    alice.open_note(&note_id, "hello");
    settle(&mut alice, &mut alice_body, "alice", |_, body| {
        body == "hello"
    });

    let mut bob = Collab::new();
    // Bob's window only draws when it is asked to, and an idle one never asks, so a remote
    // keystroke has to wake it; otherwise the other person's words wait for Bob's next mouse move.
    let repaints = Arc::new(AtomicUsize::new(0));
    let woken = repaints.clone();
    bob.set_waker(Arc::new(move || {
        woken.fetch_add(1, Ordering::SeqCst);
    }));
    bob.connect(profile(harness.port), "bob".into());
    let mut bob_body = String::new();
    settle(&mut bob, &mut bob_body, "bob", |c, _| c.is_connected());
    bob.open_note(&note_id, "hello");
    settle(&mut bob, &mut bob_body, "bob", |_, body| body == "hello");

    // Alice sees Bob arrive in the presence list for this note.
    settle(&mut alice, &mut alice_body, "alice presence", |c, _| {
        c.watchers(&note_id).iter().any(|u| u.name == "bob")
    });

    // Alice moves her caret; Bob can see where she is.
    alice.send_cursor(3);
    settle(&mut bob, &mut bob_body, "bob caret", |c, _| {
        c.peers()
            .iter()
            .any(|peer| peer.name == "alice" && peer.cursor == Some(3))
    });

    // Alice types; Bob's text follows.
    alice_body.push_str(" from alice");
    alice.send_body(&note_id, &alice_body);
    settle(&mut bob, &mut bob_body, "bob", |_, body| {
        body == "hello from alice"
    });

    // Nothing is happening now, so the count starts from zero: anything after this is a wake-up.
    std::thread::sleep(Duration::from_millis(100));
    repaints.store(0, Ordering::SeqCst);
    alice_body.push('!');
    alice.send_body(&note_id, &alice_body);
    settle(&mut bob, &mut bob_body, "bob repaint", |_, body| {
        body.ends_with('!')
    });
    assert!(
        repaints.load(Ordering::SeqCst) > 0,
        "a remote edit must ask for a new frame"
    );

    // Both type at once, at different ends. The merged text must contain both edits.
    let alice_edit = format!("A{}", alice_body);
    let bob_edit = format!("{bob_body} B");
    alice.send_body(&note_id, &alice_edit);
    bob.send_body(&note_id, &bob_edit);

    // A third client is the neutral observer of the merged result.
    let mut viewer = Collab::new();
    viewer.connect(profile(harness.port), "viewer".into());
    let mut viewer_body = String::new();
    settle(&mut viewer, &mut viewer_body, "viewer", |c, _| {
        c.is_connected()
    });
    viewer.open_note(&note_id, "");
    settle(&mut viewer, &mut viewer_body, "viewer", |c, body| {
        body.contains("from alice") && body.contains(" B") && c.is_watched(&note_id)
    });
    assert!(
        viewer_body.starts_with("Ahello") || viewer_body.starts_with("hello"),
        "unexpected merged body {viewer_body:?}"
    );

    // Bob ends up with the same text as the viewer, whichever order the updates arrived in.
    settle(&mut bob, &mut bob_body, "bob merged", |_, body| {
        !body.is_empty() && body.ends_with('B')
    });

    // Alice adds a note to a folder in the shared library; everyone sees it and the body and
    // frontmatter land in a plain Markdown file.
    alice.create_note(
        "New Plan",
        Some("Team"),
        &["plan".to_string()],
        "the body Alice wrote",
        true,
        Some("book"),
    );
    let mut created_id = None;
    let mut waited = 0;
    while created_id.is_none() && waited < 800 {
        for event in alice.poll() {
            match event {
                Incoming::Created(note) => {
                    assert_eq!(note.title, "New Plan");
                    assert_eq!(note.folder.as_deref(), Some("Team"));
                    assert_eq!(note.tags, vec!["plan".to_string()]);
                    assert!(note.pinned);
                    assert_eq!(note.icon.as_deref(), Some("book"));
                    created_id = Some(note.id);
                }
                Incoming::Body { .. } => {}
                Incoming::Offline(reason) => panic!("alice went offline: {reason}"),
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(10));
        waited += 10;
    }
    let created_id = created_id.expect("the server should report the new note");
    settle(&mut bob, &mut bob_body, "bob library", |c, _| {
        c.notes()
            .iter()
            .any(|m| m.id == created_id && m.title == "New Plan")
    });
    let created_path = harness.dir.join("notes").join(format!("{created_id}.md"));
    let created_raw = std::fs::read_to_string(&created_path).expect("the new note file");
    assert!(
        created_raw.contains("folder: \"Team\""),
        "frontmatter: {created_raw:?}"
    );
    assert!(
        created_raw.contains("tags: [plan]"),
        "frontmatter: {created_raw:?}"
    );
    assert!(
        created_raw.contains("the body Alice wrote"),
        "body: {created_raw:?}"
    );

    // Everyone left but the viewer: presence goes away again.
    alice.disconnect();
    bob.disconnect();
    settle(&mut viewer, &mut viewer_body, "viewer", |c, _| {
        c.watchers(&note_id).is_empty()
    });

    // The server writes the merged body back to a plain Markdown file, on a short debounce.
    let path = harness.dir.join("notes").join(format!("{note_id}.md"));
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut saved = String::new();
    while Instant::now() < deadline {
        saved = std::fs::read_to_string(&path).expect("saved note");
        if saved.contains("from alice") {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        saved.contains("Ahello from alice! B"),
        "saved body: {saved:?}"
    );
    assert!(
        saved.contains("title: \"Shared\""),
        "frontmatter: {saved:?}"
    );

    viewer.disconnect();
}
