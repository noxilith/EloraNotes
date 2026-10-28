//! Opening EloraNotes from a share link.
//!
//! Three things have to work for clicking `elora://share/…` (or the web address that hands it
//! over) to end up inside the app:
//!
//! * the operating system has to know that this program handles `elora://` links (see
//!   [`register_scheme`]; installers and bundles say so as well);
//! * a program started with a link has to pass it to the copy that is already running instead of
//!   opening a second window ([`forward`] and [`listen`]);
//! * and where the system cannot pass a link at all, the app notices one on the clipboard.

use crate::share::find_link;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Where the running copy leaves the way to reach it: a port on this computer and a secret that
/// only programs of this user can read, so no other program can make the app join anything.
fn instance_file() -> PathBuf {
    let dir = std::env::var_os("ELORA_INSTANCE_DIR")
        .map(PathBuf::from)
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("elora-notes")
        });
    dir.join("instance")
}

/// Links that other copies of the program handed over, waiting to be opened.
#[derive(Clone, Default)]
pub struct LinkInbox {
    queue: Arc<Mutex<Vec<String>>>,
}

impl LinkInbox {
    /// Everything that arrived since the last call.
    pub fn take(&self) -> Vec<String> {
        self.queue
            .lock()
            .map(|mut queue| std::mem::take(&mut *queue))
            .unwrap_or_default()
    }
}

/// Starts listening for links from other copies of the program. `wake` is called when one
/// arrives, so an idle window gets drawn and opens it.
pub fn listen(wake: impl Fn() + Send + 'static) -> LinkInbox {
    let inbox = LinkInbox::default();
    let Ok(listener) = TcpListener::bind(("127.0.0.1", 0)) else {
        return inbox;
    };
    let Ok(port) = listener.local_addr().map(|addr| addr.port()) else {
        return inbox;
    };
    let secret: String = (0..16)
        .map(|_| format!("{:02x}", rand::random::<u8>()))
        .collect();
    let file = instance_file();
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if crate::storage::write_atomic(&file, format!("{port}\n{secret}\n")).is_err() {
        return inbox;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
    }
    let queue = Arc::clone(&inbox.queue);
    std::thread::Builder::new()
        .name("elora-links".to_string())
        .spawn(move || {
            for stream in listener.incoming().flatten() {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let mut text = String::new();
                if (&stream).take(16 * 1024).read_to_string(&mut text).is_err() {
                    continue;
                }
                let Some((token, link)) = text.split_once('\n') else {
                    continue;
                };
                if token.trim() != secret {
                    continue;
                }
                if let Some(link) = find_link(link) {
                    if let Ok(mut queue) = queue.lock() {
                        queue.push(link);
                    }
                    wake();
                    let _ = (&stream).write_all(b"ok");
                }
            }
        })
        .ok();
    inbox
}

/// Hands `link` to the copy that is already running. True when it took it: this copy should then
/// exit instead of opening a window of its own.
pub fn forward(link: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(instance_file()) else {
        return false;
    };
    let mut lines = text.lines();
    let (Some(port), Some(secret)) = (
        lines.next().and_then(|p| p.parse::<u16>().ok()),
        lines.next(),
    ) else {
        return false;
    };
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(500)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(1500)));
    if stream
        .write_all(format!("{secret}\n{link}").as_bytes())
        .is_err()
    {
        return false;
    }
    let _ = stream.shutdown(Shutdown::Write);
    let mut answer = String::new();
    let _ = stream.read_to_string(&mut answer);
    answer == "ok"
}

/// The first share link among the program's arguments.
pub fn link_in_args(args: impl IntoIterator<Item = String>) -> Option<String> {
    args.into_iter().find_map(|arg| find_link(&arg))
}

/// The share link on the clipboard, if there is one.
pub fn link_on_clipboard() -> Option<String> {
    #[cfg(feature = "gui")]
    {
        let text = arboard::Clipboard::new().ok()?.get_text().ok()?;
        find_link(&text)
    }
    #[cfg(not(feature = "gui"))]
    {
        None
    }
}

/// The program to start for a link: the AppImage file when running from one (the binary inside
/// it is gone once it closes), otherwise this binary.
fn program_path() -> Option<PathBuf> {
    std::env::var_os("APPIMAGE")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok())
}

/// The desktop entry that makes a Linux desktop hand `elora://` links to `program`.
pub fn desktop_entry(program: &Path) -> String {
    let exec = program
        .display()
        .to_string()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    format!(
        "[Desktop Entry]\nType=Application\nName=EloraNotes\nComment=Open a shared note\n\
         Exec=\"{exec}\" %u\nIcon=io.github.noxilith.EloraNotes\nTerminal=false\n\
         NoDisplay=true\nMimeType=x-scheme-handler/elora;\nStartupWMClass=elora-notes\n"
    )
}

