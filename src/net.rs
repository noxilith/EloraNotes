//! Wire protocol shared by the EloraNotes client and `elora-server`.
//!
//! The connection is a TLS WebSocket. JSON text frames carry the library conversation
//! (handshake, note metadata, presence); binary frames carry Yjs payloads for whichever
//! note the connection currently has open, prefixed with a one-byte kind.

use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::path::PathBuf;

/// A frontmatter value a client may change, leave alone, or clear.
///
/// The obvious `Option<Option<T>>` cannot say "clear it": serde turns both `Keep` and a cleared
/// value into `null`, so a folder could be set but never taken off again. This type keeps the
/// three cases apart while still putting a plain value or `null` on the wire, which is what the
/// protocol has always sent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Field<T> {
    /// The message does not mention it.
    #[default]
    Keep,
    /// Set it to nothing.
    Clear,
    Set(T),
}

impl<T> Field<T> {
    pub fn is_keep(&self) -> bool {
        matches!(self, Field::Keep)
    }
}

impl<T: Serialize> Serialize for Field<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Field::Keep | Field::Clear => serializer.serialize_none(),
            Field::Set(value) => value.serialize(serializer),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Field<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match Option::<T>::deserialize(deserializer)? {
            Some(value) => Field::Set(value),
            None => Field::Clear,
        })
    }
}

/// Bumped whenever a message changes shape in a way an older peer cannot read.
/// Version 2 tags every Yjs sync frame with the note it belongs to.
/// Version 3 adds the frames that carry a shared note's pictures. A version-2 server has no
/// handler for them and drops them without a word, so a client that can share pictures must
/// not pair with one: the picture would quietly never reach the library, which is why this
/// went to 3 rather than staying at 2.
/// Version 4 adds the identifier each client connects with, and the per-note answer to who may
/// see it. A version-3 server does not know either, so it would list every note to everybody
/// and accept changes from people the note was restricted away from — the opposite of what the
/// setting promises. Nothing may pair across that line.
pub const PROTOCOL_VERSION: u32 = 4;

/// Prefixes a Yjs sync payload with the id of the note it belongs to.
///
/// A connection has one note open at a time, but frames already on the wire when the note is
/// switched arrive afterwards. Without the id they look exactly like the new note's state, and a
/// stale frame was enough to put the note just left into the one being opened.
pub fn encode_sync(note_id: &str, message: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + note_id.len() + message.len());
    out.extend_from_slice(&(note_id.len() as u16).to_be_bytes());
    out.extend_from_slice(note_id.as_bytes());
    out.extend_from_slice(message);
    out
}

/// An attachment frame: `[op][name len: u16 big endian][name][bytes…]`.
pub fn encode_attachment(op: u8, name: &str, bytes: &[u8]) -> Vec<u8> {
    let name = name.as_bytes();
    let mut out = Vec::with_capacity(3 + name.len() + bytes.len());
    out.push(op);
    out.extend_from_slice(&(name.len() as u16).to_be_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(bytes);
    out
}

/// Splits an attachment frame back into its op, name and bytes.
pub fn decode_attachment(payload: &[u8]) -> Option<(u8, &str, &[u8])> {
    let op = *payload.first()?;
    let len = u16::from_be_bytes([*payload.get(1)?, *payload.get(2)?]) as usize;
    let name = payload.get(3..3 + len)?;
    Some((op, std::str::from_utf8(name).ok()?, payload.get(3 + len..)?))
}

/// A picture's name inside the library's `attachments` folder: one plain file name, one of the
/// formats the preview can draw, and nothing that could point outside the folder.
pub fn valid_attachment_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 96
        && !name.starts_with('.')
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        && name.rsplit_once('.').is_some_and(|(_, extension)| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif"
            )
        })
}

/// Splits a sync payload back into its note id and the Yjs message.
pub fn decode_sync(payload: &[u8]) -> Option<(&str, &[u8])> {
    let len = u16::from_be_bytes([*payload.first()?, *payload.get(1)?]) as usize;
    let id = payload.get(2..2 + len)?;
    let message = payload.get(2 + len..)?;
    Some((std::str::from_utf8(id).ok()?, message))
}

/// Frame kinds. Every frame is `[kind][len: u32 big endian][payload]`.
pub const FRAME_JSON: u8 = 0;
pub const FRAME_SYNC: u8 = 1;
pub const FRAME_AWARENESS: u8 = 2;
/// Pictures. A shared note names one with `![alt](attachments/…)`, and the bytes live on the
/// server so everyone with the library can see it and it outlives the computer that added it.
pub const FRAME_ATTACHMENT: u8 = 3;

