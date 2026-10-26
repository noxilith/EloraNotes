use crate::icon_names::TABLER_ICON_NAMES;
use egui::{FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

/// Looks up the Tabler glyph for an icon name such as `flare`.
pub fn tabler_glyph(name: &str) -> Option<&'static str> {
    TABLER_ICON_NAMES
        .binary_search_by_key(&name, |(n, _)| n)
        .ok()
        .map(|i| TABLER_ICON_NAMES[i].1)
}

/// Embedded Tabler Icons outline font (MIT License).
pub const TABLER_FONT_BYTES: &[u8] = include_bytes!("../assets/tabler-icons.ttf");

/// Bold weight of DejaVu Sans (free license, see assets/DejaVuSans-LICENSE.txt).
pub const BOLD_FONT_BYTES: &[u8] = include_bytes!("../assets/DejaVuSans-Bold.ttf");

/// Registers `tabler-icons.ttf` as a fallback on `Proportional` and `Monospace` font families
/// so Tabler Icon glyphs render seamlessly alongside standard text in labels, buttons, and inputs.
pub fn install_tabler_icons(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "tabler-icons".to_owned(),
        Arc::new(FontData::from_static(TABLER_FONT_BYTES)),
    );

    if let Some(family) = fonts.families.get_mut(&FontFamily::Proportional) {
        family.push("tabler-icons".to_owned());
    }
    if let Some(family) = fonts.families.get_mut(&FontFamily::Monospace) {
        family.push("tabler-icons".to_owned());
    }

    // Real bold weight (DejaVu Sans Bold, Bitstream/free license) exposed as its own family, so
    // `**bold**` text is drawn with genuinely heavier glyphs rather than an overdraw trick.
    fonts.font_data.insert(
        "dejavu-bold".to_owned(),
        Arc::new(FontData::from_static(BOLD_FONT_BYTES)),
    );
    let mut bold_family = vec!["dejavu-bold".to_owned()];
    if let Some(proportional) = fonts.families.get(&FontFamily::Proportional) {
        bold_family.extend(proportional.iter().cloned());
    }
    fonts
        .families
        .insert(FontFamily::Name(BOLD_FAMILY.into()), bold_family);

    ctx.set_fonts(fonts);
}

/// Name of the bold font family used by bold Markdown text.
pub const BOLD_FAMILY: &str = "bold";

/// Returns the bold font family for use in `FontId`s.
pub fn bold_family() -> FontFamily {
    FontFamily::Name(BOLD_FAMILY.into())
}

