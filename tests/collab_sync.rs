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
    notes_dir: std::path::PathBuf,
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

    Harness {
        dir,
        port,
        note_id,
        notes_dir,
    }
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

/// The certificate folder follows `ELORA_SERVER_DIR`, which is process-wide, so the tests here
/// take turns instead of starting servers over each other's files.
static SERVER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn test_two_clients_edit_the_same_note() {
    let _turn = SERVER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
            && c.peers().iter().any(|p| p.name == "bob")
    });

    // Bob switches to a note of his own: he is no longer editing this one, so Alice stops
    // drawing his name and caret over it.
    bob.close_note();
    settle(
        &mut alice,
        &mut alice_body,
        "alice after bob left",
        |c, _| c.peers().is_empty() && c.watchers(&note_id).is_empty(),
    );
    bob.open_note(&note_id, &bob_body);
    settle(
        &mut alice,
        &mut alice_body,
        "alice after bob returned",
        |c, _| c.peers().iter().any(|p| p.name == "bob"),
    );

    // Alice moves her caret; Bob can see where she is.
    alice.send_cursor(&note_id, 3);
    settle(&mut bob, &mut bob_body, "bob caret", |c, _| {
        c.peers()
            .iter()
            .any(|peer| peer.name == "alice" && peer.cursor == Some(3))
    });

    // A name typed in Settings reaches the other side without the connection being made again.
    alice.set_name("Alice B.");
    settle(&mut bob, &mut bob_body, "bob sees the new name", |c, _| {
        c.peers().iter().any(|peer| peer.name == "Alice B.")
    });
    alice.set_name("alice");
    settle(&mut bob, &mut bob_body, "bob sees the old name", |c, _| {
        c.peers().iter().all(|peer| peer.name != "Alice B.")
            && c.peers().iter().any(|peer| peer.name == "alice")
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

    // Two machines with the same account name: one leaving must not take the other's presence
    // away with it. Bob is the one looking, since Alice does not show herself.
    let mut twin = Collab::new();
    twin.connect(profile(harness.port), "alice".into());
    settle(&mut twin, &mut String::new(), "twin", |c, _| {
        c.is_connected()
    });
    twin.open_note(&note_id, "");
    settle(&mut bob, &mut bob_body, "bob sees both alices", |c, _| {
        c.peers().iter().filter(|p| p.name == "alice").count() == 2
    });
    twin.disconnect();
    settle(&mut bob, &mut bob_body, "bob keeps alice", |c, _| {
        c.peers().iter().filter(|p| p.name == "alice").count() == 1
    });

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
        body.contains("from alice")
            && body.contains(" B")
            && c.is_watched(&note_id)
            // Whoever was already here is visible straight away, not once they happen to move.
            && c.peers().iter().any(|p| p.name == "alice")
            && c.peers().iter().any(|p| p.name == "bob")
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

    // Opening the note she just made must start from that note's own text. The client document
    // belongs to one note; keeping it across the switch handed the server the note Alice had
    // been looking at, so the new note came out as a copy of it.
    alice.open_note(&created_id, "");
    settle(
        &mut alice,
        &mut alice_body,
        "alice opens the new note",
        |_, body| body == "the body Alice wrote",
    );

    // Text aimed at the note she left must not land in the one she has open: the connection
    // tags every body with its note and drops the ones that do not match.
    alice.send_body(&note_id, "this belongs to the other note");
    std::thread::sleep(Duration::from_millis(300));
    settle(
        &mut viewer,
        &mut viewer_body,
        "viewer keeps its text",
        |_, body| body == "Ahello from alice! B",
    );

    // Bob is still in the shared note; Alice is in the one she just made, so her caret and name
    // must not be drawn over his note.
    settle(
        &mut bob,
        &mut bob_body,
        "bob after alice switched notes",
        |c, _| c.peers().iter().all(|p| p.name != "alice"),
    );
    std::thread::sleep(Duration::from_millis(700));
    let after_open = std::fs::read_to_string(&created_path).expect("the new note file");
    assert!(
        !after_open.contains("from alice"),
        "the note Alice left leaked into the new note: {after_open:?}"
    );
    assert!(
        after_open.contains("the body Alice wrote"),
        "the new note lost its own body: {after_open:?}"
    );

    // Switching back and forth between two shared notes must show each note's own text. A
    // shared note used to keep the text it was left with, so the note reopened after a switch
    // showed the other note until something happened to refresh it.
    let shared_body = "Ahello from alice! B".to_string();
    let new_body = "the body Alice wrote plus a line".to_string();
    alice.send_body(&created_id, &new_body);
    // Her own keystrokes are not echoed back to her, so the write is confirmed on the server.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let saved = std::fs::read_to_string(&created_path).expect("the new note file");
        if saved.contains("plus a line") {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    alice.open_note(&note_id, &shared_body);
    settle(
        &mut alice,
        &mut alice_body,
        "alice back on the shared note",
        |_, body| body == shared_body,
    );
    alice.open_note(&created_id, &new_body);
    settle(
        &mut alice,
        &mut alice_body,
        "alice back on the new note",
        |_, body| body == new_body,
    );
    alice.open_note(&note_id, &shared_body);
    settle(
        &mut alice,
        &mut alice_body,
        "alice on the shared note again",
        |_, body| body == shared_body,
    );

    // Everyone left but the viewer: their names and carets go away with them, not just their
    // sidebar entries.
    alice.disconnect();
    bob.disconnect();
    settle(&mut viewer, &mut viewer_body, "viewer", |c, _| {
        c.watchers(&note_id).is_empty() && c.peers().is_empty()
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

/// A picture added to a shared note goes to the server, so somebody who never had the file can
/// see it — including after the computer that added it has gone.
#[test]
fn test_a_shared_notes_picture_outlives_the_computer_that_added_it() {
    let _turn = SERVER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let harness = start_server();
    let file = "holiday-1a2b3c4d.png";
    // Not a real PNG: the shared library moves bytes and never decodes them.
    let picture: Vec<u8> = b"\x89PNG fake picture bytes".to_vec();

    let mut alice = Collab::new();
    alice.connect(profile(harness.port), "alice".into());
    settle(&mut alice, &mut String::new(), "alice", |c, _| {
        c.is_connected()
    });
    alice.upload_attachment(file, picture.clone());
    assert!(
        wait_for_stored(&mut alice, file),
        "the server never confirmed the picture"
    );
    assert!(
        harness.notes_dir.join("attachments").join(file).is_file(),
        "the picture should sit in the server's own library folder"
    );

    // Bob has never had the file; he asks the library for what the note names.
    let mut bob = Collab::new();
    bob.connect(profile(harness.port), "bob".into());
    settle(&mut bob, &mut String::new(), "bob", |c, _| c.is_connected());
    bob.fetch_attachment(file);
    assert_eq!(
        wait_for_picture(&mut bob, file).as_deref(),
        Some(&picture[..]),
        "a picture added on another computer should arrive byte for byte"
    );

    // A picture that has not been uploaded yet is reported as missing, and when it is uploaded
    // a moment later the clients waiting for it are told to ask again.
    let later = "later-9f8e7d6c.png";
    bob.fetch_attachment(later);
    assert!(
        wait_for_missing(&mut bob, later),
        "an unknown picture should be reported as missing"
    );
    alice.upload_attachment(later, picture.clone());
    assert!(
        wait_for_stored(&mut alice, later),
        "the second picture was not stored"
    );
    assert!(
        wait_for_added(&mut bob, later),
        "bob should hear that the picture he was waiting for has arrived"
    );
    bob.fetch_attachment(later);
    assert_eq!(
        wait_for_picture(&mut bob, later).as_deref(),
        Some(&picture[..]),
        "asking again after the upload should hand the picture over"
    );

    // Alice's computer goes away entirely; the picture is still served from the library.
    alice.disconnect();
    drop(alice);
    bob.fetch_attachment(file);
    assert_eq!(
        wait_for_picture(&mut bob, file).as_deref(),
        Some(&picture[..]),
        "the picture should still be there after its owner left"
    );
}

fn wait_for_stored(collab: &mut Collab, file: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        for event in collab.poll() {
            match event {
                Incoming::AttachmentStored { name } => {
                    assert_eq!(name, file);
                    return true;
                }
                Incoming::AttachmentMissing { name } => panic!("the server refused {name}"),
                Incoming::Offline(reason) => panic!("went offline: {reason}"),
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

fn wait_for_picture(collab: &mut Collab, file: &str) -> Option<Vec<u8>> {
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        for event in collab.poll() {
            match event {
                Incoming::Attachment { name, bytes } => {
                    assert_eq!(name, file);
                    return Some(bytes);
                }
                Incoming::AttachmentMissing { name } => {
                    panic!("{name} is not in the library")
                }
                Incoming::Offline(reason) => panic!("went offline: {reason}"),
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

fn wait_for_missing(collab: &mut Collab, file: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        for event in collab.poll() {
            if let Incoming::AttachmentMissing { name } = event {
                assert_eq!(name, file);
                return true;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

fn wait_for_added(collab: &mut Collab, file: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        for event in collab.poll() {
            if let Incoming::AttachmentAdded { name } = event {
                if name == file {
                    return true;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}
