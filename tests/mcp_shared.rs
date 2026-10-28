//! `elora --mcp` against a real shared library: a real `elora-server` on a loopback port, and the
//! MCP tools reaching its notes through the server settings kept in the vault.

use elora_notes::collab::ServerProfile;
use elora_notes::mcp::{Vault, VaultOptions};
use elora_notes::net::ServerConfig;
use elora_notes::people;
use elora_notes::server::Server;
use elora_notes::storage::{AppSettings, NoteStorage};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn key(name: &str) -> String {
    format!("{name}-0123456789abcdef0123456789abcdef")
}

struct Harness {
    dir: std::path::PathBuf,
    profile: ServerProfile,
    server_notes: std::path::PathBuf,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A server on a free loopback port holding one note owned by alice.
fn start_server() -> Harness {
    let dir = std::env::temp_dir().join(format!(
        "elora-mcp-shared-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    // SAFETY: this file holds a single test, so nothing else reads the environment meanwhile.
    unsafe { std::env::set_var("ELORA_SERVER_DIR", dir.join("server")) };
    let server_notes = dir.join("server-notes");
    std::fs::create_dir_all(&server_notes).unwrap();
    let owner_key = people::public_key(&key("alice"));
    std::fs::write(
        server_notes.join("shared-0001.md"),
        format!(
            "---\ntitle: \"Shared\"\npinned: false\ntrashed: false\ntags: []\n\
             owner: \"alice\"\nowner_key: \"{owner_key}\"\n\
             created_at: 2026-01-01T00:00:00+00:00\n\
             updated_at: 2026-01-01T00:00:00+00:00\n---\nhello"
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
        notes_dir: server_notes.clone(),
        bind: "127.0.0.1".into(),
        ..ServerConfig::default()
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
        server_notes,
    }
}

fn call(vault: &Vault, tool: &str, args: Value) -> Value {
    let text = vault
        .call(tool, &args)
        .unwrap_or_else(|why| panic!("{tool} failed: {why}"));
    serde_json::from_str(&text).unwrap()
}

fn refused(vault: &Vault, tool: &str, args: Value) -> String {
    vault
        .call(tool, &args)
        .expect_err("the call should have been refused")
}

/// A vault folder whose settings point at the server, as the app leaves them.
fn vault_for(harness: &Harness, name: &str, read_only_ids: &[&str]) -> Vault {
    let dir = harness.dir.join(format!("vault-{name}"));
    let storage = NoteStorage::new(dir).unwrap();
    storage.save_settings(&AppSettings {
        server: harness.profile.clone(),
        display_name: name.into(),
        identifier: key(name),
        mcp_read_only_ids: read_only_ids.iter().map(|id| id.to_string()).collect(),
        ..AppSettings::default()
    });
    Vault::with_options(storage, VaultOptions::default())
}

#[test]
fn test_the_mcp_tools_work_on_shared_library_notes() {
    let harness = start_server();
    let vault = vault_for(&harness, "alice", &["locked-by-id"]);

    // Listing: the library's note is there, marked shared, without having read its text.
    let info = call(&vault, "vault_info", json!({}));
    assert!(
        info["shared_library"]
            .as_str()
            .is_some_and(|s| s.starts_with("connected to Test Vault")),
        "{info}"
    );
    let listed = call(&vault, "list_notes", json!({}));
    let row = &listed["notes"][0];
    assert_eq!(row["title"], "Shared");
    assert_eq!(row["shared"], true);
    assert!(row["path"].is_null());

    // Reading fetches the text from the room.
    let read = call(&vault, "read_note", json!({ "note": "Shared" }));
    assert_eq!(read["body"], "hello");

    // Writing changes the room, and a fresh read sees it.
    call(
        &vault,
        "update_note",
        json!({ "note": "Shared", "append": "from an assistant" }),
    );
    let again = call(&vault, "read_note", json!({ "note": "Shared" }));
    assert_eq!(again["body"], "hello\n\nfrom an assistant");
    call(
        &vault,
        "update_note",
        json!({ "note": "Shared", "prepend": "top" }),
    );
    assert_eq!(
        call(&vault, "read_note", json!({ "note": "Shared" }))["body"],
        "top\n\nhello\n\nfrom an assistant"
    );

    // Replacing the whole text leaves the new text and nothing of the old: an assistant that
    // rewrites a note must not find its rewrite stacked on top of the version it replaced.
    let mut text = String::from("# Progress\n\n");
    for step in 0..400 {
        text.push_str(&format!("- [x] step {step} is built and tested\n"));
    }
    for round in 0..3 {
        let rewritten = text.replace("step 7 ", &format!("step 7 (round {round}) "));
        call(
            &vault,
            "update_note",
            json!({ "note": "Shared", "body": rewritten }),
        );
        assert_eq!(
            call(&vault, "read_note", json!({ "note": "Shared" }))["body"],
            rewritten,
            "round {round}"
        );
    }

    // Properties go through the server's own update.
    let set = call(
        &vault,
        "set_note_properties",
        json!({ "note": "Shared", "tags": ["team"], "pinned": true, "icon": "flare" }),
    );
    assert_eq!(set["updated"]["tags"], json!(["team"]));
    assert_eq!(set["updated"]["pinned"], true);
    assert_eq!(set["updated"]["icon"], "flare");

    // A note can be made in the library, found, renamed, trashed and deleted there.
    let made = call(
        &vault,
        "create_note",
        json!({ "title": "From MCP", "body": "created remotely", "shared": true, "tags": ["ai"] }),
    );
    assert_eq!(made["created"]["shared"], true);
    assert_eq!(made["created"]["icon"], "robot");
    let found = call(&vault, "search_notes", json!({ "query": "From MCP" }));
    assert_eq!(found["results"][0]["shared"], true);
    assert_eq!(
        call(&vault, "read_note", json!({ "note": "From MCP" }))["body"],
        "created remotely"
    );
    call(
        &vault,
        "rename_note",
        json!({ "note": "From MCP", "new_title": "Renamed remotely" }),
    );
    assert_eq!(
        call(&vault, "read_note", json!({ "note": "Renamed remotely" }))["title"],
        "Renamed remotely"
    );
    call(&vault, "trash_note", json!({ "note": "Renamed remotely" }));
    let trash = call(&vault, "list_notes", json!({ "trashed": true }));
    assert_eq!(trash["notes"][0]["title"], "Renamed remotely");
    call(
        &vault,
        "restore_note",
        json!({ "note": "Renamed remotely" }),
    );
    call(&vault, "trash_note", json!({ "note": "Renamed remotely" }));
    call(
        &vault,
        "delete_note",
        json!({ "note": "Renamed remotely", "confirm": true }),
    );
    let left = call(&vault, "list_notes", json!({ "trashed": true }));
    assert_eq!(left["count"], 0);

    // The server really holds the change, not just this client's view of it.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut on_disk = String::new();
    while Instant::now() < deadline {
        on_disk = std::fs::read_to_string(harness.server_notes.join("shared-0001.md"))
            .unwrap_or_default();
        if on_disk.contains("(round 2)") {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(on_disk.contains("(round 2)"), "{on_disk}");
    assert!(!on_disk.contains("from an assistant"), "{on_disk}");
    assert!(on_disk.contains("pinned: true"), "{on_disk}");

    // A second person's assistant sees the same library, including what the first one wrote.
    let bob = vault_for(&harness, "bob", &[]);
    let seen = call(&bob, "read_note", json!({ "note": "Shared" }));
    assert!(seen["body"].as_str().unwrap().contains("(round 2)"));

    // A note marked read-only for assistants in the settings stays untouched.
    let guarded = vault_for(&harness, "alice2", &["shared-0001"]);
    let why = refused(
        &guarded,
        "update_note",
        json!({ "note": "Shared", "append": "no" }),
    );
    assert!(why.contains("marked read-only"), "{why}");
    let info = call(&guarded, "read_note", json!({ "note": "Shared" }));
    assert_eq!(info["read_only"], true);
}

/// A library that is set up but cannot be reached must not take the local notes down with it:
/// the tools still work, and say why the shared notes are missing.
#[test]
fn test_an_unreachable_library_leaves_the_local_notes_working() {
    let dir = std::env::temp_dir().join(format!(
        "elora-mcp-offline-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let closed_port = probe.local_addr().unwrap().port();
    drop(probe);
    let storage = NoteStorage::new(dir.clone()).unwrap();
    storage.save_settings(&AppSettings {
        server: ServerProfile {
            host: "127.0.0.1".into(),
            port: closed_port,
            password: "letmein".into(),
            fingerprint: Some("00".into()),
        },
        display_name: "alice".into(),
        identifier: key("alice"),
        ..AppSettings::default()
    });
    let vault = Vault::new(storage);

    let made = call(&vault, "create_note", json!({ "title": "Local only" }));
    assert_eq!(made["created"]["shared"], false);
    let listed = call(&vault, "list_notes", json!({}));
    assert_eq!(listed["count"], 1);
    assert!(listed["shared_library_unavailable"].is_string(), "{listed}");
    // Asking for the shared library itself says so plainly.
    let why = refused(
        &vault,
        "create_note",
        json!({ "title": "Remote", "shared": true }),
    );
    assert!(!why.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
