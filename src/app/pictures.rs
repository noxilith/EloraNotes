//! Pictures in notes: storing, inserting, picking, dropping and sharing them with the library.
use super::*;

/// A "choose a picture" dialog running on a thread of its own.
///
/// The desktop's dialog answers on the file portal's schedule, and the blocking call waits for
/// it. On the window's own thread that stops the event loop for as long as somebody browses
/// their pictures, which is long enough for the desktop to decide the app has hung and offer to
/// close it. Off the thread, the window keeps drawing and the desktop never asks.
pub(super) struct PicturePick {
    /// The note the picture goes into; the person may have moved on by the time it arrives.
    pub(super) note_id: String,
    pub(super) result: Receiver<Option<PathBuf>>,
}

impl EloraApp {
    /// Where pictures dropped or pasted into a note are kept: a folder beside the note files, so
    /// the Markdown can name them with a path relative to the note.
    pub(super) fn attachments_dir(&self) -> PathBuf {
        self.storage.root_dir().join("attachments")
    }

    /// Files a picture away under a name that is safe to put in Markdown (no spaces to end the
    /// address early, no characters a file system dislikes) and returns the path to write into
    /// the note, with the plain file name beside it.
    ///
    /// The name carries a digest of the picture's bytes. That keeps two people's
    /// `screenshot.png` apart in a shared library, makes the same picture file once however
    /// often it is added, and lets a shared note ask the server for exactly the file it names.
    pub(super) fn store_picture(
        &self,
        bytes: &[u8],
        name_hint: &str,
        extension: &str,
    ) -> Result<(String, String), String> {
        let dir = self.attachments_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
        let file = stored_picture_name(bytes, name_hint, extension);
        let path = dir.join(&file);
        // A name that already exists holds these very bytes, so there is nothing to write again.
        if !path.is_file() {
            std::fs::write(&path, bytes).map_err(|e| format!("Could not save the picture: {e}"))?;
        }
        Ok((format!("attachments/{file}"), file))
    }

    /// Puts a picture into a note at the caret: the file is copied into the vault's pictures
    /// folder and a `![name](attachments/…)` line is written where the user is typing.
    pub(super) fn insert_picture(
        &mut self,
        note_idx: usize,
        bytes: &[u8],
        name_hint: &str,
        extension: &str,
    ) -> Result<(), String> {
        let (src, file) = self.store_picture(bytes, name_hint, extension)?;
        let alt = name_hint.trim();
        let alt = if alt.is_empty() {
            "picture".to_string()
        } else {
            alt.to_string()
        };
        let snippet = format!("![{alt}]({src})");
        let note_id = self.notes[note_idx].id.clone();
        self.insert_markdown_at_caret(note_idx, &snippet)?;

        // A shared note is looked at by other people, and they have no access to this disk, so
        // the picture itself goes to the library as well.
        if self.is_remote(&note_id) {
            if bytes.len() > crate::net::MAX_ATTACHMENT {
                self.status_message = Some(format!(
                    "Added {alt} — too large to share with the library (over 12 MB)"
                ));
            } else {
                self.shared_uploads.insert(file.clone());
                self.collab.upload_attachment(&file, bytes.to_vec());
                self.status_message = Some(format!("Added {alt} — sharing it with the library"));
            }
        } else {
            self.status_message = Some(format!(
                "Added {alt} — drag its corner in Preview to resize it"
            ));
        }
        Ok(())
    }

    /// Stores a picture the library sent, where the preview looks for it. Only a plain picture
    /// name is accepted, so a picture can never be written outside the pictures folder, and a
    /// file that is already here is left alone.
    pub(super) fn save_shared_picture(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        if !crate::net::valid_attachment_name(name) {
            return Err(format!("{name} is not a picture name"));
        }
        if bytes.is_empty() || bytes.len() > crate::net::MAX_ATTACHMENT {
            return Err(format!("{name} is not a usable picture"));
        }
        let dir = self.attachments_dir();
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
        let path = dir.join(name);
        if path.is_file() {
            return Ok(());
        }
        std::fs::write(&path, bytes).map_err(|e| format!("Could not save {name}: {e}"))
    }

