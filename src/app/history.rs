use super::*;
use crate::storage::NoteVersion;

/// The window listing a note's earlier copies: which note, what was kept, and which copy is
/// being looked at.
pub(super) struct History {
    note_id: String,
    versions: Vec<(NoteVersion, Note)>,
    selected: usize,
}

impl EloraApp {
    /// Opens the earlier versions of the note at `note_idx`. A copy is kept whenever an
    /// assistant overwrites or deletes the note, so this is where such a change is undone.
    pub(super) fn open_history(&mut self, note_idx: usize) {
        let note = &self.notes[note_idx];
        let versions = self
            .storage
            .versions(&note.id)
            .into_iter()
            .filter_map(|version| {
                let earlier = self.storage.read_version(&version, note).ok()?;
                Some((version, earlier))
            })
            .collect();
        self.history = Some(History {
            note_id: note.id.clone(),
            versions,
            selected: 0,
        });
    }

    /// Makes an assistant's access to a note read-only, or gives it back. A note on this computer
    /// keeps the choice in its own file; a shared note has none, so it is kept in the settings.
    pub(super) fn set_mcp_read_only(&mut self, note_idx: usize, read_only: bool) {
        let id = self.notes[note_idx].id.clone();
        if self.is_remote(&id) {
            self.mcp_read_only_ids.retain(|known| *known != id);
            if read_only {
                self.mcp_read_only_ids.push(id);
            }
            self.persist_settings();
        } else {
            self.notes[note_idx].ai.read_only = read_only;
            if let Err(e) = self.persist_note(note_idx) {
                self.status_message = Some(format!("Auto-save error: {e}"));
                return;
            }
        }
        self.status_message = Some(
            if read_only {
                "Assistants can read this note but not change it"
            } else {
                "Assistants may change this note again"
            }
            .to_string(),
        );
    }

    /// Puts an earlier copy's text back. What is there now is kept first, so restoring can
    /// itself be undone. Where the note lives (folder, pin, trash) and who may edit it stay as
    /// they are.
    pub(super) fn restore_version(&mut self, id: &str, earlier: &Note) {
        let Some(idx) = self.notes.iter().position(|n| n.id == id) else {
            return;
        };
        let _ = self.storage.snapshot(&self.notes[idx]);
        let now = &self.notes[idx];
        let mut restored = earlier.clone();
        restored.id = now.id.clone();
        restored.file_path = now.file_path.clone();
        restored.folder = now.folder.clone();
        restored.pinned = now.pinned;
        restored.trashed = now.trashed;
        restored.created_at = now.created_at;
        restored.ai = now.ai.clone();
        restored.ai.edited = false;
        restored.updated_at = Utc::now();
        let title = restored.display_title();
        self.notes[idx] = restored;
        if let Err(e) = self.persist_note(idx) {
            self.status_message = Some(format!("Auto-save error: {e}"));
            return;
        }
        self.resort();
        self.status_message = Some(format!("Restored an earlier version of “{title}”"));
    }

    pub(super) fn history_window(&mut self, ctx: &egui::Context) {
        let Some(history) = self.history.as_mut() else {
            return;
        };
        let p = self.theme.palette_with(&self.colors, self.glow_strength);
        let mut open = true;
        let mut restore: Option<Note> = None;
        let title = self
            .notes
            .iter()
            .find(|n| n.id == history.note_id)
            .map(|n| n.display_title())
            .unwrap_or_default();
        egui::Window::new(format!("Earlier versions of “{title}”"))
            .id(Id::new("elora_note_history"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([560.0, 360.0])
            .show(ctx, |ui| {
                if history.versions.is_empty() {
                    ui.label(
                        RichText::new(
                            "Nothing has been kept yet. A copy is saved each time an assistant \
                             changes or deletes this note.",
                        )
                        .size(12.0)
                        .color(p.fg_secondary),
                    );
                    return;
                }
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(190.0);
                        egui::ScrollArea::vertical()
                            .id_salt("history_list")
                            .max_height(300.0)
                            .show(ui, |ui| {
                                for (i, (version, earlier)) in history.versions.iter().enumerate() {
                                    let when = version
                                        .taken
                                        .with_timezone(&Local)
                                        .format("%b %e, %H:%M:%S");
                                    let label =
                                        format!("{when}  ·  {} words", earlier.word_count());
                                    if ui.selectable_label(history.selected == i, label).clicked() {
                                        history.selected = i;
                                    }
                                }
                            });
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        let (_, earlier) = &history.versions[history.selected];
                        ui.label(
                            RichText::new(earlier.display_title())
                                .strong()
                                .color(p.fg_primary),
                        );
                        egui::ScrollArea::vertical()
                            .id_salt("history_preview")
                            .max_height(250.0)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(&earlier.body)
                                            .size(12.0)
                                            .color(p.fg_secondary),
                                    )
                                    .wrap(),
                                );
                            });
                        ui.add_space(4.0);
                        if ui
                            .button(format!("{}  Restore this version", icon::RESTORE))
                            .on_hover_text("The text as it is now is kept first")
                            .clicked()
                        {
                            restore = Some(earlier.clone());
                        }
                    });
                });
            });
        if let Some(earlier) = restore {
            let id = history.note_id.clone();
            self.history = None;
            self.restore_version(&id, &earlier);
            return;
        }
        if !open {
            self.history = None;
        }
    }
}
