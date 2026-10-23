//! Pictures inside notes, rendered the way GitHub renders `![alt](path)`: paths are relative to
//! the note's own folder, and an `http(s)` address is downloaded in the background once and then
//! kept as a texture.

use crate::theme::Palette;
use egui::{
    load::SizedTexture, Color32, Context, CornerRadius, CursorIcon, Frame, Margin, RichText, Sense,
    Stroke, TextureHandle, TextureOptions, Ui, Vec2,
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
/// A picture dragged smaller than this would be a speck, so the handle stops there.
const MIN_WIDTH: f32 = 48.0;
/// Side of the square that sits in a picture's bottom-right corner to resize it.
const HANDLE: f32 = 13.0;

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

/// A size a picture is being dragged to, before the drag is written into the note.
struct Resizing {
    key: String,
    /// Width when the drag started, so the handle follows the pointer from where it was grabbed.
    start: f32,
    width: f32,
}

/// Pictures shown in the preview, with the textures already decoded for this window.
#[derive(Default)]
pub struct NoteImages {
    textures: HashMap<String, Option<TextureHandle>>,
    downloads: HashMap<String, Receiver<Result<Vec<u8>, String>>>,
    resizing: Option<Resizing>,
    /// Where each picture and its corner handle were drawn, so a test can grab the one it means.
    #[cfg(test)]
    pub(crate) handles: Vec<(String, egui::Rect)>,
    #[cfg(test)]
    pub(crate) drawn: Vec<(String, egui::Rect)>,
}

impl NoteImages {
    /// Draws one image, with a compact placeholder while a download is running or when the
    /// picture cannot be read. Never fails loudly: a broken link is a line of muted text.
    ///
    /// `width` is the size the note asks for, and a drag on the square in the picture's corner
    /// changes it while the picture follows the pointer. Letting go returns the width to keep;
    /// the caller writes it into the note, so the size survives a restart.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        src: &str,
        alt: &str,
        width: Option<u32>,
        note_dir: Option<&Path>,
        p: &Palette,
    ) -> Option<u32> {
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
            return None;
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
                    return None;
                }
            }
        }

        let texture = match self.textures.get(&key) {
            Some(Some(texture)) => texture.clone(),
            _ => {
                placeholder(
                    ui,
                    crate::icons::icon::PHOTO_OFF,
                    alt,
                    src,
                    p,
                    "This picture could not be read",
                );
                return None;
            }
        };

        let natural = texture.size_vec2();
        let available = ui.available_width().max(MIN_WIDTH);
        let dragged = self
            .resizing
            .as_ref()
            .filter(|resizing| resizing.key == key)
            .map(|resizing| resizing.width);
        let shown = dragged
            .or_else(|| width.map(|width| width as f32))
            .unwrap_or(natural.x);
        let size = picture_size(natural, shown, available);

        let response = ui.add(
            egui::Image::new(SizedTexture::new(texture.id(), natural))
                .fit_to_exact_size(size)
                .corner_radius(CornerRadius::same(4)),
        );
        if !alt.is_empty() {
            response.clone().on_hover_text(alt);
        }

        // The corner square: drag it and the picture grows or shrinks with the pointer, the way
        // a picture on a OneNote page does.
        let handle = egui::Rect::from_min_size(
            response.rect.right_bottom() - Vec2::splat(HANDLE),
            Vec2::splat(HANDLE),
        );
        #[cfg(test)]
        {
            self.handles.push((key.clone(), handle));
            self.drawn.push((key.clone(), response.rect));
        }
        let grab = ui.interact(handle, response.id.with("resize"), Sense::drag());
        let hovering = response.hovered() || grab.hovered() || grab.dragged();
        if hovering || grab.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeNwSe);
        }

        if grab.drag_started() {
            self.resizing = Some(Resizing {
                key: key.clone(),
                start: size.x,
                width: size.x,
            });
        }
        if let Some(resizing) = self.resizing.as_mut().filter(|r| r.key == key) {
            if grab.dragged() {
                resizing.width = (resizing.start + grab.drag_delta().x).clamp(MIN_WIDTH, available);
            }
        }
        if grab.drag_stopped() {
            if let Some(resizing) = self.resizing.take() {
                let kept = resizing.width.round().max(MIN_WIDTH) as u32;
                if width != Some(kept) {
                    return Some(kept);
                }
            }
        }

        if hovering {
            paint_handle(ui, handle, p, self.resizing.is_some());
        }
        if let Some(resizing) = self.resizing.as_ref().filter(|r| r.key == key) {
            let label = format!("{} px", resizing.width.round() as u32);
            let galley = ui.fonts(|fonts| {
                fonts.layout_no_wrap(label, egui::FontId::proportional(10.0), p.fg_primary)
            });
            let chip = egui::Rect::from_min_size(
                egui::Pos2::new(handle.left() - galley.size().x - 14.0, handle.top() - 2.0),
                galley.size() + Vec2::new(8.0, 4.0),
            );
            let painter = ui.painter();
            painter.rect_filled(chip, CornerRadius::same(4), p.bg_elevated);
            painter.galley(chip.min + Vec2::new(4.0, 2.0), galley, p.fg_primary);
        }

        None
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

/// How big a picture is drawn: the width the note asks for (or its own width), kept inside the
/// column and never smaller than the handle allows, with the height following the picture's
/// proportions.
fn picture_size(natural: Vec2, requested: f32, available: f32) -> Vec2 {
    let available = available.max(MIN_WIDTH);
    let width = requested.clamp(MIN_WIDTH, available);
    let height = (width * natural.y / natural.x.max(1.0)).max(1.0);
    Vec2::new(width, height)
}

