use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Stroke, Style, TextStyle, Visuals};
use serde::{Deserialize, Serialize};
use std::sync::RwLock;

/// Palette read from the file matugen writes (see `matugen/elora.json`). `None` until loaded.
static MATUGEN_PALETTE: RwLock<Option<Palette>> = RwLock::new(None);

/// Where matugen should write the generated colors: `<config dir>/elora/matugen.json`.
///
/// A Flatpak redirects the config dir into the sandbox (`~/.var/app/.../config`), where matugen
/// never writes, so the file at the real `~/.config/elora/matugen.json` is used as well. The
/// bundle mounts that folder read-only.
pub fn matugen_colors_path() -> Option<std::path::PathBuf> {
    let in_config_dir = dirs::config_dir().map(|d| d.join("elora").join("matugen.json"));
    let in_home = dirs::home_dir().map(|h| h.join(".config").join("elora").join("matugen.json"));
    let existing = in_config_dir
        .iter()
        .chain(in_home.iter())
        .find(|path| path.is_file())
        .cloned();
    existing.or(in_config_dir.or(in_home))
}

fn parse_hex(s: &str) -> Option<Color32> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 || !s.is_ascii() {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Color32::from_rgb(r, g, b))
}

/// Reads matugen's Material 3 colors into the palette. Keeps the last good palette when the file
/// is missing or invalid, and fills any missing role from the Obsidian theme.
pub fn reload_matugen_palette() {
    let Some(path) = matugen_colors_path() else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    let base = ThemeMode::Obsidian.palette();
    let get = |key: &str, fallback: Color32| {
        v.get(key)
            .and_then(|x| x.as_str())
            .and_then(parse_hex)
            .unwrap_or(fallback)
    };
    let palette = Palette {
        bg_canvas: get("surface", base.bg_canvas),
        bg_sidebar: get("surface_container_low", base.bg_sidebar),
        bg_elevated: get("surface_container", base.bg_elevated),
        bg_selected: get("surface_container_highest", base.bg_selected),
        bg_hover: get("surface_container_high", base.bg_hover),
        fg_primary: get("on_surface", base.fg_primary),
        fg_secondary: get("on_surface_variant", base.fg_secondary),
        fg_muted: get("outline", base.fg_muted),
        accent: get("primary", base.accent),
        accent_soft: get("primary_container", base.accent_soft),
        pin_color: get("tertiary", base.pin_color),
        danger: get("error", base.danger),
        border: get("outline_variant", base.border),
        glow_strength: base.glow_strength,
    };
    if let Ok(mut slot) = MATUGEN_PALETTE.write() {
        *slot = Some(palette);
    }
}

fn matugen_palette() -> Palette {
    MATUGEN_PALETTE
        .read()
        .ok()
        .and_then(|p| *p)
        .unwrap_or_else(|| ThemeMode::Obsidian.palette())
}

/// User-chosen color overrides layered on top of the selected theme. `None` keeps the theme's color.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ColorOverrides {
    #[serde(default)]
    pub bg: Option<[u8; 3]>,
    #[serde(default)]
    pub sidebar: Option<[u8; 3]>,
    #[serde(default)]
    pub fg: Option<[u8; 3]>,
    #[serde(default)]
    pub accent: Option<[u8; 3]>,
    #[serde(default)]
    pub pin: Option<[u8; 3]>,
    #[serde(default)]
    pub fg_muted: Option<[u8; 3]>,
    #[serde(default)]
    pub border: Option<[u8; 3]>,
    #[serde(default)]
    pub danger: Option<[u8; 3]>,
}

fn rgb_or(value: Option<[u8; 3]>, fallback: Color32) -> Color32 {
    match value {
        Some([r, g, b]) => Color32::from_rgb(r, g, b),
        None => fallback,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    Obsidian,
    Paper,
    Nordic,
    Matcha,
    Sepia,
    Midnight,
    Matugen,
}

#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub bg_canvas: Color32,
    pub bg_sidebar: Color32,
    pub bg_elevated: Color32,
    pub bg_selected: Color32,
    pub bg_hover: Color32,
    pub fg_primary: Color32,
    pub fg_secondary: Color32,
    pub fg_muted: Color32,
    pub accent: Color32,
    pub accent_soft: Color32,
    pub pin_color: Color32,
    pub danger: Color32,
    pub border: Color32,
    /// Intensity of the glow halo around highlights, 0.0 (off) to 1.0 (strong).
    pub glow_strength: f32,
}

