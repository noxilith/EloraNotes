//! Sharing one note by link, with nobody running a server.
//!
//! The person sharing starts a small temporary server that holds just that note, and opens a
//! door to it that people with the link can reach directly, from anywhere (the connections are
//! made between the devices, helped through firewalls by public relays that only ever see
//! encrypted traffic). The link carries everything a guest needs: where to connect, a secret
//! password, and the certificate fingerprint to trust. There are two passwords: one that lets
//! somebody edit, and one that lets them only watch.
//!
//! Guests and the host then use the same live-editing code as a shared library: text, carets and
//! names all work the same way, because it is the same server and the same client underneath.

use crate::collab::ServerProfile;
use crate::net::ServerConfig;
use crate::server::Server;
use base64::Engine;
use iroh::{endpoint::presets, Endpoint, EndpointAddr, EndpointId, RelayUrl, TransportAddr};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// The name the two ends use to agree they are speaking this protocol.
const ALPN: &[u8] = b"elora-notes/share/1";
const LINK_PREFIX: &str = "elora://share/";
/// The page that opens a link in the app, for places where `elora://` cannot be clicked.
const WEB_PREFIX: &str = "https://noxilith.github.io/join/";

/// How devices find each other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Network {
    /// Across the internet, with public relays to get through firewalls.
    Internet,
    /// Only this computer: for tests that run with no network at all.
    Loopback,
}

/// What a link lets somebody do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Edit,
    View,
}

/// Everything in a share link.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ticket {
    pub v: u8,
    /// Where the host can be reached.
    pub addr: EndpointAddr,
    /// The certificate to trust, hex.
    pub fp: String,
    pub note: String,
    pub title: String,
    pub pw: String,
    /// True for a link that only lets somebody watch.
    pub view: bool,
}

impl Ticket {
    /// The link in the app's own form. The ticket is packed into bytes (about a quarter of the
    /// size of its JSON) whenever it can be, which is always for tickets this app makes.
    pub fn to_link(&self) -> String {
        let bytes = self
            .pack()
            .unwrap_or_else(|| serde_json::to_vec(self).unwrap_or_default());
        format!(
            "{LINK_PREFIX}{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
        )
    }

    /// The ticket as bytes: a version byte, the access, the host's key, the password and the
    /// certificate as raw bytes, the addresses, then the note's id and title. `None` for a ticket
    /// holding something the packed form cannot, which then travels as JSON.
    fn pack(&self) -> Option<Vec<u8>> {
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let password = engine.decode(&self.pw).ok().filter(|pw| pw.len() == 16)?;
        let fingerprint = hex_bytes(&self.fp).filter(|fp| fp.len() == 32)?;
        let mut out = vec![PACKED, u8::from(self.view)];
        out.extend_from_slice(self.addr.id.as_bytes());
        out.extend_from_slice(&password);
        out.extend_from_slice(&fingerprint);
        out.push(u8::try_from(self.addr.addrs.len()).ok()?);
        for addr in &self.addr.addrs {
            match addr {
                TransportAddr::Relay(url) => {
                    let url = url.to_string();
                    if url == DEFAULT_RELAY {
                        out.push(0);
                    } else {
                        out.push(1);
                        out.push(u8::try_from(url.len()).ok()?);
                        out.extend_from_slice(url.as_bytes());
                    }
                }
                TransportAddr::Ip(SocketAddr::V4(ip)) => {
                    out.push(2);
                    out.extend_from_slice(&ip.ip().octets());
                    out.extend_from_slice(&ip.port().to_be_bytes());
                }
                TransportAddr::Ip(SocketAddr::V6(ip)) => {
                    out.push(3);
                    out.extend_from_slice(&ip.ip().octets());
                    out.extend_from_slice(&ip.port().to_be_bytes());
                }
                _ => return None,
            }
        }
        for text in [&self.note, &self.title] {
            // A long title is cut: it is only a label, and the link should stay short.
            let text: String = if text.len() > 60 {
                text.chars()
                    .scan(0, |n, c| {
                        *n += c.len_utf8();
                        (*n <= 60).then_some(c)
                    })
                    .collect()
            } else {
                text.clone()
            };
            out.push(u8::try_from(text.len()).ok()?);
            out.extend_from_slice(text.as_bytes());
        }
        Some(out)
    }

