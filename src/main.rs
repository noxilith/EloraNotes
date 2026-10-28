#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use elora_notes::app::EloraApp;
use elora_notes::storage::{migrate_legacy_notes, NoteStorage};
use std::path::PathBuf;

fn main() -> eframe::Result<()> {
    let cli_dir = parse_cli_dir();
    // `elora --mcp` serves the notes to an AI assistant over stdin/stdout instead of opening a
    // window, so it needs no display and no other binary.
    if std::env::args().skip(1).any(|arg| arg == "--mcp") {
        let notes_dir = cli_dir.unwrap_or_else(NoteStorage::default_notes_dir);
        let storage = NoteStorage::new(notes_dir).expect("Failed to open the notes folder");
        let options = elora_notes::mcp::VaultOptions {
            read_only: has_flag("--read-only"),
            folder: flag_value("--folder"),
            no_shared: has_flag("--no-shared"),
        };
        let vault = elora_notes::mcp::Vault::with_options(storage, options);
        if let Err(e) = elora_notes::mcp::serve(vault) {
            eprintln!("elora --mcp: {e}");
            std::process::exit(1);
        }
        return Ok(());
    }
    // Started from a share link (a click on elora://… or the page that hands it over): if a copy is
    // already running it takes the link, and this one has nothing more to do.
    let launch_link = elora_notes::links::link_in_args(std::env::args().skip(1));
    if let Some(link) = &launch_link {
        if elora_notes::links::forward(link) {
            return Ok(());
        }
    }
    // Tell the system that this program opens elora:// links; quick, and on its own thread.
    std::thread::spawn(elora_notes::links::register_scheme);
    let uses_default_dir = cli_dir.is_none();
    let notes_dir = cli_dir.unwrap_or_else(NoteStorage::default_notes_dir);
    let storage = NoteStorage::new(notes_dir).expect("Failed to initialize EloraNotes directory");
    if uses_default_dir {
        let copied = migrate_legacy_notes(storage.root_dir());
        if copied > 0 {
            eprintln!(
                "EloraNotes: copied {copied} file(s) from the old notes folder into {}",
                storage.root_dir().display()
            );
        }
    }

    // On Wayland a synchronized swap waits for a frame callback that the compositor
    // stops sending while the surface is hidden (another workspace, fullscreen app on
    // top). The UI thread then blocks inside eglSwapBuffers and the window stays frozen
    // until it is shown again. Presenting without waiting avoids that deadlock; egui's
    // reactive repaint scheduling already paces the UI. X11 and the other platforms keep
    // vsync so they stay tear-free.
    let vsync = std::env::var_os("WAYLAND_DISPLAY").is_none();

    let mut viewport = egui::ViewportBuilder::default()
        .with_title("EloraNotes")
        .with_app_id("elora-notes")
        .with_inner_size([1060.0, 700.0])
        .with_min_inner_size([640.0, 420.0]);
    // The window the last run left behind. A size below the minimum is asked for at the minimum,
    // since the window manager would only clamp it after the title bar had been placed.
    if let Some(window) = storage.load_settings().window {
        viewport = viewport
            .with_inner_size([window.width.max(640.0), window.height.max(420.0)])
            .with_maximized(window.maximized);
        if !window.maximized {
            viewport = viewport.with_position([window.x, window.y]);
        }
    }
    // Windows and macOS take the taskbar icon from the running window; the Linux builds also
    // install a .desktop file and hicolor icons, which is what compositors look up by app id.
    let viewport = match window_icon() {
        Some(icon) => viewport.with_icon(icon),
        None => viewport,
    };

    let native_options = eframe::NativeOptions {
        vsync,
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "EloraNotes",
        native_options,
        Box::new(move |cc| {
            let mut app = EloraApp::new(cc, storage);
            if let Some(link) = launch_link {
                app.open_link(link);
            }
            Ok(Box::new(app))
        }),
    )
}

/// The EloraNotes mark, generated into `assets/icons/` by `scripts/make-icons.py` and
/// embedded as raw RGBA so the app needs no image decoder at runtime.
#[cfg(has_window_icon)]
fn window_icon() -> Option<egui::IconData> {
    const EDGE: usize = 256;
    const RGBA: &[u8] = include_bytes!("../assets/icons/window-icon-256.rgba");
    if RGBA.len() != EDGE * EDGE * 4 {
        return None;
    }
    Some(egui::IconData {
        rgba: RGBA.to_vec(),
        width: EDGE as u32,
        height: EDGE as u32,
    })
}

#[cfg(not(has_window_icon))]
fn window_icon() -> Option<egui::IconData> {
    None
}

/// True if the command line has exactly this flag.
fn has_flag(flag: &str) -> bool {
    std::env::args().skip(1).any(|arg| arg == flag)
}

/// The word after `flag` on the command line.
fn flag_value(flag: &str) -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == flag {
            return args.next();
        }
    }
    None
}

fn parse_cli_dir() -> Option<PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dir" | "-d" => {
                if let Some(dir) = args.next() {
                    return Some(PathBuf::from(dir));
                }
            }
            // Its value is a folder inside the notes, not the notes folder itself.
            "--folder" => {
                args.next();
            }
            "--help" | "-h" => {
                println!(
                    "EloraNotes — Minimalistic cross-platform Markdown notes app\n\n\
                     USAGE:\n  \
                       elora [OPTIONS] [NOTES_DIR]\n\n\
                     OPTIONS:\n  \
                       -d, --dir <PATH>   Directory to store Markdown notes\n  \
                           --mcp          Serve the notes to an AI assistant over MCP (stdio)\n  \
                           --read-only    With --mcp: let the assistant read but never change\n  \
                           --folder <F>   With --mcp: show the assistant only this folder\n  \
                           --no-shared    With --mcp: leave the shared library out\n  \
                       -h, --help         Show this help message"
                );
                std::process::exit(0);
            }
            other if !other.starts_with('-') => {
                return Some(PathBuf::from(other));
            }
            _ => {}
        }
    }
    None
}