    /// Asks the library for the pictures a shared note names that this computer has not got.
    /// Each is asked for once per connection; when a picture arrives it is written into the
    /// vault and the preview draws it on the next frame.
    pub(super) fn request_shared_pictures(&mut self, body: &str) {
        for name in picture_references(body) {
            if self.attachments_dir().join(&name).is_file() {
                continue;
            }
            if !self.requested_pictures.insert(name.clone()) {
                continue;
            }
            self.collab.fetch_attachment(&name);
        }
    }

    /// Sends the pictures a note names to the library, for when a local note is moved there.
    pub(super) fn upload_pictures(&mut self, body: &str) {
        for name in picture_references(body) {
            let path = self.attachments_dir().join(&name);
            match std::fs::read(&path) {
                Ok(bytes) => self.collab.upload_attachment(&name, bytes),
                Err(e) => {
                    self.status_message = Some(format!("Could not share {}: {e}", path.display()));
                    return;
                }
            }
        }
    }

    /// Adds a picture that is already on this computer, chosen from the toolbar's picture
    /// button: the file is read, filed away next to the notes and written into the note.
    pub(super) fn add_picture_file(&mut self, note_idx: usize, path: &Path) -> Result<(), String> {
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !is_picture_extension(&extension) {
            return Err(format!(
                "{} is not a picture EloraNotes can show (PNG, JPEG and GIF)",
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("That file")
            ));
        }
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(str::to_string)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| "picture".to_string());
        let bytes =
            std::fs::read(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
        self.insert_picture(note_idx, &bytes, &name, &extension)
    }

    /// The toolbar's picture button: the system's own file dialog, starting where pictures
    /// usually are, then the chosen file goes into the note like a dropped one.
    pub(super) fn pick_picture(&mut self, ctx: &egui::Context, note_idx: usize) {
        if self.picture_pick.is_some() {
            self.status_message = Some("A picture is already being chosen".to_string());
            return;
        }
        self.picture_pick = Some(PicturePick {
            note_id: self.notes[note_idx].id.clone(),
            result: choose_picture_file(ctx),
        });
    }

    /// Picks up the file dialog's answer, if it has one by now, and adds the picture to the note
    /// it was opened from. Called every frame, because the answer arrives between frames.
    pub(super) fn poll_picture_pick(&mut self) {
        let Some(pick) = self.picture_pick.as_ref() else {
            return;
        };
        let note_id = pick.note_id.clone();
        let answer = pick.result.try_recv();
        match answer {
            Ok(Some(path)) => {
                self.picture_pick = None;
                match self.notes.iter().position(|note| note.id == note_id) {
                    Some(note_idx) => {
                        if let Err(e) = self.add_picture_file(note_idx, &path) {
                            self.status_message = Some(e);
                        }
                    }
                    // The note was closed, copied away or the server dropped it while the dialog
                    // was open; there is nowhere to put the picture now.
                    None => {
                        self.status_message =
                            Some("That note is gone, so the picture was not added".to_string())
                    }
                }
            }
            Ok(None) => self.picture_pick = None,
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => self.picture_pick = None,
        }
    }