/// Tabler Icons (Outline set) codepoints.
#[allow(dead_code)]
pub mod icon {
    pub const PLUS: &str = "\u{eb0b}";
    pub const SEARCH: &str = "\u{eb1c}";
    pub const X: &str = "\u{eb55}";
    pub const PIN: &str = "\u{ec9c}";
    pub const PINNED: &str = "\u{ed60}";
    pub const PINNED_OFF: &str = "\u{ed5f}";
    pub const STAR: &str = "\u{eb2e}";
    pub const SUN: &str = "\u{eb30}";
    pub const MOON: &str = "\u{eaf8}";
    pub const PALETTE: &str = "\u{eb01}";
    pub const SPARKLES: &str = "\u{f6d7}";
    pub const TRASH: &str = "\u{eb41}";
    pub const TRASH_X: &str = "\u{ef88}";
    pub const RESTORE: &str = "\u{fafd}";
    pub const ARCHIVE: &str = "\u{ea0b}";
    pub const EYE: &str = "\u{ea9a}";
    pub const EDIT: &str = "\u{ea98}";
    pub const PENCIL: &str = "\u{eb04}";
    pub const BRUSH: &str = "\u{ebb8}";
    pub const COLUMNS: &str = "\u{eb83}";
    pub const LAYOUT_COLUMNS: &str = "\u{ead4}";
    pub const LAYOUT_SIDEBAR: &str = "\u{eada}";
    pub const SIDEBAR_COLLAPSE: &str = "\u{f004}";
    pub const SIDEBAR_EXPAND: &str = "\u{f005}";
    pub const MAXIMIZE: &str = "\u{eaea}";
    pub const MINIMIZE: &str = "\u{eaf1}";
    pub const TAG: &str = "\u{10096}";
    pub const TAGS: &str = "\u{ef86}";
    pub const HASH: &str = "\u{eabc}";
    pub const FOLDER: &str = "\u{eaad}";
    pub const NOTES: &str = "\u{eb6e}";
    pub const NOTE: &str = "\u{eb6d}";
    pub const FILE_TEXT: &str = "\u{eaa2}";
    pub const FILE_PLUS: &str = "\u{eaa0}";
    pub const CALENDAR: &str = "\u{ea53}";
    pub const CALENDAR_EVENT: &str = "\u{ea52}";
    pub const CLOCK: &str = "\u{ea70}";
    pub const CHECK: &str = "\u{ea5e}";
    pub const CHECKBOX: &str = "\u{eba6}";
    pub const SQUARE: &str = "\u{eb2c}";
    pub const SQUARE_CHECK: &str = "\u{eb28}";
    pub const BOLD: &str = "\u{eb7b}";
    pub const ITALIC: &str = "\u{eb93}";
    pub const STRIKETHROUGH: &str = "\u{eb9e}";
    pub const UNDERLINE: &str = "\u{eba2}";
    pub const HIGHLIGHT: &str = "\u{ef3f}";
    pub const COLOR_PICKER: &str = "\u{ebe6}";
    pub const COLOR_SWATCH: &str = "\u{eb61}";
    pub const CLEAR_FORMATTING: &str = "\u{ebe5}";
    pub const SEPARATOR: &str = "\u{ebda}";
    pub const FLARE: &str = "\u{ee8e}";
    pub const PHOTO: &str = "\u{eb0a}";
    pub const PHOTO_OFF: &str = "\u{ecf6}";
    pub const DROPLET: &str = "\u{ea97}";
    pub const CODE: &str = "\u{ea77}";
    pub const QUOTE: &str = "\u{efbe}";
    pub const HEADING: &str = "\u{ee9b}";
    pub const H1: &str = "\u{ec94}";
    pub const H2: &str = "\u{ec95}";
    pub const H3: &str = "\u{ec96}";
    pub const LIST: &str = "\u{eb6b}";
    pub const LIST_CHECK: &str = "\u{eb6a}";
    pub const LINK: &str = "\u{eade}";
    pub const ARROW_BACK_UP: &str = "\u{eb77}";
    pub const ARROWS_SORT: &str = "\u{eb5a}";
    pub const SORT_ASC: &str = "\u{eb26}";
    pub const SORT_DESC: &str = "\u{eb27}";
    pub const COMMAND: &str = "\u{ea78}";
    pub const COPY: &str = "\u{ea7a}";
    pub const DOWNLOAD: &str = "\u{ea96}";
    pub const FILE_EXPORT: &str = "\u{ede9}";
    pub const DOTS: &str = "\u{ea95}";
    pub const CHEVRON_RIGHT: &str = "\u{ea61}";
    pub const CHEVRON_DOWN: &str = "\u{ea5f}";
    pub const CHEVRON_UP: &str = "\u{ea62}";
    pub const BOOK: &str = "\u{ea39}";
    pub const BOOKMARK: &str = "\u{ea3a}";
    pub const BULB: &str = "\u{ea51}";
    pub const FEATHER: &str = "\u{ee8b}";
    pub const COMPASS: &str = "\u{ea79}";
    pub const ADJUSTMENTS: &str = "\u{ea03}";
    pub const SETTINGS: &str = "\u{eb20}";
    pub const INFO_CIRCLE: &str = "\u{eac5}";
    pub const REFRESH: &str = "\u{eb13}";
    pub const ALERT_TRIANGLE: &str = "\u{ea06}";
    pub const CLOUD: &str = "\u{ea76}";
    pub const SERVER: &str = "\u{eb1f}";
    pub const USERS: &str = "\u{ebf2}";
    pub const USER_PLUS: &str = "\u{eb4b}";
    pub const LOCK: &str = "\u{eae2}";
    pub const LOCK_OPEN: &str = "\u{eae1}";
    pub const WORLD: &str = "\u{eb54}";
    pub const WIFI_OFF: &str = "\u{ecfa}";
    pub const GRAPH: &str = "\u{f288}";
    pub const ROBOT: &str = "\u{f00b}";
    pub const HISTORY: &str = "\u{ebea}";
}
