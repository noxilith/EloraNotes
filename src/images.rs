//! Pictures inside notes, rendered the way GitHub renders `![alt](path)`: paths are relative to
//! the note's own folder, and an `http(s)` address is downloaded in the background once and then
//! kept as a texture.

use crate::theme::Palette;
use egui::{
    load::SizedTexture, Context, CornerRadius, Frame, Margin, RichText, Stroke, TextureHandle,
    TextureOptions, Ui,
};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

/// A picture bigger than this is not decoded: a note should never be able to exhaust memory.
const MAX_BYTES: u64 = 32 * 1024 * 1024;
/// Textures kept around after switching notes. Comfortably more than any one note shows.
const MAX_CACHED: usize = 128;

/// Where an image in a note points.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImageRef {
    /// A file on this computer, resolved next to the note.
    File(PathBuf),
    /// An `http(s)` address, downloaded while the preview is open.
    Remote(String),
}

impl ImageRef {
    /// Cache identity: the resolved file and its modification time, so editing a picture on disk
    /// shows the new version instead of the one already in memory.
    fn cache_key(&self) -> String {
        match self {
            ImageRef::File(path) => {
                let stamp = std::fs::metadata(path)
                    .and_then(|meta| meta.modified())
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|age| age.as_millis())
                    .unwrap_or(0);
                format!("file:{}:{stamp}", path.display())
            }
            ImageRef::Remote(url) => format!("url:{url}"),
        }
    }
}

/// Resolves the `src` of a Markdown image. Relative paths are taken from `note_dir`, the way a
/// file in a GitHub repository is resolved; absolute paths, `file://` URLs, `~/` paths and
/// `http(s)` links are used as they are.
pub fn resolve(src: &str, note_dir: Option<&Path>) -> Option<ImageRef> {
    let src = src.trim();
    let src = src
        .strip_prefix('<')
        .and_then(|inner| inner.strip_suffix('>'))
        .unwrap_or(src)
        .trim();
    if src.is_empty() {
        return None;
    }
    let lower = src.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(ImageRef::Remote(src.to_string()));
    }
    let decoded = decode_percent(src);
    let path = if let Some(rest) = decoded.strip_prefix("file://") {
        PathBuf::from(rest)
    } else if let Some(rest) = decoded.strip_prefix("~/") {
        dirs::home_dir()?.join(rest)
    } else {
        let path = PathBuf::from(&decoded);
        if path.is_absolute() {
            path
        } else {
            note_dir?.join(path)
        }
    };
    Some(ImageRef::File(path))
}

/// Turns `%20`-style escapes back into characters, so a link copied from a browser still finds
/// the file it names.
fn decode_percent(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(value) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(value);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| text.to_string())
}

/// Pictures shown in the preview, with the textures already decoded for this window.
#[derive(Default)]
pub struct NoteImages {
    textures: HashMap<String, Option<TextureHandle>>,
    downloads: HashMap<String, Receiver<Result<Vec<u8>, String>>>,
}

impl NoteImages {
    /// Draws one image, with a compact placeholder while a download is running or when the
    /// picture cannot be read. Never fails loudly: a broken link is a line of muted text.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        src: &str,
        alt: &str,
        note_dir: Option<&Path>,
        p: &Palette,
    ) {
        self.collect(ui.ctx());

        let Some(reference) = resolve(src, note_dir) else {
            placeholder(
                ui,
                crate::icons::icon::PHOTO_OFF,
                alt,
                src,
                p,
                "No folder to look in for this picture",
            );
            return;
        };
        let key = reference.cache_key();

        if !self.textures.contains_key(&key) {
            match &reference {
                ImageRef::File(path) => {
                    let texture = std::fs::read(path)
                        .ok()
                        .and_then(|bytes| decode(ui.ctx(), &key, &bytes));
                    self.store(key.clone(), texture);
                }
                ImageRef::Remote(url) => {
                    if !self.downloads.contains_key(&key) {
                        self.start_download(ui.ctx(), key.clone(), url.clone());
                    }
                    placeholder(
                        ui,
                        crate::icons::icon::PHOTO,
                        alt,
                        src,
                        p,
                        "Downloading this picture…",
                    );
                    return;
                }
            }
        }

        match self.textures.get(&key) {
            Some(Some(texture)) => {
                let sized = SizedTexture::new(texture.id(), texture.size_vec2());
                let response = ui.add(
                    egui::Image::new(sized)
                        .max_width(ui.available_width())
                        .maintain_aspect_ratio(true)
                        .corner_radius(CornerRadius::same(4)),
                );
                if !alt.is_empty() {
                    response.on_hover_text(alt);
                }
            }
            _ => placeholder(
                ui,
                crate::icons::icon::PHOTO_OFF,
                alt,
                src,
                p,
                "This picture could not be read",
            ),
        }
    }

    /// How many pictures are ready to draw. Used by the tests to prove a preview really decoded
    /// an embedded picture rather than leaving a placeholder.
    #[cfg(test)]
    pub(crate) fn ready_pictures(&self) -> usize {
        self.textures
            .values()
            .filter(|texture| texture.is_some())
            .count()
    }

    fn store(&mut self, key: String, texture: Option<TextureHandle>) {
        if self.textures.len() >= MAX_CACHED {
            self.textures.clear();
        }
        self.textures.insert(key, texture);
    }

    /// Moves finished downloads into the texture cache.
    fn collect(&mut self, ctx: &Context) {
        if self.downloads.is_empty() {
            return;
        }
        let mut finished: Vec<(String, Option<Vec<u8>>)> = Vec::new();
        for (key, receiver) in &self.downloads {
            match receiver.try_recv() {
                Ok(Ok(bytes)) => finished.push((key.clone(), Some(bytes))),
                Ok(Err(_)) => finished.push((key.clone(), None)),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => finished.push((key.clone(), None)),
            }
        }
        for (key, bytes) in finished {
            self.downloads.remove(&key);
            let texture = bytes.and_then(|bytes| decode(ctx, &key, &bytes));
            self.store(key, texture);
        }
    }

    /// Fetches an `http(s)` picture off the UI thread, asking for a frame when the bytes arrive.
    fn start_download(&mut self, ctx: &Context, key: String, url: String) {
        let (sender, receiver) = mpsc::channel();
        let wake = ctx.clone();
        std::thread::spawn(move || {
            let result = fetch(&url);
            let _ = sender.send(result);
            wake.request_repaint();
        });
        self.downloads.insert(key, receiver);
    }
}