    fn unpack(bytes: &[u8]) -> Option<Self> {
        let mut at = Reader { bytes, at: 0 };
        if at.take(1)? != [PACKED] {
            return None;
        }
        let view = at.take(1)?[0] != 0;
        let id = EndpointId::from_bytes(at.take(32)?.try_into().ok()?).ok()?;
        let password = at.take(16)?;
        let fingerprint = at.take(32)?;
        let mut addr = EndpointAddr::new(id);
        for _ in 0..at.take(1)?[0] {
            match at.take(1)?[0] {
                0 => addr = addr.with_relay_url(DEFAULT_RELAY.parse::<RelayUrl>().ok()?),
                1 => {
                    let length = usize::from(at.take(1)?[0]);
                    let url = std::str::from_utf8(at.take(length)?).ok()?;
                    addr = addr.with_relay_url(url.parse::<RelayUrl>().ok()?);
                }
                2 => {
                    let ip: [u8; 4] = at.take(4)?.try_into().ok()?;
                    let port = u16::from_be_bytes(at.take(2)?.try_into().ok()?);
                    addr = addr.with_ip_addr(SocketAddr::from((ip, port)));
                }
                3 => {
                    let ip: [u8; 16] = at.take(16)?.try_into().ok()?;
                    let port = u16::from_be_bytes(at.take(2)?.try_into().ok()?);
                    addr = addr.with_ip_addr(SocketAddr::from((ip, port)));
                }
                _ => return None,
            }
        }
        let mut texts = Vec::new();
        for _ in 0..2 {
            let length = usize::from(at.take(1)?[0]);
            texts.push(String::from_utf8(at.take(length)?.to_vec()).ok()?);
        }
        let title = texts.pop()?;
        let note = texts.pop()?;
        Some(Self {
            v: 1,
            addr,
            fp: fingerprint.iter().map(|b| format!("{b:02x}")).collect(),
            note,
            title,
            pw: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(password),
            view,
        })
    }

    /// The invite link: `https://noxilith.github.io/join/INVITECODE`. It works when clicked in a
    /// browser, a chat or an email: a small page on the docs site hands the code to the app. The
    /// code is part of the address, so the site's host can see it; the link is as private as the
    /// place it is sent to.
    pub fn to_web_link(&self) -> String {
        let link = self.to_link();
        format!("{WEB_PREFIX}{}", &link[LINK_PREFIX.len()..])
    }

    /// Reads a link pasted by a person. Either form works, spaces and line breaks around it do
    /// not matter, and so does the sentence it was pasted in.
    pub fn from_link(link: &str) -> Result<Self, String> {
        let body = payload_in(link).ok_or("that is not an EloraNotes share link")?;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(body)
            .map_err(|_| "that share link is damaged: copy it again")?;
        // Links made by this version are packed bytes; earlier ones are JSON.
        let ticket: Ticket = match bytes.first() {
            Some(&PACKED) => {
                Self::unpack(&bytes).ok_or("that share link is damaged: copy it again")?
            }
            _ => serde_json::from_slice(&bytes)
                .map_err(|_| "that share link is damaged: copy it again")?,
        };
        if ticket.v != 1 {
            return Err("that link is from a newer EloraNotes: update the app".to_string());
        }
        Ok(ticket)
    }
}

/// Marks a packed ticket. JSON starts with `{`, so the two can never be mistaken for each other.
const PACKED: u8 = 2;
/// The relay most links name, written as one byte instead of its address.
const DEFAULT_RELAY: &str = "https://use1-1.relay.n0.iroh.link./";

