//! The window icon and the Windows executable icon both come from `assets/icons/`, which
//! `scripts/make-icons.py` generates from the project logo. Generating them is a manual step
//! (the artwork is not always present), so the build only wires them in when the files exist.

fn main() {
    println!("cargo:rerun-if-changed=assets/icons/window-icon-256.rgba");
    println!("cargo:rerun-if-changed=assets/icons/elora-notes.ico");
    println!("cargo:rustc-check-cfg=cfg(has_window_icon)");

    if std::path::Path::new("assets/icons/window-icon-256.rgba").exists() {
        println!("cargo:rustc-cfg=has_window_icon");
    }

    // Explorer, the Start menu shortcut, a pinned taskbar icon and the setup wizard all take
    // the executable's own icon resource, which has nothing to do with the icon the running
    // window sets. Only MSVC/Windows hosts can compile the resource.
    #[cfg(windows)]
    embed_executable_icon();
}

#[cfg(windows)]
fn embed_executable_icon() {
    if !std::path::Path::new("assets/icons/elora-notes.ico").exists() {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("assets/icons/elora-notes.ico")
        .set("ProductName", "EloraNotes")
        .set("FileDescription", "EloraNotes — Markdown notes")
        .set("LegalCopyright", "GPL-3.0-or-later");
    if let Err(error) = resource.compile() {
        // A missing resource compiler should not stop the build; the window icon still works.
        println!("cargo:warning=could not embed the Windows icon: {error}");
    }
}
