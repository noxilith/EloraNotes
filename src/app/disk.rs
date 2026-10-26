use super::*;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// How often the notes folder is looked at for edits made by another program, such as an AI
/// assistant working through `elora --mcp`.
const WATCH_INTERVAL: Duration = Duration::from_millis(750);

/// Watches the notes folder from a thread of its own, so the window is only drawn again when
/// something in it really changed. Checking from the frame loop instead would redraw the whole
/// window on every tick, which is most of the cost of an app that is otherwise sitting still.
pub(super) struct DiskWatch {
    changed: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}

impl DiskWatch {
    pub(super) fn start(ctx: &egui::Context, dir: PathBuf) -> Self {
        let changed = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (flag, halt, wake) = (changed.clone(), stop.clone(), ctx.clone());
        std::thread::spawn(move || {
            let mut last = None;
            while !halt.load(Ordering::Relaxed) {
                let now = fingerprint(&dir);
                if last.is_some_and(|before| before != now) {
                    flag.store(true, Ordering::Relaxed);
                    wake.request_repaint();
                }
                last = Some(now);
                std::thread::sleep(WATCH_INTERVAL);
            }
        });
        Self { changed, stop }
    }

    /// True once if the folder changed since the last time this was asked.
    fn take_change(&self) -> bool {
        self.changed.swap(false, Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(super) fn flag_change(&self) {
        self.changed.store(true, Ordering::Relaxed);
    }
}

impl Drop for DiskWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// One number that changes whenever a note file is added, removed, resized or rewritten. Each
/// file adds its own hash, so the order the folder lists them in does not matter.
fn fingerprint(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("md"))
        .filter_map(|e| Some((e.file_name(), e.metadata().ok()?)))
        .fold(0u64, |sum, (name, meta)| {
            let mut hasher = DefaultHasher::new();
            name.hash(&mut hasher);
            meta.len().hash(&mut hasher);
            meta.modified().ok().hash(&mut hasher);
            sum.wrapping_add(hasher.finish())
        })
}

impl EloraApp {
    /// Brings notes that another program created, edited, trashed or deleted into the open
    /// window, so they show up as they happen instead of after a restart.
    pub(super) fn poll_disk(&mut self) {
        if !self.disk_watch.take_change() {
            return;
        }
        let changes = self.storage.take_disk_changes();
        if changes.changed.is_empty() && changes.removed.is_empty() {
            return;
        }
        let mut notice = None;
        for fresh in changes.changed {
            if self.is_remote(&fresh.id) {
                continue;
            }
            match self
                .notes
                .iter()
                .position(|n| n.file_path == fresh.file_path)
            {
                Some(idx) => {
                    if fresh.ai.edited && fresh.body != self.notes[idx].body {
                        notice = Some(format!(
                            "“{}” was changed by an assistant",
                            fresh.display_title()
                        ));
                    }
                    self.notes[idx] = fresh;
                }
                None => {
                    if fresh.ai.created {
                        notice = Some(format!("An assistant added “{}”", fresh.display_title()));
                    }
                    self.notes.push(fresh);
                }
            }
        }
        if notice.is_some() {
            self.status_message = notice;
        }
        for path in changes.removed {
            self.notes
                .retain(|n| n.file_path != path || self.remote_ids.contains(&n.id));
        }
        self.resort();
        let selected_gone = self
            .selected_id
            .as_ref()
            .is_some_and(|id| !self.notes.iter().any(|n| &n.id == id));
        if selected_gone {
            self.select_first_visible();
        }
    }
}
