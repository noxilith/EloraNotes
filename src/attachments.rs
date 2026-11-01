//! How a picture in a note is named and filed: the same on the desktop and on the phone, so a
//! picture added on one is the file the other asks the library for.

use sha2::{Digest, Sha256};

/// A file name for a stored picture: lower case letters and digits, with everything else folded
/// into single dashes, so the Markdown address never ends at a space.
pub fn picture_name(hint: &str) -> String {
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
pub fn picture_extension(extension: &str) -> String {
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
pub fn stored_picture_name(bytes: &[u8], name_hint: &str, extension: &str) -> String {
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