    /// Inserts a block on lines of its own at the caret (replacing the selection, when there is
    /// one) and leaves the caret after it.
    pub(super) fn insert_markdown_at_caret(
        &mut self,
        note_idx: usize,
        snippet: &str,
    ) -> Result<(), String> {
        let note_id = self.notes[note_idx].id.clone();
        let body = self.notes[note_idx].body.clone();
        let length = body.chars().count();
        // The caret only means something for the note the editor is actually showing; after a
        // switch (or from the list) the picture goes at the end instead of into an old position.
        let editor_showing_note = self
            .last_seen_body
            .as_ref()
            .is_some_and(|(seen, _)| seen == &note_id);
        let (start_char, end_char) = match editor_showing_note {
            true => self
                .last_editor_selection
                .map(|(start, end)| (start.min(length), end.min(length)))
                .unwrap_or_else(|| {
                    let caret = self.last_editor_caret.unwrap_or(length).min(length);
                    (caret, caret)
                }),
            false => (length, length),
        };
        let (start_char, end_char) = (start_char.min(end_char), end_char.max(start_char));

        let byte_at = |char_index: usize| {
            body.char_indices()
                .nth(char_index)
                .map(|(byte, _)| byte)
                .unwrap_or(body.len())
        };
        let (start_byte, end_byte) = (byte_at(start_char), byte_at(end_char));
        let before = &body[..start_byte];
        let after = &body[end_byte..];

        let mut inserted = String::new();
        if !before.is_empty() && !before.ends_with('\n') {
            inserted.push('\n');
        }
        inserted.push_str(snippet);
        let caret = start_char + inserted.chars().count();
        if !after.starts_with('\n') {
            inserted.push('\n');
        }

        self.notes[note_idx].body = format!("{before}{inserted}{after}");
        self.notes[note_idx].updated_at = Utc::now();
        self.last_editor_selection = Some((caret, caret));
        self.last_editor_caret = Some(caret);
        self.caret_after_insert = Some((note_id, caret));
        self.persist_note(note_idx)
    }

    /// Pictures dropped onto the window, added to the open note in the order they arrived.
    pub(super) fn add_dropped_pictures(&mut self, files: Vec<egui::DroppedFile>) {
        let Some(note_idx) = self
            .selected_id
            .as_ref()
            .and_then(|id| self.notes.iter().position(|note| &note.id == id))
        else {
            self.status_message = Some("Open a note before dropping a picture into it".to_string());
            return;
        };

        let mut added = 0usize;
        let mut problems: Vec<String> = Vec::new();
        for file in files {
            let result = match (file.path.as_ref(), file.bytes) {
                // Some desktops hand over the file's contents instead of, or as well as, its
                // path; those do not need to be read again.
                (_, Some(bytes)) => {
                    let name = file
                        .path
                        .as_ref()
                        .and_then(|path| path.file_stem())
                        .and_then(|stem| stem.to_str())
                        .map(str::to_string)
                        .or_else(|| Some(file.name.clone()))
                        .filter(|name| !name.trim().is_empty())
                        .unwrap_or_else(|| "picture".to_string());
                    let extension = file
                        .path
                        .as_ref()
                        .and_then(|path| path.extension())
                        .and_then(|extension| extension.to_str())
                        .unwrap_or("png")
                        .to_ascii_lowercase();
                    match is_picture_extension(&extension) {
                        true => self.insert_picture(note_idx, &bytes, &name, &extension),
                        false => Err(format!(
                            "{}: {extension} pictures are not supported",
                            file.name
                        )),
                    }
                }
                (Some(path), None) => self.add_picture_file(note_idx, path),
                (None, None) => Err(format!("{}: could not be read", file.name)),
            };
            match result {
                Ok(()) => added += 1,
                Err(e) => problems.push(e),
            }
        }

        self.status_message = match (added, problems.first()) {
            (0, Some(problem)) => Some(problem.clone()),
            (0, None) => None,
            (count, None) => Some(format!(
                "Added {count} picture{}",
                if count == 1 { "" } else { "s" }
            )),
            (count, Some(problem)) => Some(format!(
                "Added {count} picture{}; {problem}",
                if count == 1 { "" } else { "s" }
            )),
        };
    }
}

/// A file name for a stored picture: lower case letters and digits, with everything else folded
/// into single dashes, so the Markdown address never ends at a space.
pub(super) fn picture_name(hint: &str) -> String {
    let mut name = String::new();
    for ch in hint.chars() {
        if ch.is_ascii_alphanumeric() {
            name.push(ch.to_ascii_lowercase());
        } else if !name.is_empty() && !name.ends_with('-') {
            name.push('-');
        }
        if name.len() >= 48 {
            break;
        }
    }
    let name = name.trim_matches('-').to_string();
    if name.is_empty() {
        "picture".to_string()
    } else {
        name
    }
}