/// Tells the operating system that this program opens `elora://` links, for this user. Cheap and
/// safe to do on every start: it changes nothing when it is already right, and it never fails
/// loudly (installers and app bundles register the link type themselves where they can).
pub fn register_scheme() {
    let Some(program) = program_path() else {
        return;
    };
    #[cfg(target_os = "linux")]
    linux_register(&program);
    #[cfg(windows)]
    windows_register(&program);
    #[cfg(not(any(target_os = "linux", windows)))]
    let _ = program;
}

#[cfg(target_os = "linux")]
fn linux_register(program: &Path) {
    // Inside Flatpak the app's own desktop file says it, and the sandbox cannot write outside.
    if Path::new("/.flatpak-info").exists() {
        return;
    }
    let Some(dir) = dirs::data_dir().map(|dir| dir.join("applications")) else {
        return;
    };
    let file = dir.join("elora-notes-links.desktop");
    let wanted = desktop_entry(program);
    if std::fs::read_to_string(&file).is_ok_and(|have| have == wanted) {
        return;
    }
    if std::fs::create_dir_all(&dir).is_err() || std::fs::write(&file, wanted).is_err() {
        return;
    }
    let quietly = |program: &str, args: &[&str]| {
        let _ = std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    };
    quietly(
        "xdg-mime",
        &[
            "default",
            "elora-notes-links.desktop",
            "x-scheme-handler/elora",
        ],
    );
    quietly("update-desktop-database", &[&dir.display().to_string()]);
}

#[cfg(windows)]
fn windows_register(program: &Path) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let command = format!("\"{}\" \"%1\"", program.display());
    let add = |key: &str, name: Option<&str>, value: &str| {
        let mut reg = std::process::Command::new("reg");
        reg.args(["add", key]);
        match name {
            Some(name) => reg.args(["/v", name]),
            None => reg.arg("/ve"),
        };
        let _ = reg
            .args(["/d", value, "/f"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    };
    let root = r"HKCU\Software\Classes\elora";
    add(root, None, "URL:EloraNotes share link");
    add(root, Some("URL Protocol"), "");
    add(&format!(r"{root}\shell\open\command"), None, &command);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::{Access, Network, ShareHost};

    #[test]
    fn a_link_is_found_in_arguments_and_the_desktop_entry_names_the_program() {
        let host = ShareHost::start("Plan", "words", Network::Loopback).unwrap();
        let link = host.link(Access::Edit).to_string();
        let args = ["--dir", "/notes", link.as_str(), "other"].map(String::from);
        assert_eq!(link_in_args(args), Some(link.clone()));
        assert_eq!(link_in_args(["--mcp".to_string()]), None);
        assert!(link_in_args([host.web_link(Access::View).to_string()]).is_some());

        let entry = desktop_entry(Path::new("/opt/Elora Notes/elora"));
        assert!(
            entry.contains("Exec=\"/opt/Elora Notes/elora\" %u"),
            "{entry}"
        );
        assert!(entry.contains("MimeType=x-scheme-handler/elora;"));
        assert!(entry.contains("NoDisplay=true"));
    }

    /// A link handed to a running copy arrives there, and nothing else gets in.
    #[test]
    fn a_running_copy_receives_links_from_another() {
        let dir = std::env::temp_dir().join(format!("elora-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // SAFETY: the only test that reads this variable; nothing else changes the environment.
        unsafe { std::env::set_var("ELORA_INSTANCE_DIR", &dir) };

        let host = ShareHost::start("Plan", "words", Network::Loopback).unwrap();
        let link = host.link(Access::Edit).to_string();

        // Nobody is listening yet.
        assert!(!forward(&link));

        let woken = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = Arc::clone(&woken);
        let inbox = listen(move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        assert!(forward(&link), "the running copy did not take the link");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut got = Vec::new();
        while got.is_empty() && std::time::Instant::now() < deadline {
            got = inbox.take();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(got, vec![link.clone()]);
        assert!(woken.load(std::sync::atomic::Ordering::SeqCst) >= 1);

        // A made-up secret, or something that is not a link, is refused.
        let file = std::fs::read_to_string(instance_file()).unwrap();
        let port: u16 = file.lines().next().unwrap().parse().unwrap();
        for payload in [format!("wrong-secret\n{link}"), "no newline".to_string()] {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            stream.write_all(payload.as_bytes()).unwrap();
            stream.shutdown(Shutdown::Write).unwrap();
            let mut answer = String::new();
            let _ = stream.read_to_string(&mut answer);
            assert_ne!(answer, "ok", "{payload}");
        }
        std::thread::sleep(Duration::from_millis(100));
        assert!(inbox.take().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