/// The square in a picture's corner, with a small diagonal grip so it reads as a handle and not
/// as part of the picture.
fn paint_handle(ui: &Ui, rect: egui::Rect, p: &Palette, dragging: bool) {
    let painter = ui.painter();
    let fill = if dragging { p.accent } else { p.fg_secondary };
    painter.rect_filled(rect.shrink(2.0), CornerRadius::same(3), fill);
    let ink = if fill.r() as u32 * 3 + fill.g() as u32 * 5 + fill.b() as u32 * 2 < 900 {
        Color32::WHITE
    } else {
        p.bg_canvas
    };
    let grip = rect.shrink(5.5);
    painter.line_segment(
        [grip.left_bottom(), grip.right_top()],
        Stroke::new(1.2_f32, ink),
    );
    painter.line_segment(
        [
            grip.left_bottom() + Vec2::new(0.0, -2.5),
            grip.right_top() + Vec2::new(-2.5, 0.0),
        ],
        Stroke::new(1.2_f32, ink),
    );
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
        png_of(4, 3)
    }

    fn png_of(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba([200, 80, 40, 255]));
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
                    None,
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
                    None,
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
                    None,
                    Some(&dir),
                    &ThemeMode::Obsidian.palette(),
                );
            });
        });
        assert_eq!(images.textures.len(), 1);
        assert!(images.textures.values().all(Option::is_none));
    }

    /// The picture's corner handle is a real drag: grab it, move it and let go, and the width it
    /// was dragged to comes back so the note can keep it.
    #[test]
    fn test_dragging_the_corner_handle_returns_the_new_width() {
        let dir = temp_dir("resize");
        std::fs::write(dir.join("wide.png"), png_of(400, 200)).unwrap();
        let ctx = Context::default();
        let mut images = NoteImages::default();
        let mut kept: Option<u32> = None;
        let mut handle = egui::Pos2::ZERO;

        let frame = |events: Vec<egui::Event>,
                     images: &mut NoteImages,
                     kept: &mut Option<u32>,
                     handle: &mut egui::Pos2| {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(640.0, 480.0),
                )),
                events,
                ..Default::default()
            };
            let _ = ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let origin = ui.next_widget_position();
                    if let Some(width) = images.show(
                        ui,
                        "wide.png",
                        "a wide picture",
                        None,
                        Some(&dir),
                        &ThemeMode::Obsidian.palette(),
                    ) {
                        *kept = Some(width);
                    }
                    // The picture is drawn at its own 400 × 200 size, so its corner — where the
                    // handle sits — is that far from where it started.
                    *handle = origin + Vec2::new(400.0, 200.0) - Vec2::splat(HANDLE / 2.0);
                });
            });
        };

        // Let the picture be drawn once, then grab its handle and pull it 60 points right.
        frame(Vec::new(), &mut images, &mut kept, &mut handle);
        frame(
            vec![
                egui::Event::PointerMoved(handle),
                egui::Event::PointerButton {
                    pos: handle,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
            ],
            &mut images,
            &mut kept,
            &mut handle,
        );
        let moved_to = handle + Vec2::new(60.0, 0.0);
        frame(
            vec![egui::Event::PointerMoved(moved_to)],
            &mut images,
            &mut kept,
            &mut handle,
        );
        assert_eq!(kept, None, "nothing is written until the drag is released");
        frame(
            vec![egui::Event::PointerButton {
                pos: moved_to,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
            &mut images,
            &mut kept,
            &mut handle,
        );

        assert_eq!(
            kept,
            Some(460),
            "the picture should ask for the width it was dragged to"
        );
    }

    #[test]
    fn test_a_picture_is_drawn_at_the_width_the_note_asks_for() {
        let dir = temp_dir("width");
        std::fs::write(dir.join("wide.png"), png_of(400, 200)).unwrap();
        let ctx = Context::default();
        let mut images = NoteImages::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(640.0, 480.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                images.show(
                    ui,
                    "wide.png",
                    "a wide picture",
                    Some(120),
                    Some(&dir),
                    &ThemeMode::Obsidian.palette(),
                );
            });
        });

        let (_, rect) = images.drawn.first().expect("the picture was drawn");
        assert!(
            (rect.width() - 120.0).abs() < 0.5,
            "drawn {} wide",
            rect.width()
        );
        assert!(
            (rect.height() - 60.0).abs() < 0.5,
            "the height keeps the picture's proportions: {}",
            rect.height()
        );
    }

    #[test]
    fn test_a_picture_keeps_its_shape_at_the_size_it_was_dragged_to() {
        // The note's width wins over the picture's own size, and the height follows it.
        assert_eq!(
            picture_size(Vec2::new(400.0, 200.0), 100.0, 600.0),
            Vec2::new(100.0, 50.0)
        );
        // A picture wider than the column is held to the column.
        assert_eq!(
            picture_size(Vec2::new(1200.0, 600.0), 1200.0, 500.0),
            Vec2::new(500.0, 250.0)
        );
        // It never becomes a speck, however far the handle is dragged inwards.
        assert_eq!(
            picture_size(Vec2::new(400.0, 200.0), 4.0, 600.0).x,
            MIN_WIDTH
        );
    }

    #[test]
    fn test_decode_rejects_unknown_bytes() {
        let ctx = Context::default();
        assert!(decode(&ctx, "not-an-image", b"definitely not a picture").is_none());
    }
}
