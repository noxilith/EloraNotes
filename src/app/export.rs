//! Importing Markdown files and exporting notes as HTML pages with their pictures.
use super::*;

impl EloraApp {
    /// Writes HTML copies of the selected note (or every live note) to the Downloads folder.
    /// Falls back to the notes folder when no Downloads folder is available.
    /// Imports `.md` files from the `Import` folder inside the notes folder, then reloads the list.
    pub(super) fn import_markdown(&mut self) {
        let import_dir = self.storage.root_dir().join("Import");
        if let Err(e) = std::fs::create_dir_all(&import_dir) {
            self.status_message = Some(format!("Import failed: {e}"));
            return;
        }
        match self.storage.import_markdown_dir(&import_dir) {
            Ok(0) => {
                self.status_message = Some(format!(
                    "No .md files in {} to import",
                    import_dir.display()
                ));
            }
            Ok(count) => {
                self.notes = self.storage.load_all().unwrap_or_default();
                self.status_message = Some(format!("Imported {count} note(s)"));
            }
            Err(e) => self.status_message = Some(format!("Import failed: {e}")),
        }
    }

    /// Works out what an export would write and keeps it for the confirmation: the size is shown
    /// before anything touches the disk.
    pub(super) fn export_html(&mut self, whole_vault: bool) {
        match self.plan_export(whole_vault) {
            Some(plan) => self.export_offer = Some(plan),
            None => self.status_message = Some("Select a note to export".to_string()),
        }
    }

    /// The pages an export would write, the pictures that go beside them and where they land.
    /// Nothing is read from or written to the export folder here, so this is safe to call for a
    /// count.
    pub(super) fn plan_export(&self, whole_vault: bool) -> Option<ExportPlan> {
        let notes: Vec<&Note> = if whole_vault {
            self.notes.iter().filter(|n| !n.trashed).collect()
        } else {
            let start = self
                .selected_id
                .as_ref()
                .and_then(|id| self.notes.iter().find(|n| &n.id == id))?;
            // The note and the notes linked to it either way, so no page it writes links to a
            // page that was not written.
            notes_linked_around(&self.notes, start)
        };

        // A folder somebody picked is used as it is: they chose where the pages go. Until then a
        // single note goes to the downloads folder, and a whole vault gets a folder of its own
        // inside it so it does not mix with everything else.
        let dir = match self.export_dir.clone() {
            Some(dir) => dir,
            None => {
                let base =
                    dirs::download_dir().unwrap_or_else(|| self.storage.root_dir().to_path_buf());
                if whole_vault {
                    base.join("EloraNotes Export")
                } else {
                    base
                }
            }
        };

        let mut pictures = Vec::new();
        let mut copies = Vec::new();
        let mut number = 0usize;
        let mut pages = Vec::new();
        for note in &notes {
            // The same folder the preview resolves a note's pictures from: a shared note has no
            // file of its own here, so the library folder is the closest thing it has.
            let note_dir = if self.is_remote(&note.id) {
                Some(self.storage.root_dir().to_path_buf())
            } else {
                note.file_path.parent().map(Path::to_path_buf)
            };
            let plan = plan_pictures(&note.id, &note.body, note_dir.as_deref(), &mut number);
            copies.extend(plan.copies);
            pictures.push(plan.mapped);

            // A wiki link matches either title a note can have, the same rule clicking one uses.
            let heading = note.display_title();
            let mut titles = vec![note.title.clone()];
            if !titles
                .iter()
                .any(|title| title.eq_ignore_ascii_case(&heading))
            {
                titles.push(heading.clone());
            }
            pages.push(ExportPage {
                id: note.id.clone(),
                heading,
                titles,
                body: note.body.clone(),
            });
        }
        Some(ExportPlan {
            dir,
            whole_vault,
            pages,
            pictures,
            copies,
        })
    }

    /// The export that was worked out, waiting for a yes: how many pages and pictures it writes, and
    /// where. Nothing is written until **Export** is pressed, so a single note that turns out to
    /// pull in a hundred through its links can be called off first.
    pub(super) fn render_export_offer(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(mut plan) = self.export_offer.take() else {
            return;
        };
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            // A folder answer that arrives after this is not for anything.
            self.export_folder_pick = None;
            return;
        }

        // The folder dialog answers between frames, on its own thread.
        let answer = self.export_folder_pick.as_ref().map(|pick| pick.try_recv());
        match answer {
            Some(Ok(Some(dir))) => {
                self.export_folder_pick = None;
                self.export_dir = Some(dir.clone());
                // Remembered, so the next export starts where this one went.
                self.persist_settings();
                plan.dir = dir;
            }
            Some(Ok(None)) | Some(Err(TryRecvError::Disconnected)) => {
                self.export_folder_pick = None;
            }
            _ => {}
        }
        let picking = self.export_folder_pick.is_some();
        let mut pick_folder = false;