/// What an attachment frame asks for or answers.
pub const ATTACHMENT_SEND: u8 = 0;
pub const ATTACHMENT_ASK: u8 = 1;
pub const ATTACHMENT_DATA: u8 = 2;
pub const ATTACHMENT_MISSING: u8 = 3;

/// The largest picture the library moves in one frame. Comfortably above a phone photo, and
/// below the frame limit so one picture always fits.
pub const MAX_ATTACHMENT: usize = 12 * 1024 * 1024;

/// Largest frame accepted from a peer, so a bad length cannot make the other side allocate
/// gigabytes. One attachment frame has to fit inside it.
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

/// Writes one frame; the caller flushes.
pub fn write_frame<W: std::io::Write>(
    out: &mut W,
    kind: u8,
    payload: &[u8],
) -> std::io::Result<()> {
    out.write_all(&[kind])?;
    out.write_all(&(payload.len() as u32).to_be_bytes())?;
    out.write_all(payload)
}

/// Reads whatever is available into `buffer` and returns every complete frame it now holds.
/// Incomplete bytes stay in the buffer for the next call.
pub fn take_frames(buffer: &mut Vec<u8>) -> Vec<(u8, Vec<u8>)> {
    let mut frames = Vec::new();
    let mut offset = 0;
    while buffer.len() >= offset + 5 {
        let kind = buffer[offset];
        let len = u32::from_be_bytes([
            buffer[offset + 1],
            buffer[offset + 2],
            buffer[offset + 3],
            buffer[offset + 4],
        ]) as usize;
        if len > MAX_FRAME {
            // Corrupt or hostile length: drop the buffer instead of waiting forever.
            buffer.clear();
            return frames;
        }
        if buffer.len() < offset + 5 + len {
            break;
        }
        frames.push((kind, buffer[offset + 5..offset + 5 + len].to_vec()));
        offset += 5 + len;
    }
    if offset > 0 {
        buffer.drain(..offset);
    }
    frames
}

/// The name people see next to your caret and in the presence list: the account this machine
/// is logged into, which is what the user asked for.
pub fn os_username() -> String {
    for key in ["ELORA_USER", "USER", "LOGNAME", "USERNAME"] {
        if let Ok(value) = std::env::var(key) {
            let value = value.trim();
            if !value.is_empty() {
                return value.to_string();
            }
        }
    }
    "guest".to_string()
}

/// Trims an identifier to what can go into a note's frontmatter and be compared with another:
/// no surrounding space, nothing on a second line, and a sane length.
pub fn normalize_identifier(raw: &str) -> String {
    // Commas and brackets are left out: a note's frontmatter keeps its list of identifiers on
    // one `shared_with: [a, b]` line, and a name containing either would split it.
    let cleaned: String = raw
        .trim()
        .chars()
        .filter(|ch| !ch.is_control() && !matches!(ch, ',' | '[' | ']'))
        .take(64)
        .collect();
    cleaned.trim().to_string()
}

/// Whether a note is for this person: everything public is for everybody, and a restricted note
/// is only for its owner and the people it names. Identifiers are compared without case, so a
/// name typed with a capital in one place and not the other still grants access.
pub fn visible_to(meta: &NoteMeta, who: &str) -> bool {
    if !meta.private {
        return true;
    }
    let who = who.trim();
    if who.is_empty() {
        return false;
    }
    meta.owner
        .as_deref()
        .is_some_and(|owner| owner.eq_ignore_ascii_case(who))
        || meta
            .shared_with
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(who))
}

/// Notes the server owns, as the sidebar and the editor need them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NoteMeta {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub trashed: bool,
    #[serde(default)]
    pub icon: Option<String>,
    /// Unix seconds; the sort order in the sidebar.
    #[serde(default)]
    pub updated_at: i64,
    /// The identifier of the person the note belongs to. Recorded for every note so the same
    /// identifier on another computer finds them again, and used as the owner of a restricted
    /// note.
    #[serde(default)]
    pub owner: Option<String>,
    /// True when only the owner and the people in `shared_with` may see the note.
    #[serde(default)]
    pub private: bool,
    /// Identifiers the owner has let in to a private note.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared_with: Vec<String>,
}

