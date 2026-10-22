//! The window icon is generated into `assets/icons/` by `scripts/make-icons.py` from the
//! project logo. Generating it is a manual step (the artwork is not always present), so the
//! build only wires it in when the file exists.

fn main() {
    println!("cargo:rerun-if-changed=assets/icons/window-icon-256.rgba");
    println!("cargo:rustc-check-cfg=cfg(has_window_icon)");
    if std::path::Path::new("assets/icons/window-icon-256.rgba").exists() {
        println!("cargo:rustc-cfg=has_window_icon");
    }
}
