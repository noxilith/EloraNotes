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
pub const PROTOCOL_VERSION: u32 = 2;

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

/// Largest frame accepted from a peer, so a bad length cannot make the other side allocate
/// gigabytes.
pub const MAX_FRAME: usize = 8 * 1024 * 1024;

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
        password: String,
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
    fn test_short_fingerprint_groups_bytes() {
        assert_eq!(short_fingerprint("a1b2c3d4e5f6"), "A1B2-C3D4");
        assert_eq!(short_fingerprint("ab"), "AB");
    }

    #[test]
    fn test_client_message_round_trips_as_json() {
        let msg = ClientMsg::Hello {
            v: PROTOCOL_VERSION,
            name: "nox".into(),
            password: "hunter2".into(),
        };
        let text = msg.to_text().unwrap();
        assert!(text.contains("\"type\":\"hello\""));
        let back: ClientMsg = serde_json::from_str(&text).unwrap();
        assert!(matches!(back, ClientMsg::Hello { .. }));
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