impl ThemeMode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Obsidian => "Obsidian",
            Self::Paper => "Paper",
            Self::Nordic => "Nordic",
            Self::Matcha => "Matcha",
            Self::Sepia => "Sepia",
            Self::Midnight => "Midnight",
            Self::Matugen => "Matugen",
        }
    }

    pub fn from_name(name: &str) -> Self {
        match name {
            "Paper" | "Light" => Self::Paper,
            "Nordic" => Self::Nordic,
            "Matcha" => Self::Matcha,
            "Sepia" => Self::Sepia,
            "Midnight" => Self::Midnight,
            "Matugen" => Self::Matugen,
            _ => Self::Obsidian,
        }
    }

    pub fn is_dark(self) -> bool {
        match self {
            Self::Matugen => {
                // Matugen's mode follows the wallpaper, so judge darkness from the canvas color.
                let c = matugen_palette().bg_canvas;
                (c.r() as u32 + c.g() as u32 + c.b() as u32) < 384
            }
            _ => matches!(self, Self::Obsidian | Self::Nordic | Self::Midnight),
        }
    }

    pub fn palette(self) -> Palette {
        match self {
            Self::Matugen => matugen_palette(),
            Self::Obsidian => Palette {
                bg_canvas: Color32::from_rgb(20, 21, 24),
                bg_sidebar: Color32::from_rgb(25, 27, 31),
                bg_elevated: Color32::from_rgb(32, 35, 40),
                bg_selected: Color32::from_rgb(41, 46, 54),
                bg_hover: Color32::from_rgb(34, 38, 44),
                fg_primary: Color32::from_rgb(232, 230, 225),
                fg_secondary: Color32::from_rgb(168, 166, 160),
                fg_muted: Color32::from_rgb(112, 112, 108),
                accent: Color32::from_rgb(134, 168, 145),
                accent_soft: Color32::from_rgb(43, 56, 48),
                pin_color: Color32::from_rgb(214, 173, 108),
                danger: Color32::from_rgb(209, 109, 106),
                border: Color32::from_rgb(40, 43, 49),
                glow_strength: 0.6,
            },
            Self::Paper => Palette {
                bg_canvas: Color32::from_rgb(250, 248, 244),
                bg_sidebar: Color32::from_rgb(242, 239, 233),
                bg_elevated: Color32::from_rgb(234, 230, 222),
                bg_selected: Color32::from_rgb(225, 220, 210),
                bg_hover: Color32::from_rgb(235, 231, 223),
                fg_primary: Color32::from_rgb(38, 37, 35),
                fg_secondary: Color32::from_rgb(100, 98, 93),
                fg_muted: Color32::from_rgb(148, 144, 137),
                accent: Color32::from_rgb(76, 114, 91),
                accent_soft: Color32::from_rgb(218, 230, 222),
                pin_color: Color32::from_rgb(184, 128, 48),
                danger: Color32::from_rgb(186, 74, 72),
                border: Color32::from_rgb(224, 219, 210),
                glow_strength: 0.6,
            },
            Self::Nordic => Palette {
                bg_canvas: Color32::from_rgb(24, 28, 36),
                bg_sidebar: Color32::from_rgb(29, 34, 44),
                bg_elevated: Color32::from_rgb(37, 43, 55),
                bg_selected: Color32::from_rgb(47, 56, 72),
                bg_hover: Color32::from_rgb(41, 48, 62),
                fg_primary: Color32::from_rgb(229, 233, 240),
                fg_secondary: Color32::from_rgb(165, 176, 194),
                fg_muted: Color32::from_rgb(112, 124, 145),
                accent: Color32::from_rgb(136, 192, 208),
                accent_soft: Color32::from_rgb(43, 62, 74),
                pin_color: Color32::from_rgb(235, 203, 139),
                danger: Color32::from_rgb(191, 97, 106),
                border: Color32::from_rgb(46, 52, 64),
                glow_strength: 0.6,
            },
            Self::Sepia => Palette {
                bg_canvas: Color32::from_rgb(246, 238, 222),
                bg_sidebar: Color32::from_rgb(237, 227, 207),
                bg_elevated: Color32::from_rgb(252, 247, 236),
                bg_selected: Color32::from_rgb(230, 216, 190),
                bg_hover: Color32::from_rgb(241, 232, 214),
                fg_primary: Color32::from_rgb(58, 45, 30),
                fg_secondary: Color32::from_rgb(118, 96, 70),
                fg_muted: Color32::from_rgb(160, 142, 118),
                accent: Color32::from_rgb(150, 92, 40),
                accent_soft: Color32::from_rgb(236, 214, 180),
                pin_color: Color32::from_rgb(170, 110, 30),
                danger: Color32::from_rgb(168, 66, 56),
                border: Color32::from_rgb(222, 206, 180),
                glow_strength: 0.6,
            },
            Self::Midnight => Palette {
                bg_canvas: Color32::from_rgb(12, 16, 28),
                bg_sidebar: Color32::from_rgb(16, 22, 38),
                bg_elevated: Color32::from_rgb(24, 32, 52),
                bg_selected: Color32::from_rgb(33, 45, 72),
                bg_hover: Color32::from_rgb(28, 38, 61),
                fg_primary: Color32::from_rgb(220, 228, 245),
                fg_secondary: Color32::from_rgb(150, 164, 196),
                fg_muted: Color32::from_rgb(98, 112, 145),
                accent: Color32::from_rgb(110, 170, 255),
                accent_soft: Color32::from_rgb(30, 50, 84),
                pin_color: Color32::from_rgb(240, 190, 100),
                danger: Color32::from_rgb(238, 120, 130),
                border: Color32::from_rgb(34, 44, 68),
                glow_strength: 0.6,
            },
            Self::Matcha => Palette {
                bg_canvas: Color32::from_rgb(244, 247, 242),
                bg_sidebar: Color32::from_rgb(234, 240, 232),
                bg_elevated: Color32::from_rgb(224, 232, 221),
                bg_selected: Color32::from_rgb(211, 223, 207),
                bg_hover: Color32::from_rgb(228, 236, 225),
                fg_primary: Color32::from_rgb(34, 43, 36),
                fg_secondary: Color32::from_rgb(86, 102, 89),
                fg_muted: Color32::from_rgb(134, 148, 136),
                accent: Color32::from_rgb(65, 118, 84),
                accent_soft: Color32::from_rgb(206, 226, 211),
                pin_color: Color32::from_rgb(176, 124, 46),
                danger: Color32::from_rgb(184, 76, 74),
                border: Color32::from_rgb(212, 221, 209),
                glow_strength: 0.6,
            },
        }
    }

    /// Palette for this theme with the user's color overrides applied.
    pub fn palette_with(self, o: &ColorOverrides, glow_strength: f32) -> Palette {
        let mut p = self.palette();
        p.glow_strength = glow_strength;
        p.bg_canvas = rgb_or(o.bg, p.bg_canvas);
        p.bg_sidebar = rgb_or(o.sidebar, rgb_or(o.bg, p.bg_sidebar));
        p.fg_primary = rgb_or(o.fg, p.fg_primary);
        p.accent = rgb_or(o.accent, p.accent);
        p.pin_color = rgb_or(o.pin, p.pin_color);
        p.fg_muted = rgb_or(o.fg_muted, p.fg_muted);
        p.border = rgb_or(o.border, p.border);
        p.danger = rgb_or(o.danger, p.danger);
        p
    }

    /// Applies the theme to egui. `body_size` scales all text styles relative to the 15pt default.
    pub fn apply(self, ctx: &egui::Context, o: &ColorOverrides, body_size: f32, glow: f32) {
        if self == Self::Matugen {
            reload_matugen_palette();
        }
        let p = self.palette_with(o, glow);
        let s = body_size / 15.0;
        let mut style: Style = (*ctx.style()).clone();

        style.text_styles = [
            (
                TextStyle::Heading,
                FontId::new(22.0 * s, FontFamily::Proportional),
            ),
            (
                TextStyle::Name("NoteTitle".into()),
                FontId::new(24.0 * s, FontFamily::Proportional),
            ),
            (
                TextStyle::Name("Subheading".into()),
                FontId::new(16.5 * s, FontFamily::Proportional),
            ),
            (
                TextStyle::Body,
                FontId::new(body_size, FontFamily::Proportional),
            ),
            (
                TextStyle::Monospace,
                FontId::new(13.5 * s, FontFamily::Monospace),
            ),
            (
                TextStyle::Button,
                FontId::new(13.5 * s, FontFamily::Proportional),
            ),
            (
                TextStyle::Small,
                FontId::new(12.0 * s, FontFamily::Proportional),
            ),
        ]
        .into();

        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.window_margin = Margin::same(12);

        let mut visuals = if self.is_dark() {
            Visuals::dark()
        } else {
            Visuals::light()
        };

        visuals.panel_fill = p.bg_canvas;
        visuals.window_fill = p.bg_elevated;
        visuals.extreme_bg_color = p.bg_canvas;
        visuals.faint_bg_color = p.bg_sidebar;
        visuals.code_bg_color = p.bg_elevated;
        visuals.override_text_color = Some(p.fg_primary);
        visuals.selection.bg_fill = p.accent_soft;
        visuals.selection.stroke = Stroke::new(1.0_f32, p.accent);

        let rounding = CornerRadius::same(6);
        visuals.widgets.noninteractive.bg_fill = p.bg_canvas;
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.fg_primary);
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.border);
        visuals.widgets.noninteractive.corner_radius = rounding;

        visuals.widgets.inactive.bg_fill = p.bg_elevated;
        visuals.widgets.inactive.weak_bg_fill = p.bg_elevated;
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, p.fg_primary);
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, p.border);
        visuals.widgets.inactive.corner_radius = rounding;

        visuals.widgets.hovered.bg_fill = p.bg_hover;
        visuals.widgets.hovered.weak_bg_fill = p.bg_hover;
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, p.fg_primary);
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, p.accent);
        visuals.widgets.hovered.corner_radius = rounding;

        visuals.widgets.active.bg_fill = p.bg_selected;
        visuals.widgets.active.weak_bg_fill = p.bg_selected;
        visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, p.fg_primary);
        visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, p.accent);
        visuals.widgets.active.corner_radius = rounding;

        style.visuals = visuals;
        ctx.set_style(style);
    }
}
