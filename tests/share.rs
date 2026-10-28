//! Sharing a note by link, end to end on this one computer: a host, a guest who can edit and a
//! guest who can only watch, joined through the same door a person on another network would use.

#![cfg(feature = "share")]

use elora_notes::live::{LiveNote, LiveSnapshot};
use elora_notes::share::{join, Access, Network, ShareHost, Ticket};
use std::time::{Duration, Instant};

fn key(name: &str) -> String {
    format!("{name}-0123456789abcdef0123456789abcdef")
}

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
        let deadline = Instant::now() + Duration::from_secs(15);
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
fn test_a_link_is_read_back_and_junk_is_refused() {
    let host = ShareHost::start("Plan", "words", Network::Loopback).unwrap();
    let edit = Ticket::from_link(host.link(Access::Edit)).unwrap();
    let view = Ticket::from_link(host.link(Access::View)).unwrap();
    assert_eq!((edit.view, view.view), (false, true));
    assert_eq!(edit.title, "Plan");
    assert_ne!(
        edit.pw, view.pw,
        "watching and editing have different secrets"
    );
    assert_eq!(edit.fp, view.fp);
    // The web form carries the same ticket, and so do links inside a sentence.
    let web = host.web_link(Access::Edit);
    assert!(
        web.starts_with("https://noxilith.github.io/join/#"),
        "{web}"
    );
    assert_eq!(Ticket::from_link(web).unwrap().pw, edit.pw);
    let sentence = format!("Join my note: {} (it is live!)", host.link(Access::View));
    assert_eq!(Ticket::from_link(&sentence).unwrap().pw, view.pw);
    let found = elora_notes::share::find_link(&sentence).unwrap();
    assert_eq!(found, host.link(Access::View));
    assert_eq!(
        elora_notes::share::find_link(&format!("{web}.")).unwrap(),
        host.link(Access::Edit)
    );
    assert!(elora_notes::share::find_link("no link here").is_none());
    assert!(elora_notes::share::find_link("elora://share/not-a-ticket").is_none());
    // Spaces and line breaks around a pasted link do not matter.
    let wrapped = format!("  {}\n", host.link(Access::Edit));
    assert!(Ticket::from_link(&wrapped).is_ok());
    for junk in [
        "",
        "hello",
        "elora://share/",
        "elora://share/!!!!",
        "https://example.com",
    ] {
        assert!(Ticket::from_link(junk).is_err(), "{junk}");
    }
}

#[test]
fn test_people_edit_and_watch_a_note_shared_by_link() {
    let host = ShareHost::start("Game plan", "the first idea", Network::Loopback).unwrap();
    let mine = LiveNote::open(
        host.local_profile(),
        "host".into(),
        key("host"),
        host.note_id().to_string(),
    );
    let guest = join(host.link(Access::Edit), Network::Loopback).unwrap();
    let watcher = join(host.link(Access::View), Network::Loopback).unwrap();
    assert!(!guest.view_only && watcher.view_only);
    assert_eq!(guest.title, "Game plan");
    let editing = LiveNote::open(
        guest.profile.clone(),
        "guest".into(),
        key("guest"),
        guest.note_id.clone(),
    );
    let watching = LiveNote::open(
        watcher.profile.clone(),
        "watcher".into(),
        key("watcher"),
        watcher.note_id.clone(),
    );
    let (mut h, mut g, mut w) = (
        Watcher::new(&mine),
        Watcher::new(&editing),
        Watcher::new(&watching),
    );

    // Everybody gets the note as it was when it was shared.
    for (who, watcher) in [("host", &mut h), ("guest", &mut g), ("watcher", &mut w)] {
        watcher.until(&format!("{who}'s text"), |x| x.text.is_some());
        assert_eq!(watcher.text.as_deref(), Some("the first idea"), "{who}");
    }

    // The guest types and the host and the watcher see it, with the guest's caret and name.
    editing.set_text("the first idea, and a second");
    editing.set_cursor(8);
    h.until("the host to see the guest", |x| {
        x.text.as_deref() == Some("the first idea, and a second")
    });
    w.until("the watcher to see the guest", |x| {
        x.text.as_deref() == Some("the first idea, and a second")
    });
    w.until("the watcher to see the guest's caret", |x| {
        x.last
            .peers
            .iter()
            .any(|p| p.name == "guest" && p.cursor == Some(8))
    });

    // The host types back, and both guests see it.
    mine.set_text("Idea: the first idea, and a second");
    g.until("the guest to see the host", |x| {
        x.text.as_deref() == Some("Idea: the first idea, and a second")
    });
    w.until("the watcher to see the host", |x| {
        x.text.as_deref() == Some("Idea: the first idea, and a second")
    });

    // The watcher's typing goes nowhere.
    watching.set_text("Idea: the first idea, and a second -- vandalism");
    let quiet = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < quiet {
        for look in [mine.poll(), editing.poll()] {
            assert!(
                look.text
                    .as_deref()
                    .is_none_or(|t| !t.contains("vandalism")),
                "a watcher changed the note"
            );
        }
        std::thread::sleep(Duration::from_millis(30));
    }

    // When the host stops sharing, the guests are told they are cut off.
    drop(host);
    g.until("the guest to be cut off", |x| !x.last.connected);
}