fn hex_bytes(text: &str) -> Option<Vec<u8>> {
    (text.len().is_multiple_of(2) && text.is_ascii())
        .then(|| {
            (0..text.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
                .collect()
        })
        .flatten()
}

/// Reads a packed ticket front to back.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let slice = self.bytes.get(self.at..self.at.checked_add(count)?)?;
        self.at += count;
        Some(slice)
    }
}

/// The part of a share link that carries the ticket, wherever it sits in `text`.
fn payload_in(text: &str) -> Option<&str> {
    let lower = text.to_ascii_lowercase();
    let start = if let Some(at) = lower.find(LINK_PREFIX) {
        at + LINK_PREFIX.len()
    } else {
        lower.find(WEB_PREFIX).map(|at| at + WEB_PREFIX.len())?
    };
    let rest = &text[start..];
    // Invite links made before the code moved into the path had a `#` in front of it.
    let rest = rest.strip_prefix('#').unwrap_or(rest);
    // The ticket is letters, digits, dashes and underscores; whatever follows (a full stop at the
    // end of a sentence, a bracket) is not part of it.
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
}

/// The first share link in `text`, in the form the app uses (`elora://share/…`), or `None`.
/// Used on command-line arguments and on whatever is on the clipboard.
pub fn find_link(text: &str) -> Option<String> {
    let payload = payload_in(text)?;
    let link = format!("{LINK_PREFIX}{payload}");
    Ticket::from_link(&link).ok().map(|_| link)
}

fn secret() -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(rand::random::<[u8; 16]>())
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("elora-share")
        .enable_all()
        .build()
        .map_err(|e| format!("could not start sharing: {e}"))
}

async fn bind(network: Network, accept: bool) -> Result<Endpoint, String> {
    let alpns = if accept {
        vec![ALPN.to_vec()]
    } else {
        Vec::new()
    };
    let built = match network {
        Network::Internet => Endpoint::builder(presets::N0).alpns(alpns).bind().await,
        Network::Loopback => {
            Endpoint::builder(presets::Minimal)
                .alpns(alpns)
                .bind()
                .await
        }
    };
    built.map_err(|e| format!("could not start sharing: {e}"))
}

/// A note being shared. Dropping it stops the sharing: the door closes and the temporary server
/// and its files go.
pub struct ShareHost {
    runtime: Option<tokio::runtime::Runtime>,
    endpoint: Endpoint,
    stop: Arc<AtomicBool>,
    dir: PathBuf,
    note_id: String,
    local: ServerProfile,
    edit_link: String,
    view_link: String,
    edit_web: String,
    view_web: String,
}

