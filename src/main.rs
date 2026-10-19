#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod icon_names;
mod icons;
mod markdown;
mod storage;
mod theme;

use app::EloraApp;
use std::path::PathBuf;
use storage::{migrate_legacy_notes, NoteStorage};

fn main() -> eframe::Result<()> {
    let cli_dir = parse_cli_dir();
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

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("EloraNotes")
            .with_app_id("elora-notes")
            .with_inner_size([1060.0, 700.0])
            .with_min_inner_size([640.0, 420.0]),
        ..Default::default()
    };

    eframe::run_native(
        "EloraNotes",
        native_options,
        Box::new(|cc| Ok(Box::new(EloraApp::new(cc, storage)))),
    )
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
            "--help" | "-h" => {
                println!(
                    "EloraNotes — Minimalistic cross-platform Markdown notes app\n\n\
                     USAGE:\n  \
                       elora [OPTIONS] [NOTES_DIR]\n\n\
                     OPTIONS:\n  \
                       -d, --dir <PATH>   Directory to store Markdown notes\n  \
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
