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
use iroh::{endpoint::presets, Endpoint, EndpointAddr};
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
const WEB_PREFIX: &str = "https://noxilith.github.io/join/#";

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
    pub fn to_link(&self) -> String {
        let json = serde_json::to_vec(self).unwrap_or_default();
        format!(
            "{LINK_PREFIX}{}",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
        )
    }

    /// The same link as a web address that works when clicked in a browser, a chat or an email:
    /// a small page on the docs site hands it to the app. What follows the `#` is never sent to
    /// any server, so the secret in it stays between the page's own script and the person.
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
        let ticket: Ticket = serde_json::from_slice(&bytes)
            .map_err(|_| "that share link is damaged: copy it again")?;
        if ticket.v != 1 {
            return Err("that link is from a newer EloraNotes: update the app".to_string());
        }
        Ok(ticket)
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