/// The formats the preview can decode; anything else is stored as PNG instead.
pub(super) fn picture_extension(extension: &str) -> String {
    let extension = extension
        .trim()
        .trim_start_matches('.')
        .to_ascii_lowercase();
    match extension.as_str() {
        "png" | "jpg" | "jpeg" | "gif" => extension,
        _ => "png".to_string(),
    }
}

/// The name a picture is filed under: the name it was given, folded down to safe characters,
/// with a short digest of its bytes, so the same picture always lands on the same file and two
/// different pictures never can.
pub(super) fn stored_picture_name(bytes: &[u8], name_hint: &str, extension: &str) -> String {
    let stem = picture_name(name_hint);
    let extension = picture_extension(extension);
    let digest = Sha256::digest(bytes);
    let tag: String = digest
        .iter()
        .take(4)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{stem}-{tag}.{extension}")
}

/// The pictures a note names with `![alt](attachments/…)`, as plain file names. Anything that is
/// not a plain picture name is ignored: a note can only ask the library for what it could keep.
pub(super) fn picture_references(body: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for line in body.lines() {
        for piece in split_images(line) {
            let LinePiece::Image { src, .. } = piece else {
                continue;
            };
            let Some(name) = src.trim().strip_prefix("attachments/") else {
                continue;
            };
            if crate::net::valid_attachment_name(name) {
                names.insert(name.to_string());
            }
        }
    }
    names
}

/// A file with a picture's extension that also starts like a PNG, JPEG or GIF.
pub(super) fn is_picture_file(path: &Path) -> bool {
    use std::io::Read;
    let named_like_one = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(is_picture_extension);
    if !named_like_one || !path.is_file() {
        return false;
    }
    let mut header = [0u8; 8];
    let Ok(read) = std::fs::File::open(path).and_then(|mut f| f.read(&mut header)) else {
        return false;
    };
    let header = &header[..read];
    header.starts_with(b"\x89PNG\r\n\x1a\n")
        || header.starts_with(&[0xFF, 0xD8, 0xFF])
        || header.starts_with(b"GIF87a")
        || header.starts_with(b"GIF89a")
}

pub(super) fn is_picture_extension(extension: &str) -> bool {
    matches!(
        extension
            .trim_start_matches('.')
            .to_ascii_lowercase()
            .as_str(),
        "png" | "jpg" | "jpeg" | "gif"
    )
}

/// The picture on the clipboard, encoded as PNG, when this desktop offers one. Wayland sessions
/// that do not bridge the clipboard to X11 simply report nothing here, and text pasting is left
/// to the editor as usual.
pub(super) fn clipboard_picture() -> Option<(Vec<u8>, String)> {
    let mut clipboard = arboard::Clipboard::new().ok()?;
    let image = clipboard.get_image().ok()?;
    let rgba = image::RgbaImage::from_raw(
        u32::try_from(image.width).ok()?,
        u32::try_from(image.height).ok()?,
        image.bytes.into_owned(),
    )?;
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(rgba)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some((png, "screenshot".to_string()))
}

/// Opens the system's "choose a picture" dialog on a thread of its own and hands back the
/// channel its answer arrives on, so the window is never blocked waiting for it.
pub(super) fn choose_picture_file(ctx: &egui::Context) -> Receiver<Option<PathBuf>> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let wake = ctx.clone();
    std::thread::spawn(move || {
        let mut dialog = rfd::FileDialog::new()
            .set_title("Add a picture to this note")
            .add_filter("Pictures", &["png", "jpg", "jpeg", "gif"]);
        // Start where pictures usually are, but only if that folder really exists: a portal
        // asked for a missing folder is left showing nothing useful.
        if let Some(start) = dirs::picture_dir()
            .filter(|dir| dir.is_dir())
            .or_else(|| dirs::home_dir().filter(|dir| dir.is_dir()))
        {
            dialog = dialog.set_directory(start);
        }
        let chosen = dialog.pick_file();
        let _ = sender.send(chosen);
        // A window that is not asked to draw would not see the answer until something else
        // happened to repaint it.
        wake.request_repaint();
    });
    receiver
}