/// Who is in a note right now.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PresenceUser {
    pub name: String,
    /// RGB used for this person's caret and dot, so the same person keeps the same colour.
    pub color: [u8; 3],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    Hello {
        v: u32,
        name: String,
        /// Who this client is: the identifier from its settings. It decides which restricted
        /// notes it is shown and which it may change.
        id: String,
        password: String,
    },
    /// Restricts a note to its owner and the identifiers listed, or opens it to the whole
    /// server again. Only the note's owner may send this.
    SetShare {
        id: String,
        private: bool,
        #[serde(default)]
        shared_with: Vec<String>,
    },
    /// Start receiving Yjs payloads for one note (switching rooms is allowed at any time).
    Open {
        id: String,
    },
    /// Leave the current room but stay connected to the library.
    Close,
    /// Create a note in the shared library. Folder, tags and body are optional, so a client that
    /// only knows a title still works against this server.
    Create {
        title: String,
        #[serde(default)]
        folder: Option<String>,
        #[serde(default)]
        tags: Vec<String>,
        #[serde(default)]
        body: String,
        #[serde(default)]
        pinned: bool,
        #[serde(default)]
        icon: Option<String>,
    },
    /// Update the fields that live in the note's frontmatter. `None` means "leave as is";
    /// `Field` is used where a value can also be cleared.
    Update {
        id: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Field::is_keep")]
        folder: Field<String>,
        #[serde(default)]
        tags: Option<Vec<String>>,
        #[serde(default)]
        pinned: Option<bool>,
        #[serde(default)]
        trashed: Option<bool>,
        #[serde(default, skip_serializing_if = "Field::is_keep")]
        icon: Field<String>,
    },
    /// Move a note to the trash (`permanent: false`) or delete it outright.
    Delete {
        id: String,
        permanent: bool,
    },
    Ping,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Welcome {
        v: u32,
        server: String,
        you: String,
        notes: Vec<NoteMeta>,
    },
    Error {
        message: String,
    },
    /// The whole library after a change, so a client never has to merge metadata itself.
    Notes {
        notes: Vec<NoteMeta>,
    },
    /// Sent to the client that asked for a new note, so it can open it straight away.
    Created {
        note: NoteMeta,
    },
    /// A picture has joined the library, so a client that asked for it before the upload
    /// finished can ask again instead of waiting for the note to be reopened.
    Attachment {
        name: String,
    },
    /// Who is editing which note, so the sidebar can show a dot on the card.
    Presence {
        /// note id -> names of everyone with it open
        notes: Vec<(String, Vec<PresenceUser>)>,
    },
    Pong,
}