        let pages = plan.pages.len();
        let pictures = plan.copies.len();
        let question = if pictures == 0 {
            format!(
                "Write {pages} {}?",
                if pages == 1 { "page" } else { "pages" }
            )
        } else {
            format!(
                "Write {pages} {} and {pictures} {}?",
                if pages == 1 { "page" } else { "pages" },
                if pictures == 1 { "picture" } else { "pictures" },
            )
        };
        let scope = if plan.whole_vault {
            "Every note in the vault.".to_string()
        } else if pages == 1 {
            "The note on its own: it links to nothing, and nothing links to it.".to_string()
        } else {
            format!(
                "The note and the {} linked to it, either way.",
                if pages == 2 {
                    "one note".to_string()
                } else {
                    format!("{} notes", pages - 1)
                }
            )
        };

        let mut open = true;
        let mut confirmed = false;
        let mut cancelled = false;
        egui::Window::new("Export to HTML")
            .id(Id::new("elora_export_offer"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_TOP, Vec2::new(0.0, 68.0))
            .frame(
                Frame::new()
                    .fill(p.bg_sidebar)
                    .stroke(Stroke::new(1.5_f32, p.accent))
                    .corner_radius(CornerRadius::same(10))
                    .inner_margin(Margin::symmetric(14, 12)),
            )
            .show(ctx, |ui| {
                ui.set_width(360.0);
                ui.label(
                    RichText::new(question)
                        .size(13.5)
                        .strong()
                        .color(p.fg_primary),
                );
                ui.label(RichText::new(scope).size(11.0).color(p.fg_muted));
                ui.add_space(4.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(format!("Into {}", plan.dir.display()))
                            .size(10.5)
                            .color(p.fg_secondary),
                    );
                    if ui
                        .add_enabled(
                            !picking,
                            egui::Button::new(RichText::new("Choose…").size(10.5)).frame(false),
                        )
                        .on_hover_text("Pick the folder the pages and pictures go into")
                        .clicked()
                    {
                        pick_folder = true;
                    }
                });
                if pictures > 0 {
                    ui.label(
                        RichText::new(
                            "Pictures are copied into a pictures folder beside the pages.",
                        )
                        .size(10.0)
                        .color(p.fg_muted),
                    );
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let export = ui.add(
                        egui::Button::new(
                            RichText::new(format!("{}  Export", icon::DOWNLOAD))
                                .color(p.bg_canvas)
                                .strong()
                                .size(12.5),
                        )
                        .fill(p.accent)
                        .corner_radius(CornerRadius::same(6)),
                    );
                    if export.clicked() {
                        confirmed = true;
                    }
                    if ui.button("Cancel").clicked() {
                        cancelled = true;
                    }
                });
            });

        if pick_folder {
            self.export_folder_pick = Some(choose_folder(ctx, &plan.dir));
        }

        if confirmed {
            // A folder answer that arrives after this is not for anything.
            self.export_folder_pick = None;
            self.status_message = Some(match write_export(&plan) {
                Ok(summary) => summary,
                Err(e) => format!("Export failed: {e}"),
            });
        } else if open && !cancelled {
            // Still waiting for an answer: keep the offer for the next frame.
            self.export_offer = Some(plan);
        } else {
            self.export_folder_pick = None;
        }
    }
}

/// The note a `[[link]]` opens, by the rule a click uses: the title the note was written with or
/// the title it shows, either case, and never a note in the trash.
pub(super) fn note_for_link<'a>(notes: &'a [Note], target: &str) -> Option<&'a Note> {
    let wanted = target.trim();
    if wanted.is_empty() {
        return None;
    }
    notes.iter().find(|note| {
        !note.trashed
            && (note.title.eq_ignore_ascii_case(wanted)
                || note.display_title().eq_ignore_ascii_case(wanted))
    })
}

/// The note to export and the notes linked to it either way: the ones its `[[links]]` reach, the
/// ones that link to it, and so on until nothing new turns up. Exporting that set is what keeps a
/// page's links from pointing at a page the export never wrote. The note comes first, then the
/// notes it links to, then the ones linked to it, each in the order they were found.
pub(super) fn notes_linked_around<'a>(notes: &'a [Note], start: &'a Note) -> Vec<&'a Note> {
    // Each note's links, resolved once: following them in both directions is then a lookup. A note
    // in the trash links to nothing and is linked to by nothing, the way it is everywhere else.
    let links: Vec<Vec<usize>> = notes
        .iter()
        .map(|note| {
            if note.trashed {
                return Vec::new();
            }
            extract_wiki_links(&note.body)
                .iter()
                .filter_map(|target| {
                    let found = note_for_link(notes, target)?;
                    notes.iter().position(|note| note.id == found.id)
                })
                .collect()
        })
        .collect();
    let Some(start) = notes.iter().position(|note| note.id == start.id) else {
        return vec![start];
    };

    let mut picked = vec![start];
    let mut next = 0;
    while next < picked.len() {
        let current = picked[next];
        // The notes this one links to...
        for &target in &links[current] {
            if !picked.contains(&target) {
                picked.push(target);
            }
        }
        // ...and the notes that link to it.
        for (from, targets) in links.iter().enumerate() {
            if !picked.contains(&from) && targets.contains(&current) {
                picked.push(from);
            }
        }
        next += 1;
    }
    picked.into_iter().map(|index| &notes[index]).collect()
}