fn fetch(url: &str) -> Result<Vec<u8>, String> {
    let response = ureq::get(url)
        .timeout(Duration::from_secs(15))
        .call()
        .map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err("the picture is too large".to_string());
    }
    Ok(bytes)
}

/// Decodes the common formats a note points at: PNG, JPEG and GIF.
fn decode(ctx: &Context, key: &str, bytes: &[u8]) -> Option<TextureHandle> {
    let image = image::load_from_memory(bytes).ok()?;
    let rgba = image.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    if size[0] == 0 || size[1] == 0 {
        return None;
    }
    let color = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    Some(ctx.load_texture(key, color, TextureOptions::LINEAR))
}

/// The quiet line that stands in for a picture that is loading or missing.
fn placeholder(ui: &mut Ui, glyph: &str, alt: &str, src: &str, p: &Palette, note: &str) {
    let label = if alt.trim().is_empty() { src } else { alt };
    Frame::new()
        .fill(p.bg_elevated)
        .stroke(Stroke::new(1.0_f32, p.border))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(glyph).size(14.0).color(p.fg_muted));
                ui.label(RichText::new(label).size(12.0).color(p.fg_secondary));
            });
        })
        .response
        .on_hover_text(format!("{note}\n{src}"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::ThemeMode;
    use std::io::Cursor;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "elora-images-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn png_bytes() -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(4, 3, image::Rgba([200, 80, 40, 255]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    #[test]
    fn test_resolve_relative_absolute_and_remote() {
        let dir = Path::new("/notes");
        assert_eq!(
            resolve("pics/cat.png", Some(dir)),
            Some(ImageRef::File(PathBuf::from("/notes/pics/cat.png")))
        );
        assert_eq!(
            resolve("/tmp/cat.png", Some(dir)),
            Some(ImageRef::File(PathBuf::from("/tmp/cat.png")))
        );
        assert_eq!(
            resolve("https://example.com/cat.png", Some(dir)),
            Some(ImageRef::Remote("https://example.com/cat.png".to_string()))
        );
        assert_eq!(
            resolve("my%20cat.png", Some(dir)),
            Some(ImageRef::File(PathBuf::from("/notes/my cat.png")))
        );
        assert_eq!(
            resolve("<my cat.png>", Some(dir)),
            Some(ImageRef::File(PathBuf::from("/notes/my cat.png")))
        );
        // With no folder to resolve against, a relative path cannot be shown.
        assert_eq!(resolve("cat.png", None), None);
    }

    #[test]
    fn test_local_picture_is_decoded_and_cached() {
        let dir = temp_dir("local");
        let path = dir.join("pixel.png");
        std::fs::write(&path, png_bytes()).unwrap();

        let ctx = Context::default();
        let mut images = NoteImages::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(480.0, 320.0),
            )),
            ..Default::default()
        };
        let src = "pixel.png".to_string();
        let note_dir = dir.clone();
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                images.show(
                    ui,
                    &src,
                    "a pixel",
                    Some(&note_dir),
                    &ThemeMode::Obsidian.palette(),
                );
            });
        });

        let key = ImageRef::File(path.clone()).cache_key();
        assert!(matches!(images.textures.get(&key), Some(Some(_))));
        // A second frame reuses the texture instead of decoding the file again.
        let cached = images.textures.len();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(480.0, 320.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                images.show(
                    ui,
                    &src,
                    "a pixel",
                    Some(&note_dir),
                    &ThemeMode::Obsidian.palette(),
                );
            });
        });
        assert_eq!(images.textures.len(), cached);
    }

    #[test]
    fn test_missing_picture_shows_a_placeholder() {
        let dir = temp_dir("missing");
        let ctx = Context::default();
        let mut images = NoteImages::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(480.0, 320.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                images.show(
                    ui,
                    "gone.png",
                    "gone",
                    Some(&dir),
                    &ThemeMode::Obsidian.palette(),
                );
            });
        });
        assert_eq!(images.textures.len(), 1);
        assert!(images.textures.values().all(Option::is_none));
    }

    #[test]
    fn test_decode_rejects_unknown_bytes() {
        let ctx = Context::default();
        assert!(decode(&ctx, "not-an-image", b"definitely not a picture").is_none());
    }
}