impl ClientMsg {
    pub fn to_text(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

impl ServerMsg {
    pub fn to_text(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// What the installer asks for and the server needs to run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    /// Shown in the client's sidebar as the group title.
    pub name: String,
    pub port: u16,
    pub password: String,
    /// Folder holding the server's `.md` notes.
    pub notes_dir: PathBuf,
    /// Bind address; `0.0.0.0` accepts connections from other machines.
    #[serde(default = "default_bind")]
    pub bind: String,
}

fn default_bind() -> String {
    "0.0.0.0".to_string()
}

impl ServerConfig {
    /// Where the server keeps its config, certificate and key.
    pub fn data_dir() -> PathBuf {
        if let Ok(custom) = std::env::var("ELORA_SERVER_DIR") {
            let p = PathBuf::from(custom);
            if !p.as_os_str().is_empty() {
                return p;
            }
        }
        let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        base.join("elora-server")
    }

    pub fn config_path() -> PathBuf {
        Self::data_dir().join("config.json")
    }

    pub fn load() -> Result<Self, String> {
        let path = Self::config_path();
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("could not read {}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{} is not valid: {e}", path.display()))
    }

    pub fn save(&self) -> Result<(), String> {
        let dir = Self::data_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
        let path = Self::config_path();
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, text).map_err(|e| format!("could not write {}: {e}", path.display()))
    }

    /// Cert and key live beside the config so a re-install keeps the fingerprint clients pinned.
    pub fn cert_path(&self) -> PathBuf {
        Self::data_dir().join("cert.pem")
    }

    pub fn key_path(&self) -> PathBuf {
        Self::data_dir().join("key.pem")
    }
}

/// A short, human-checkable form of the certificate fingerprint (first four bytes).
pub fn short_fingerprint(fingerprint: &str) -> String {
    fingerprint
        .chars()
        .take(8)
        .collect::<Vec<_>>()
        .chunks(4)
        .map(|chunk| chunk.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("-")
        .to_uppercase()
}

/// Addresses this machine answers on, for the "how do others connect" hint.
pub fn local_ipv4_addresses() -> Vec<IpAddr> {
    let mut found = Vec::new();
    // A UDP socket connect() does not send anything; it just asks the routing table which
    // local address would reach the outside world.
    if let Ok(socket) = std::net::UdpSocket::bind("0.0.0.0:0") {
        if socket.connect("1.1.1.1:80").is_ok() {
            if let Ok(addr) = socket.local_addr() {
                found.push(addr.ip());
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frames_split_across_reads() {
        let mut wire = Vec::new();
        write_frame(&mut wire, FRAME_JSON, b"{\"type\":\"ping\"}").unwrap();
        write_frame(&mut wire, FRAME_SYNC, b"abc").unwrap();

        let mut buffer = Vec::new();
        // Half a frame at a time, like a slow network would deliver it.
        let half = wire.len() / 2;
        buffer.extend_from_slice(&wire[..half]);
        assert!(take_frames(&mut buffer).is_empty());
        buffer.extend_from_slice(&wire[half..]);
        let frames = take_frames(&mut buffer);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].0, FRAME_JSON);
        assert_eq!(frames[0].1, b"{\"type\":\"ping\"}");
        assert_eq!(frames[1].0, FRAME_SYNC);
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_attachment_frames_carry_a_name_and_its_bytes() {
        let frame = encode_attachment(ATTACHMENT_DATA, "holiday-1a2b3c4d.png", b"\x89PNG bytes");
        assert_eq!(
            decode_attachment(&frame),
            Some((
                ATTACHMENT_DATA,
                "holiday-1a2b3c4d.png",
                &b"\x89PNG bytes"[..]
            ))
        );
        // An empty picture (the answer to a successful upload) is still a frame with its name.
        let ack = encode_attachment(ATTACHMENT_DATA, "a.png", b"");
        assert_eq!(
            decode_attachment(&ack),
            Some((ATTACHMENT_DATA, "a.png", &b""[..]))
        );
        assert_eq!(decode_attachment(b"\x02"), None);
    }

    #[test]
    fn test_attachment_names_stay_inside_the_folder() {
        assert!(valid_attachment_name("holiday-1a2b3c4d.png"));
        assert!(valid_attachment_name("My_Cat.JPEG"));
        // Nothing that could name a file somewhere else, hide it, or point at another type.
        assert!(!valid_attachment_name("../secrets.png"));
        assert!(!valid_attachment_name("sub/dir/cat.png"));
        assert!(!valid_attachment_name("/etc/passwd"));
        assert!(!valid_attachment_name(".hidden.png"));
        assert!(!valid_attachment_name("notes.md"));
        assert!(!valid_attachment_name("cat"));
        assert!(!valid_attachment_name(""));
        assert!(!valid_attachment_name(&format!("{}.png", "a".repeat(100))));
    }

    #[test]
    fn test_short_fingerprint_groups_bytes() {
        assert_eq!(short_fingerprint("a1b2c3d4e5f6"), "A1B2-C3D4");
        assert_eq!(short_fingerprint("ab"), "AB");
    }

    #[test]
    fn test_client_message_round_trips_as_json() {
        let msg = ClientMsg::Hello {
            v: PROTOCOL_VERSION,
            name: "nox".into(),
            id: "SuperSaiyan".into(),
            password: "hunter2".into(),
        };
        let text = msg.to_text().unwrap();
        assert!(text.contains("\"type\":\"hello\""));
        assert!(text.contains("\"id\":\"SuperSaiyan\""));
        let back: ClientMsg = serde_json::from_str(&text).unwrap();
        assert!(matches!(back, ClientMsg::Hello { .. }));
    }

    fn meta(owner: Option<&str>, private: bool, shared: &[&str]) -> NoteMeta {
        NoteMeta {
            id: "n-1".into(),
            title: "Note".into(),
            folder: None,
            tags: Vec::new(),
            pinned: false,
            trashed: false,
            icon: None,
            updated_at: 0,
            owner: owner.map(str::to_string),
            private,
            shared_with: shared.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn test_a_public_note_is_for_everyone_and_a_restricted_one_is_not() {
        let open = meta(Some("nox"), false, &[]);
        assert!(visible_to(&open, "nox"));
        assert!(visible_to(&open, "friend"));

        // "Only me" means exactly that, whatever the case of the name typed.
        let mine = meta(Some("Nox"), true, &[]);
        assert!(visible_to(&mine, "nox"), "the owner keeps the note");
        assert!(!visible_to(&mine, "friend"));

        // Naming somebody lets them in, and only them.
        let shared = meta(Some("nox"), true, &["SuperSaiyan", "Mira"]);
        assert!(visible_to(&shared, "supersaiyan"));
        assert!(visible_to(&shared, "MIRA"));
        assert!(!visible_to(&shared, "  "));
        assert!(!visible_to(&shared, "stranger"));

        // A restricted note whose owner is unknown is not left open by accident.
        let orphan = meta(None, true, &[]);
        assert!(!visible_to(&orphan, "nox"));
    }

    #[test]
    fn test_identifiers_keep_one_line_and_a_sane_length() {
        assert_eq!(normalize_identifier("  SuperSaiyan \n"), "SuperSaiyan");
        assert_eq!(normalize_identifier("a\u{0}b"), "ab");
        assert_eq!(normalize_identifier("nox, [mira]"), "nox mira");
        assert_eq!(normalize_identifier("   "), "");
        assert_eq!(normalize_identifier(&"a".repeat(200)).len(), 64);
    }

    #[test]
    fn test_share_message_travels_with_its_list() {
        let msg = ClientMsg::SetShare {
            id: "n-1".into(),
            private: true,
            shared_with: vec!["SuperSaiyan".into()],
        };
        let text = msg.to_text().unwrap();
        assert!(text.contains("\"type\":\"set_share\""), "{text}");
        let back: ClientMsg = serde_json::from_str(&text).unwrap();
        assert!(matches!(
            back,
            ClientMsg::SetShare {
                private: true,
                ref shared_with,
                ..
            } if shared_with == &["SuperSaiyan".to_string()]
        ));
        // A note opened to everyone carries an empty list rather than being unreadable.
        let open: ClientMsg =
            serde_json::from_str("{\"type\":\"set_share\",\"id\":\"n\",\"private\":false}")
                .unwrap();
        assert!(matches!(
            open,
            ClientMsg::SetShare {
                private: false,
                ref shared_with,
                ..
            } if shared_with.is_empty()
        ));
    }

    #[test]
    fn test_server_config_round_trips() {
        let config = ServerConfig {
            name: "Study".into(),
            port: 7331,
            password: "pw".into(),
            notes_dir: PathBuf::from("/tmp/notes"),
            bind: "0.0.0.0".into(),
        };
        let text = serde_json::to_string(&config).unwrap();
        let back: ServerConfig = serde_json::from_str(&text).unwrap();
        assert_eq!(back.name, "Study");
        assert_eq!(back.port, 7331);
    }

    #[test]
    fn test_username_falls_back_to_non_empty() {
        assert!(!os_username().is_empty());
    }

    #[test]
    fn test_sync_frames_carry_their_note_id() {
        let payload = encode_sync("untitled-a1b2", b"yjs bytes");
        assert_eq!(
            decode_sync(&payload),
            Some(("untitled-a1b2", &b"yjs bytes"[..]))
        );
        // A long id still leaves the message intact.
        let long = "n".repeat(200);
        let payload = encode_sync(&long, b"x");
        assert_eq!(decode_sync(&payload), Some((long.as_str(), &b"x"[..])));
        // Truncated or empty payloads are refused rather than misread.
        assert_eq!(decode_sync(&[]), None);
        assert_eq!(decode_sync(&[0, 9, b'a']), None);
    }

    #[test]
    fn test_field_tells_keep_clear_and_set_apart() {
        #[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
        struct Update {
            #[serde(default, skip_serializing_if = "Field::is_keep")]
            folder: Field<String>,
        }

        assert_eq!(serde_json::to_string(&Update::default()).unwrap(), "{}");
        assert_eq!(
            serde_json::to_string(&Update {
                folder: Field::Clear
            })
            .unwrap(),
            "{\"folder\":null}"
        );
        assert_eq!(
            serde_json::to_string(&Update {
                folder: Field::Set("Team".to_string())
            })
            .unwrap(),
            "{\"folder\":\"Team\"}"
        );

        let keep: Update = serde_json::from_str("{}").unwrap();
        assert_eq!(keep.folder, Field::Keep);
        let clear: Update = serde_json::from_str("{\"folder\":null}").unwrap();
        assert_eq!(clear.folder, Field::Clear);
        let set: Update = serde_json::from_str("{\"folder\":\"Team\"}").unwrap();
        assert_eq!(set.folder, Field::Set("Team".to_string()));
    }
}