/// One page an export will write: the note it comes from, what its heading says and every title a
/// `[[link]]` can use to reach it.
pub(super) struct ExportPage {
    pub(super) id: String,
    pub(super) heading: String,
    pub(super) titles: Vec<String>,
    pub(super) body: String,
}

/// An export worked out but not yet written: the pages, the picture files that go beside them and
/// where they all land. Kept so the size can be shown before anything touches the disk.
pub(super) struct ExportPlan {
    pub(super) dir: PathBuf,
    pub(super) whole_vault: bool,
    pub(super) pages: Vec<ExportPage>,
    /// What each page's picture addresses become, indexed like `pages`.
    pub(super) pictures: Vec<HashMap<String, String>>,
    /// Each picture file to copy and the name it goes under beside the pages.
    pub(super) copies: Vec<(PathBuf, String)>,
}

/// What one exported page will do with the pictures it names.
#[derive(Default)]
pub(super) struct PicturePlan {
    /// Files to copy beside the page, each with the name it goes under.
    pub(super) copies: Vec<(PathBuf, String)>,
    /// What each picture address in the note becomes on the page.
    pub(super) mapped: HashMap<String, String>,
}

/// Works out what one exported page will do with its pictures, without writing anything. A picture
/// on this computer is copied beside the page; one on the web is pointed at where it already lives;
/// one that is not on this computer is left out, so the page keeps the words the note was written
/// with instead of a broken picture.
pub(super) fn plan_pictures(
    note_id: &str,
    body: &str,
    note_dir: Option<&Path>,
    number: &mut usize,
) -> PicturePlan {
    let mut plan = PicturePlan::default();
    for line in body.lines() {
        for piece in split_images(line) {
            let LinePiece::Image { src, .. } = piece else {
                continue;
            };
            if plan.mapped.contains_key(&src) {
                continue;
            }
            match resolve(&src, note_dir) {
                Some(ImageRef::Remote(url)) => {
                    plan.mapped.insert(src, url);
                }
                // Only real pictures are copied. A note can point anywhere on this disk, and a
                // shared one was written by somebody else: `![](~/.ssh/id_ed25519)` must not
                // put that key in a folder that is about to be published.
                Some(ImageRef::File(path)) if is_picture_file(&path) => {
                    let Some(name) = path.file_name() else {
                        continue;
                    };
                    let name = export_picture_name(&name.to_string_lossy());
                    // The note id keeps one export's pictures apart from another's, and the
                    // counter apart from each other's inside a single note.
                    let href = format!("pictures/{note_id}-{number}-{name}");
                    *number += 1;
                    plan.copies.push((path, href.clone()));
                    plan.mapped.insert(src, href);
                }
                _ => {}
            }
        }
    }
    plan
}

/// Writes an export plan: the pictures first, then the pages that point at them. Returns what to
/// tell the user, or what went wrong.
pub(super) fn write_export(plan: &ExportPlan) -> Result<String, String> {
    std::fs::create_dir_all(&plan.dir)
        .map_err(|e| format!("could not create {}: {e}", plan.dir.display()))?;
    for (from, href) in &plan.copies {
        let target = plan.dir.join(href);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
        }
        std::fs::copy(from, &target).map_err(|e| {
            format!(
                "could not copy {} to {}: {e}",
                from.display(),
                target.display()
            )
        })?;
    }

    // A wiki link can only open a page that was written in this export, so the pages are the ones
    // in the plan — a single-note export already followed its links to pick those.
    let link = |target: &str| {
        let wanted = target.trim();
        plan.pages
            .iter()
            .find(|page| {
                page.titles
                    .iter()
                    .any(|title| title.eq_ignore_ascii_case(wanted))
            })
            .map(|page| format!("{}.html", page.id))
    };

    for (page, pictures) in plan.pages.iter().zip(&plan.pictures) {
        let picture = |src: &str| pictures.get(src).cloned();
        let html = markdown_to_html_with(
            &page.heading,
            &page.body,
            &ExportTargets {
                link: &link,
                picture: &picture,
            },
        );
        let path = plan.dir.join(format!("{}.html", page.id));
        std::fs::write(&path, html)
            .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    }

    let written = plan.pages.len();
    let summary = if plan.whole_vault {
        format!(
            "Exported {written} {}",
            if written == 1 { "note" } else { "notes" }
        )
    } else if written == 1 {
        "Exported the note".to_string()
    } else {
        let linked = written - 1;
        format!(
            "Exported the note, plus {linked} linked {}",
            if linked == 1 { "note" } else { "notes" }
        )
    };
    let pictures = match plan.copies.len() {
        0 => String::new(),
        1 => " and 1 picture".to_string(),
        pictures => format!(" and {pictures} pictures"),
    };
    Ok(format!("{summary}{pictures} to {}", plan.dir.display()))
}

/// A picture's file name as it can stand in a link and a folder: letters, digits, dashes, dots and
/// underscores stay, everything else becomes a dash.
pub(super) fn export_picture_name(name: &str) -> String {
    let mut safe: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect();
    if safe.trim_matches(|ch| ch == '-' || ch == '.').is_empty() {
        safe = "picture".to_string();
    }
    safe
}