impl ShareHost {
    /// Starts sharing a note with this title and text. `host_name` is how the host is named on
    /// everybody's screen.
    pub fn start(title: &str, body: &str, network: Network) -> Result<Self, String> {
        let dir = std::env::temp_dir().join(format!("elora-share-{}", secret()));
        let notes = dir.join("notes");
        std::fs::create_dir_all(&notes).map_err(|e| format!("could not make a folder: {e}"))?;
        let note = crate::storage::Note::new(title, body, &notes);
        crate::storage::write_atomic(&note.file_path, note.to_markdown())
            .map_err(|e| format!("could not write the note: {e}"))?;

        let (edit_pw, view_pw) = (secret(), secret());
        let server = Arc::new(Server::new(ServerConfig {
            name: title.to_string(),
            port: 0,
            password: edit_pw.clone(),
            notes_dir: notes,
            bind: "127.0.0.1".to_string(),
            view_password: view_pw.clone(),
            identity_dir: Some(dir.join("identity")),
            open_names: true,
        })?);
        let listener = server.listen()?;
        let port = listener
            .local_addr()
            .map_err(|e| format!("could not open the door: {e}"))?
            .port();
        let fingerprint = crate::tls::read_fingerprint(server.config())?;
        let stop = Arc::new(AtomicBool::new(false));
        {
            let (server, stop) = (Arc::clone(&server), Arc::clone(&stop));
            std::thread::Builder::new()
                .name("elora-share-server".to_string())
                .spawn(move || {
                    let _ = server.serve_listener(listener, Some(stop));
                })
                .map_err(|e| format!("could not start sharing: {e}"))?;
        }

        let runtime = runtime()?;
        let endpoint = runtime.block_on(bind(network, true))?;
        let addr = runtime.block_on(async {
            match network {
                Network::Internet => {
                    // Online means a relay has been reached, which is what lets people behind
                    // other firewalls find this device.
                    let _ = tokio::time::timeout(Duration::from_secs(10), endpoint.online()).await;
                    endpoint.addr()
                }
                Network::Loopback => {
                    let mut addr = EndpointAddr::new(endpoint.id());
                    for socket in endpoint.bound_sockets() {
                        let ip = if socket.ip().is_unspecified() {
                            std::net::Ipv4Addr::LOCALHOST.into()
                        } else {
                            socket.ip()
                        };
                        addr = addr.with_ip_addr(SocketAddr::new(ip, socket.port()));
                    }
                    addr
                }
            }
        });

        // The door: every stream a guest opens is joined to the temporary server.
        let door = endpoint.clone();
        runtime.spawn(async move {
            while let Some(incoming) = door.accept().await {
                tokio::spawn(async move {
                    let Ok(connection) = incoming.await else {
                        return;
                    };
                    while let Ok((send, recv)) = connection.accept_bi().await {
                        tokio::spawn(async move {
                            if let Ok(mut tcp) =
                                tokio::net::TcpStream::connect(("127.0.0.1", port)).await
                            {
                                let mut tunnel = tokio::io::join(recv, send);
                                let _ = tokio::io::copy_bidirectional(&mut tunnel, &mut tcp).await;
                            }
                        });
                    }
                });
            }
        });

        let ticket = |pw: &str, view: bool| Ticket {
            v: 1,
            addr: addr.clone(),
            fp: fingerprint.clone(),
            note: note.id.clone(),
            title: title.to_string(),
            pw: pw.to_string(),
            view,
        };
        Ok(Self {
            runtime: Some(runtime),
            endpoint,
            stop,
            note_id: note.id.clone(),
            local: ServerProfile {
                host: "127.0.0.1".to_string(),
                port,
                password: edit_pw.clone(),
                fingerprint: Some(fingerprint.clone()),
            },
            edit_link: ticket(&edit_pw, false).to_link(),
            view_link: ticket(&view_pw, true).to_link(),
            edit_web: ticket(&edit_pw, false).to_web_link(),
            view_web: ticket(&view_pw, true).to_web_link(),
            dir,
        })
    }

    /// The link for the kind of access given.
    pub fn link(&self, access: Access) -> &str {
        match access {
            Access::Edit => &self.edit_link,
            Access::View => &self.view_link,
        }
    }

    /// The link for the kind of access given, as a web address that can be clicked anywhere.
    pub fn web_link(&self, access: Access) -> &str {
        match access {
            Access::Edit => &self.edit_web,
            Access::View => &self.view_web,
        }
    }

    /// How the host's own app reaches the temporary server, to edit the note live like everybody
    /// else.
    pub fn local_profile(&self) -> ServerProfile {
        self.local.clone()
    }

    /// The id of the note on the temporary server.
    pub fn note_id(&self) -> &str {
        &self.note_id
    }
}

impl Drop for ShareHost {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(runtime) = self.runtime.take() {
            let endpoint = self.endpoint.clone();
            let _ = runtime.block_on(async {
                tokio::time::timeout(Duration::from_secs(2), endpoint.close()).await
            });
            runtime.shutdown_timeout(Duration::from_secs(1));
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A shared note joined from a link. Dropping it leaves.
pub struct Joined {
    runtime: Option<tokio::runtime::Runtime>,
    endpoint: Endpoint,
    /// How to reach the note: a door on this computer that leads to the host.
    pub profile: ServerProfile,
    pub note_id: String,
    pub title: String,
    /// True for a link that only lets you watch.
    pub view_only: bool,
}

/// Joins the note a link shares. Returns once the host has been reached.
pub fn join(link: &str, network: Network) -> Result<Joined, String> {
    let ticket = Ticket::from_link(link)?;
    let runtime = runtime()?;
    let endpoint = runtime.block_on(bind(network, false))?;
    let connection = runtime
        .block_on(async {
            tokio::time::timeout(
                Duration::from_secs(25),
                endpoint.connect(ticket.addr.clone(), ALPN),
            )
            .await
        })
        .map_err(|_| "could not reach the person sharing this note: are they online?".to_string())?
        .map_err(|e| format!("could not reach the person sharing this note: {e}"))?;

    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("could not open a door: {e}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("could not open a door: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("could not open a door: {e}"))?
        .port();
    runtime.spawn(async move {
        let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
            return;
        };
        while let Ok((mut tcp, _)) = listener.accept().await {
            let connection = connection.clone();
            tokio::spawn(async move {
                if let Ok((send, recv)) = connection.open_bi().await {
                    let mut tunnel = tokio::io::join(recv, send);
                    let _ = tokio::io::copy_bidirectional(&mut tcp, &mut tunnel).await;
                }
            });
        }
    });

    Ok(Joined {
        runtime: Some(runtime),
        endpoint,
        profile: ServerProfile {
            host: "127.0.0.1".to_string(),
            port,
            password: ticket.pw,
            fingerprint: Some(ticket.fp),
        },
        note_id: ticket.note,
        title: ticket.title,
        view_only: ticket.view,
    })
}

impl Drop for Joined {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            let endpoint = self.endpoint.clone();
            let _ = runtime.block_on(async {
                tokio::time::timeout(Duration::from_secs(2), endpoint.close()).await
            });
            runtime.shutdown_timeout(Duration::from_secs(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(view: bool) -> Ticket {
        let id: EndpointId = iroh::SecretKey::from_bytes(&[7; 32]).public();
        let addr = EndpointAddr::new(id)
            .with_relay_url(DEFAULT_RELAY.parse::<RelayUrl>().unwrap())
            .with_ip_addr("69.21.135.75:42088".parse().unwrap())
            .with_ip_addr("[fe80::1]:42088".parse().unwrap());
        Ticket {
            v: 1,
            addr,
            fp: "c962aae3ea8200447773acd49939b8d454030ae6196c1b36ec92877cb0c25c4d".into(),
            note: "test-share-20261028-210530".into(),
            title: "Test Share".into(),
            pw: secret(),
            view,
        }
    }

    #[test]
    fn a_ticket_packs_small_and_reads_back_the_same() {
        for view in [false, true] {
            let ticket = sample(view);
            let link = ticket.to_link();
            let json = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&ticket).unwrap());
            assert!(
                link.len() < LINK_PREFIX.len() + json.len() / 2,
                "{} packed against {} as JSON",
                link.len(),
                json.len()
            );
            let back = Ticket::from_link(&link).unwrap();
            assert_eq!(back.addr, ticket.addr);
            assert_eq!(
                (back.fp, back.note, back.title, back.pw, back.view),
                (ticket.fp, ticket.note, ticket.title, ticket.pw, ticket.view)
            );
        }
    }

    #[test]
    fn links_made_before_packing_still_open() {
        let ticket = sample(false);
        let old = format!(
            "{LINK_PREFIX}{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&ticket).unwrap())
        );
        assert_eq!(Ticket::from_link(&old).unwrap().pw, ticket.pw);
        // A cut-off packed link is refused rather than misread.
        let packed = ticket.to_link();
        assert!(Ticket::from_link(&packed[..packed.len() - 9]).is_err());
    }

    #[test]
    fn a_long_title_is_cut_to_keep_the_link_short() {
        let mut ticket = sample(false);
        ticket.title = "é".repeat(100);
        let back = Ticket::from_link(&ticket.to_link()).unwrap();
        assert!(back.title.len() <= 60 && !back.title.is_empty());
    }
}
