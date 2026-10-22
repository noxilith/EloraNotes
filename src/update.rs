//! In-app updates: asks GitHub for the newest release when the app starts, and installs that
//! release in whatever shape this copy of the app was installed in.
//!
//! There are five shapes, and each one is replaced differently:
//!
//! * **AppImage** — the whole image file is swapped for the new image and started again.
//! * **Flatpak** — the sandbox owns the files, so the app only points at `flatpak update`.
//! * **Windows setup** (the Inno Setup wizard) — the new setup runs silently over the old
//!   install, which keeps shortcuts and the uninstall entry up to date.
//! * **macOS `.app`** — the downloaded bundle replaces the old one, which is renamed aside.
//! * **Portable** — a loose `elora` / `elora.exe` binary, replaced in place (Windows renames
//!   the running image aside first).
//!
//! The check and the download both run on worker threads; the UI only ever sees the result
//! through a channel (see `EloraApp::poll_update`).

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// GitHub repository whose releases ship prebuilt binaries.
const LATEST_RELEASE_API: &str = "https://api.github.com/repos/noxilith/EloraNotes/releases/latest";
/// Refuse absurd downloads such as an HTML error page or a swapped asset. AppImages and macOS
/// bundles are an order of magnitude smaller than this.
const MAX_DOWNLOAD_BYTES: u64 = 256 * 1024 * 1024;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Flatpak application ID; the bundle in the release and the store entry both use it.
pub const FLATPAK_APP_ID: &str = "io.github.noxilith.EloraNotes";

/// One downloadable file attached to a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

/// A newer release together with the asset that fits this installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Version without the tag's leading `v`, e.g. `0.7.0`.
    pub version: String,
    pub asset: Asset,
}

/// What the launch-time check turned up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateOffer {
    /// A newer release with a download this copy can install by itself.
    SelfUpdate(Release),
    /// A newer release, but this copy is owned by a package manager that does the updating.
    Managed {
        version: String,
        manager: &'static str,
    },
}

/// How this copy was installed. Decides which release asset to fetch and how to replace it.
///
/// Some variants can only be produced on one platform, so the other platforms never construct
/// them even though the match arms still have to exist.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallKind {
    /// Running from an AppImage; `$APPIMAGE` points at the image file.
    AppImage(PathBuf),
    /// Inside the Flatpak sandbox, where the runtime owns every file.
    Flatpak,
    /// Running from `EloraNotes.app/Contents/MacOS/elora`.
    MacBundle(PathBuf),
    /// Installed by the Windows setup wizard (it leaves `unins000.exe` beside the app).
    WindowsSetup,
    /// A loose binary or portable `.exe`: only the executable itself is replaced.
    Portable,
}

fn non_empty_path(value: std::ffi::OsString) -> Option<PathBuf> {
    let path = PathBuf::from(value);
    if path.as_os_str().is_empty() {
        None
    } else {
        Some(path)
    }
}

/// The `.app` directory containing the running binary, if there is one.
#[cfg(target_os = "macos")]
fn enclosing_app_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors()
        .find(|path| {
            path.extension()
                .map(|ext| ext.eq_ignore_ascii_case("app"))
                .unwrap_or(false)
        })
        .map(Path::to_path_buf)
}

/// True when the running executable really lives inside the mount point of an AppImage.
///
/// `$APPIMAGE` is inherited by every child process, so an app started from a terminal that
/// itself runs inside some other AppImage would otherwise believe it owns that image and
/// overwrite it. Comparing the executable against `$APPDIR` (both are set by the AppImage
/// runtime for its own process) rules that out.
#[cfg(any(target_os = "linux", test))]
fn exe_lives_in(appdir: Option<PathBuf>, exe: Option<PathBuf>) -> bool {
    match (appdir, exe) {
        (Some(appdir), Some(exe)) => exe.starts_with(appdir),
        _ => false,
    }
}

#[cfg(target_os = "linux")]
fn running_from_appdir() -> bool {
    exe_lives_in(
        std::env::var_os("APPDIR").and_then(non_empty_path),
        std::env::current_exe().ok(),
    )
}

/// Detects how this copy was installed. Keeping the detection in one place means the asset
/// picker and the installer cannot disagree about what they are dealing with.
pub fn install_kind() -> InstallKind {
    #[cfg(target_os = "linux")]
    {
        if let Some(image) = std::env::var_os("APPIMAGE").and_then(non_empty_path) {
            if running_from_appdir() {
                return InstallKind::AppImage(image);
            }
        }
        if Path::new("/.flatpak-info").exists() || std::env::var_os("FLATPAK_ID").is_some() {
            return InstallKind::Flatpak;
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(bundle) = enclosing_app_bundle() {
            return InstallKind::MacBundle(bundle);
        }
    }
    #[cfg(windows)]
    {
        if let Ok(exe) = std::env::current_exe() {
            if exe.parent().map(installed_by_setup).unwrap_or(false) {
                return InstallKind::WindowsSetup;
            }
        }
    }
    InstallKind::Portable
}

/// Inno Setup drops an uninstaller next to the app it installed (`unins000.exe`, or
/// `unins001.exe` if that name was taken), which is how an installed copy tells itself apart
/// from the portable `.exe`.
#[cfg(any(windows, test))]
fn looks_like_inno_uninstaller(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with("unins") && name.ends_with(".exe")
}

#[cfg(windows)]
fn installed_by_setup(dir: &Path) -> bool {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .any(|entry| looks_like_inno_uninstaller(&entry.file_name().to_string_lossy()))
        })
        .unwrap_or(false)
}

/// `0.6.1`, `v0.6.1` and `0.6.1-beta.1` all parse as `(0, 6, 1)`.
fn version_tuple(version: &str) -> Option<(u64, u64, u64)> {
    let trimmed = version.trim().trim_start_matches('v');
    let core = trimmed.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

/// True when `candidate` is a strictly newer release than `current`. Unparseable versions
/// never count as newer, so a stray tag such as `nightly` cannot trigger an update prompt.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (version_tuple(candidate), version_tuple(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

/// The platform tag shared by every asset the release workflow builds for this machine.
/// `None` on platforms the workflow does not build for (for example an Intel Mac), in which
/// case the app stays quiet instead of offering a binary it cannot run.
pub fn platform_key() -> Option<&'static str> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("linux-x86_64")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("windows-x86_64")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("macos-arm64")
    } else {
        None
    }
}

/// File endings that fit `kind`, most preferred first. The older `0.6.x` names
/// (`elora-notes-windows-x86_64.exe`) are included so a portable copy of an old release can
/// still find its successor.
fn asset_suffixes(kind: &InstallKind, platform: &str) -> Option<&'static [&'static str]> {
    match kind {
        InstallKind::AppImage(_) => Some(&[".AppImage", ".appimage"]),
        InstallKind::Flatpak => None,
        InstallKind::MacBundle(_) => Some(&[".zip"]),
        InstallKind::WindowsSetup => Some(&["-setup.exe"]),
        InstallKind::Portable => {
            if platform.starts_with("windows") {
                Some(&["-portable.exe", "windows-x86_64.exe"])
            } else if platform.starts_with("macos") {
                Some(&[".zip"])
            } else {
                Some(&[".tar.gz"])
            }
        }
    }
}

fn select_asset<'a>(assets: &'a [Asset], platform: &str, kind: &InstallKind) -> Option<&'a Asset> {
    let suffixes = asset_suffixes(kind, platform)?;
    for suffix in suffixes {
        if let Some(asset) = assets
            .iter()
            .find(|asset| asset.name.contains(platform) && asset.name.ends_with(suffix))
        {
            return Some(asset);
        }
    }
    None
}

fn parse_release_info(json: &str) -> Result<Option<(String, Vec<Asset>)>, String> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| format!("Could not read the release information: {e}"))?;
    let version = value["tag_name"]
        .as_str()
        .unwrap_or_default()
        .trim_start_matches('v')
        .to_string();
    if version_tuple(&version).is_none() {
        return Ok(None);
    }
    let assets = value["assets"]
        .as_array()
        .map(|assets| {
            assets
                .iter()
                .filter_map(|asset| {
                    Some(Asset {
                        name: asset["name"].as_str()?.to_string(),
                        url: asset["browser_download_url"].as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Some((version, assets)))
}

/// Picks the download this installation can use out of a `/releases/latest` response. The
/// update check itself only calls this through `parse_release_info`; tests use it to cover
/// every install kind, including the ones this platform cannot run.
#[cfg(test)]
fn release_for(json: &str, kind: &InstallKind) -> Result<Option<Release>, String> {
    let Some((version, assets)) = parse_release_info(json)? else {
        return Ok(None);
    };
    let Some(platform) = platform_key() else {
        return Ok(None);
    };
    Ok(select_asset(&assets, platform, kind).map(|asset| Release {
        version,
        asset: asset.clone(),
    }))
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(20))
        .user_agent(concat!("EloraNotes/", env!("CARGO_PKG_VERSION")))
        .build()
}

fn download(url: &str) -> Result<Vec<u8>, String> {
    let response = agent()
        .get(url)
        .call()
        .map_err(|e| format!("Download failed: {e}"))?;
    let mut data = Vec::new();
    response
        .into_reader()
        .take(MAX_DOWNLOAD_BYTES)
        .read_to_end(&mut data)
        .map_err(|e| format!("Download failed: {e}"))?;
    if data.is_empty() {
        return Err("The downloaded update was empty.".to_string());
    }
    Ok(data)
}

/// Asks GitHub for the latest release. Returns `Ok(None)` when the app is already current or
/// when no asset fits this installation; a failed request is an `Err`, which the caller
/// treats as "nothing to do" so starting offline stays quiet.
pub fn check_for_update() -> Result<Option<UpdateOffer>, String> {
    let body = agent()
        .get(LATEST_RELEASE_API)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("Could not check for updates: {e}"))?
        .into_string()
        .map_err(|e| format!("Could not check for updates: {e}"))?;
    let Some((version, assets)) = parse_release_info(&body)? else {
        return Ok(None);
    };
    if !is_newer(&version, CURRENT_VERSION) {
        return Ok(None);
    }
    let kind = install_kind();
    if kind == InstallKind::Flatpak {
        return Ok(Some(UpdateOffer::Managed {
            version,
            manager: "Flatpak",
        }));
    }
    let Some(platform) = platform_key() else {
        return Ok(None);
    };
    Ok(select_asset(&assets, platform, &kind).map(|asset| {
        UpdateOffer::SelfUpdate(Release {
            version,
            asset: asset.clone(),
        })
    }))
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("elora")
        .to_string()
}

/// Where the download is unpacked before it is trusted enough to touch the installation.
/// It always sits on the same filesystem as the thing it will replace, so the final step is a
/// rename rather than a copy.
fn staging_path(kind: &InstallKind) -> Result<PathBuf, String> {
    match kind {
        InstallKind::AppImage(image) => {
            Ok(image.with_file_name(format!(".{}.update", file_name_of(image))))
        }
        InstallKind::MacBundle(bundle) => {
            let parent = bundle
                .parent()
                .ok_or("Could not find the folder holding EloraNotes.app")?;
            Ok(parent.join(".elora-notes-update"))
        }
        InstallKind::WindowsSetup => Ok(std::env::temp_dir().join("elora-notes-setup.exe")),
        InstallKind::Flatpak | InstallKind::Portable => {
            let exe = std::env::current_exe()
                .map_err(|e| format!("Could not locate the running EloraNotes binary: {e}"))?;
            Ok(unpacked_binary_path(&exe))
        }
    }
}

/// The staged executable for a loose binary. Windows only runs files named `.exe`, and the
/// staged build has to be started once to check that it works at all.
fn unpacked_binary_path(exe: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        exe.with_file_name("elora-update.exe")
    }
    #[cfg(not(windows))]
    {
        exe.with_file_name(format!("{}.new", file_name_of(exe)))
    }
}

/// On Windows the previous binary is renamed aside before the new one is put in place; it can
/// only be deleted once the old process has exited.
#[cfg(windows)]
fn backup_path(exe: &Path) -> PathBuf {
    exe.with_file_name(format!("{}.old", file_name_of(exe)))
}

/// The bundle the old `.app` is renamed to while the new one is moved into place.
#[cfg(target_os = "macos")]
fn bundle_backup_path(bundle: &Path) -> PathBuf {
    bundle.with_file_name(format!(".{}.old", file_name_of(bundle)))
}

/// Removes leftovers from an update that was downloaded but never installed, or whose old
/// copy could not be deleted while it was still in use.
pub fn cleanup_stale_updates() {
    let kind = install_kind();
    if let Ok(staged) = staging_path(&kind) {
        if staged.is_dir() {
            let _ = fs::remove_dir_all(&staged);
        } else {
            let _ = fs::remove_file(&staged);
        }
    }
    #[cfg(windows)]
    if let Ok(exe) = std::env::current_exe() {
        let _ = fs::remove_file(backup_path(&exe));
    }
    #[cfg(target_os = "macos")]
    if let InstallKind::MacBundle(bundle) = &kind {
        let _ = fs::remove_dir_all(bundle_backup_path(bundle));
    }
}

/// Starts the staged build once before it replaces anything. A binary built against a newer
/// glibc, or for a different CPU, fails here instead of leaving the user with a broken app.
fn can_run(binary: &Path) -> Result<(), String> {
    let status = std::process::Command::new(binary)
        .arg("--help")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("The downloaded build cannot run on this system: {e}"))?;
    if !status.success() {
        return Err("The downloaded build cannot run on this system.".to_string());
    }
    Ok(())
}

/// Downloads the release asset and unfolds it next to the installed copy. The installation is
/// left untouched until `install_and_restart`, so a failed download changes nothing.
pub fn stage_download(release: &Release) -> Result<PathBuf, String> {
    let kind = install_kind();
    let staged = staging_path(&kind)?;
    remove_staged(&staged);

    let data = download(&release.asset.url)?;
    match &kind {
        InstallKind::AppImage(_) => {
            fs::write(&staged, &data)
                .map_err(|e| format!("Could not save the update next to the AppImage: {e}"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))
                    .map_err(|e| format!("Could not make the update executable: {e}"))?;
            }
            check_staged(&staged)?;
        }
        InstallKind::MacBundle(_) => {
            if let Err(error) = extract_app(&data, &staged) {
                remove_staged(&staged);
                return Err(error);
            }
            check_staged(&staged)?;
        }
        InstallKind::WindowsSetup => {
            if !data.starts_with(b"MZ") || data.len() < 256 * 1024 {
                return Err("The downloaded setup program is not a Windows installer.".to_string());
            }
            fs::write(&staged, &data).map_err(|e| format!("Could not save the update: {e}"))?;
        }
        InstallKind::Flatpak | InstallKind::Portable => {
            #[cfg(unix)]
            if let Err(error) = extract_unix_binary(&data, &staged) {
                remove_staged(&staged);
                return Err(error);
            }
            #[cfg(windows)]
            fs::write(&staged, &data).map_err(|e| format!("Could not save the update: {e}"))?;
            check_staged(&staged)?;
        }
    }

    Ok(staged)
}

fn remove_staged(staged: &Path) {
    if staged.is_dir() {
        let _ = fs::remove_dir_all(staged);
    } else {
        let _ = fs::remove_file(staged);
    }
}

/// The staged artifact is a bundle directory on macOS and a binary everywhere else.
fn check_staged(staged: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    if staged.is_dir() {
        let app = prepared_app(staged)?;
        return can_run(&bundle_binary(&app));
    }
    can_run(staged)
}

/// `EloraNotes.app/Contents/MacOS/elora` inside a bundle.
#[cfg(target_os = "macos")]
fn bundle_binary(bundle: &Path) -> PathBuf {
    bundle.join("Contents").join("MacOS").join("elora")
}

/// The `.app` inside a staged download, with its executable restored to `755` in case the
/// zip lost the mode bits. Fails before anything is swapped if the binary is missing.
#[cfg(target_os = "macos")]
fn prepared_app(staged: &Path) -> Result<PathBuf, String> {
    use std::os::unix::fs::PermissionsExt;

    let app = find_app(staged)
        .ok_or_else(|| "The downloaded update does not contain EloraNotes.app.".to_string())?;
    let binary = bundle_binary(&app);
    if !binary.is_file() {
        return Err("The downloaded app does not contain the EloraNotes binary.".to_string());
    }
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("Could not make the update executable: {e}"))?;
    Ok(app)
}

#[cfg(unix)]
fn extract_unix_binary(archive: &[u8], staged: &Path) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    let decoder = flate2::read::GzDecoder::new(archive);
    let mut tar = tar::Archive::new(decoder);
    let entries = tar
        .entries()
        .map_err(|e| format!("The update archive is unreadable: {e}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| format!("The update archive is unreadable: {e}"))?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let is_binary = entry
            .path()
            .ok()
            .and_then(|path| path.file_name().map(|name| name == "elora"))
            .unwrap_or(false);
        if !is_binary {
            continue;
        }
        let mut out =
            fs::File::create(staged).map_err(|e| format!("Could not write the update: {e}"))?;
        std::io::copy(&mut entry, &mut out)
            .map_err(|e| format!("Could not write the update: {e}"))?;
        out.flush().ok();
        fs::set_permissions(staged, fs::Permissions::from_mode(0o755))
            .map_err(|e| format!("Could not make the update executable: {e}"))?;
        return Ok(());
    }
    Err("The update archive did not contain the EloraNotes binary.".to_string())
}

/// Unpacks the portable `.app` zip into `target`, which becomes `<target>/EloraNotes.app`.
/// Entry paths are checked so a crafted archive cannot write outside the target directory.
fn extract_app(archive: &[u8], target: &Path) -> Result<(), String> {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fs::create_dir_all(target).map_err(|e| format!("Could not create the update folder: {e}"))?;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(archive))
        .map_err(|e| format!("The update archive is unreadable: {e}"))?;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|e| format!("The update archive is unreadable: {e}"))?;
        let Some(relative) = entry.enclosed_name() else {
            return Err("The update archive contains an unsafe path.".to_string());
        };
        if relative.as_os_str().is_empty() {
            continue;
        }
        let out_path = target.join(&relative);
        if entry.is_dir() {
            fs::create_dir_all(&out_path)
                .map_err(|e| format!("Could not unpack the update: {e}"))?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("Could not unpack the update: {e}"))?;
        }
        let mut out =
            fs::File::create(&out_path).map_err(|e| format!("Could not unpack the update: {e}"))?;
        std::io::copy(&mut entry, &mut out)
            .map_err(|e| format!("Could not unpack the update: {e}"))?;
        out.flush().ok();
        // Zip keeps the executable bit in the high mode bits; the app binary needs it.
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            let _ = fs::set_permissions(&out_path, fs::Permissions::from_mode(mode));
        }
    }
    Ok(())
}

/// The first `.app` directory unpacked into `dir`.
#[cfg(any(target_os = "macos", test))]
fn find_app(dir: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let is_app = path.is_dir()
            && path
                .extension()
                .map(|ext| ext.eq_ignore_ascii_case("app"))
                .unwrap_or(false);
        if is_app {
            return Some(path);
        }
    }
    None
}

fn is_silent_installer(path: &Path) -> bool {
    path.extension()
        .map(|ext| ext.eq_ignore_ascii_case("exe"))
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn install_mac_bundle(staged: &Path, bundle: &Path) -> Result<(), String> {
    let new_app = prepared_app(staged)?;
    let backup = bundle_backup_path(bundle);
    let _ = fs::remove_dir_all(&backup);
    fs::rename(bundle, &backup)
        .map_err(|e| format!("Could not move the old EloraNotes.app aside: {e}"))?;
    if let Err(error) = fs::rename(&new_app, bundle) {
        let _ = fs::rename(&backup, bundle);
        return Err(format!("Could not install the update: {error}"));
    }
    let _ = fs::remove_dir_all(staged);
    std::process::Command::new("open")
        .arg("-n")
        .arg(bundle)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Could not restart EloraNotes: {e}"))?;
    Ok(())
}

/// Puts the staged update in place and starts it with the same arguments. On success the
/// caller must exit immediately so the old process lets go of the installation.
pub fn install_and_restart(staged: &Path) -> Result<(), String> {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let kind = install_kind();

    match &kind {
        InstallKind::AppImage(image) => {
            fs::rename(staged, image)
                .map_err(|e| format!("Could not replace the AppImage: {e}"))?;
            start(image, &args)
        }
        InstallKind::MacBundle(_) => {
            #[cfg(target_os = "macos")]
            {
                let InstallKind::MacBundle(bundle) = &kind else {
                    unreachable!()
                };
                install_mac_bundle(staged, bundle)
            }
            #[cfg(not(target_os = "macos"))]
            {
                Err("EloraNotes.app bundles can only be updated on macOS.".to_string())
            }
        }
        InstallKind::WindowsSetup => {
            if !is_silent_installer(staged) {
                return Err("The downloaded setup program is not a Windows installer.".to_string());
            }
            // The wizard closes the running app, replaces the files and puts the shortcuts and
            // uninstall entry back in place; the caller exits right after this returns.
            std::process::Command::new(staged)
                .args([
                    "/SILENT",
                    "/SUPPRESSMSGBOXES",
                    "/CLOSEAPPLICATIONS",
                    "/NORESTART",
                ])
                .stdin(std::process::Stdio::null())
                .spawn()
                .map_err(|e| format!("Could not start the EloraNotes setup program: {e}"))?;
            Ok(())
        }
        InstallKind::Flatpak | InstallKind::Portable => {
            let exe = std::env::current_exe()
                .map_err(|e| format!("Could not locate the running EloraNotes binary: {e}"))?;
            #[cfg(windows)]
            {
                let backup = backup_path(&exe);
                let _ = fs::remove_file(&backup);
                fs::rename(&exe, &backup)
                    .map_err(|e| format!("Could not replace the running EloraNotes binary: {e}"))?;
                if let Err(error) = fs::rename(staged, &exe) {
                    let _ = fs::rename(&backup, &exe);
                    return Err(format!("Could not install the update: {error}"));
                }
            }
            #[cfg(unix)]
            fs::rename(staged, &exe).map_err(|e| format!("Could not install the update: {e}"))?;
            start(&exe, &args)
        }
    }
}

fn start(program: &Path, args: &[std::ffi::OsString]) -> Result<(), String> {
    std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Could not restart EloraNotes: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_tuple_accepts_tag_prefixes_and_suffixes() {
        assert_eq!(version_tuple("0.7.0"), Some((0, 7, 0)));
        assert_eq!(version_tuple("v0.7.0"), Some((0, 7, 0)));
        assert_eq!(version_tuple("1.2"), Some((1, 2, 0)));
        assert_eq!(version_tuple("v0.7.0-beta.1"), Some((0, 7, 0)));
        assert_eq!(version_tuple("nightly"), None);
        assert_eq!(version_tuple(""), None);
    }

    #[test]
    fn test_is_newer_compares_numerically() {
        assert!(is_newer("0.7.1", "0.7.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("v1.0.0", "0.99.99"));
        assert!(!is_newer("0.7.0", "0.7.0"));
        assert!(!is_newer("0.6.0", "0.7.0"));
        assert!(!is_newer("nightly", "0.7.0"));
    }

    fn release_json(version: &str, assets: &[(&str, &str)]) -> String {
        let assets: Vec<String> = assets
            .iter()
            .map(|(name, url)| format!(r#"{{"name":"{name}","browser_download_url":"{url}"}}"#))
            .collect();
        format!(
            r#"{{"tag_name":"v{version}","assets":[{}]}}"#,
            assets.join(",")
        )
    }

    /// The asset names the release workflow publishes.
    fn published_assets() -> Vec<(&'static str, &'static str)> {
        vec![
            (
                "EloraNotes-0.7.0-linux-x86_64.AppImage",
                "https://x/appimage",
            ),
            ("EloraNotes-0.7.0-linux-x86_64.tar.gz", "https://x/linux"),
            (
                "EloraNotes-0.7.0-windows-x86_64-setup.exe",
                "https://x/setup",
            ),
            (
                "EloraNotes-0.7.0-windows-x86_64-portable.exe",
                "https://x/portable",
            ),
            ("EloraNotes-0.7.0-macos-arm64.zip", "https://x/macos"),
            ("EloraNotes-0.7.0-macos-arm64.dmg", "https://x/dmg"),
            ("EloraNotes-0.7.0-linux-x86_64.flatpak", "https://x/flatpak"),
        ]
    }

    #[test]
    fn test_asset_matching_follows_the_install_kind() {
        let json = release_json("0.7.0", &published_assets());
        let assets = parse_release_info(&json).unwrap().unwrap().1;
        let cases = [
            (
                InstallKind::AppImage(PathBuf::from("/home/me/EloraNotes.AppImage")),
                "linux-x86_64",
                "https://x/appimage",
            ),
            (InstallKind::Portable, "linux-x86_64", "https://x/linux"),
            (
                InstallKind::WindowsSetup,
                "windows-x86_64",
                "https://x/setup",
            ),
            (
                InstallKind::Portable,
                "windows-x86_64",
                "https://x/portable",
            ),
            (
                InstallKind::MacBundle(PathBuf::from("/Applications/EloraNotes.app")),
                "macos-arm64",
                "https://x/macos",
            ),
            (InstallKind::Portable, "macos-arm64", "https://x/macos"),
        ];
        for (kind, platform, expected) in cases {
            let asset = select_asset(&assets, platform, &kind)
                .unwrap_or_else(|| panic!("no asset for {kind:?} on {platform}"));
            assert_eq!(asset.url, expected, "wrong asset for {kind:?}");
        }
    }

    #[test]
    fn test_portable_linux_prefers_the_tarball_over_the_appimage() {
        let json = release_json("0.7.0", &published_assets());
        let assets = parse_release_info(&json).unwrap().unwrap().1;
        let asset = select_asset(&assets, "linux-x86_64", &InstallKind::Portable).unwrap();
        assert_eq!(asset.name, "EloraNotes-0.7.0-linux-x86_64.tar.gz");
    }

    #[test]
    fn test_portable_windows_prefers_the_portable_exe() {
        let json = release_json("0.7.0", &published_assets());
        let assets = parse_release_info(&json).unwrap().unwrap().1;
        let asset = select_asset(&assets, "windows-x86_64", &InstallKind::Portable).unwrap();
        assert_eq!(asset.name, "EloraNotes-0.7.0-windows-x86_64-portable.exe");
        let asset = select_asset(&assets, "windows-x86_64", &InstallKind::WindowsSetup).unwrap();
        assert_eq!(asset.name, "EloraNotes-0.7.0-windows-x86_64-setup.exe");
    }

    #[test]
    fn test_old_asset_names_still_upgrade() {
        // 0.6.x published `elora-notes-<platform>.tar.gz` / `.exe`; those copies must be able
        // to find a portable download in the new release.
        let json = release_json(
            "0.7.0",
            &[
                ("elora-notes-linux-x86_64.tar.gz", "https://x/old-linux"),
                ("elora-notes-windows-x86_64.exe", "https://x/old-windows"),
            ],
        );
        let assets = parse_release_info(&json).unwrap().unwrap().1;
        assert_eq!(
            select_asset(&assets, "linux-x86_64", &InstallKind::Portable)
                .unwrap()
                .url,
            "https://x/old-linux"
        );
        assert_eq!(
            select_asset(&assets, "windows-x86_64", &InstallKind::Portable)
                .unwrap()
                .url,
            "https://x/old-windows"
        );
        // The macOS 0.6.0 tarball is not a bundle, and 0.7.0 has no tarball for it to grab.
        assert!(select_asset(&assets, "macos-arm64", &InstallKind::Portable).is_none());
    }

    #[test]
    fn test_parse_release_ignores_unusable_responses() {
        let kind = InstallKind::Portable;
        assert!(release_for("not json", &kind).is_err());
        assert_eq!(
            release_for(r#"{"tag_name":"nightly","assets":[]}"#, &kind).unwrap(),
            None
        );
        assert_eq!(
            release_for(
                &release_json("0.7.0", &[("something-else.zip", "https://x/y")]),
                &kind
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn test_inno_uninstaller_names_are_recognised() {
        assert!(looks_like_inno_uninstaller("unins000.exe"));
        assert!(looks_like_inno_uninstaller("UNINS001.EXE"));
        assert!(!looks_like_inno_uninstaller("elora.exe"));
        assert!(!looks_like_inno_uninstaller("unins000.dat"));
    }

    #[test]
    fn test_inherited_appimage_is_not_treated_as_ours() {
        // A terminal opened from some AppImage exports APPIMAGE and APPDIR to everything it
        // starts, including this app; only a binary inside that mount point may claim it.
        assert!(exe_lives_in(
            Some(PathBuf::from("/tmp/.mount_Elora123")),
            Some(PathBuf::from("/tmp/.mount_Elora123/usr/bin/elora")),
        ));
        assert!(!exe_lives_in(
            Some(PathBuf::from("/tmp/.mount_Editor456")),
            Some(PathBuf::from("/usr/local/bin/elora")),
        ));
        assert!(!exe_lives_in(
            None,
            Some(PathBuf::from("/usr/local/bin/elora"))
        ));
    }

    #[test]
    fn test_flatpak_has_no_download_of_its_own() {
        assert!(asset_suffixes(&InstallKind::Flatpak, "linux-x86_64").is_none());
        let json = release_json("0.7.0", &published_assets());
        assert_eq!(
            release_for(&json, &InstallKind::Flatpak).unwrap(),
            None,
            "Flatpak copies are updated by flatpak itself"
        );
    }

    #[test]
    fn test_staging_sits_beside_what_it_replaces() {
        assert_eq!(
            staging_path(&InstallKind::AppImage(PathBuf::from(
                "/home/me/Apps/EloraNotes.AppImage"
            )))
            .unwrap(),
            PathBuf::from("/home/me/Apps/.EloraNotes.AppImage.update")
        );
        assert_eq!(
            staging_path(&InstallKind::MacBundle(PathBuf::from(
                "/Applications/EloraNotes.app"
            )))
            .unwrap(),
            PathBuf::from("/Applications/.elora-notes-update")
        );
    }

    #[test]
    fn test_app_zip_is_unpacked_inside_the_target() {
        let dir = std::env::temp_dir().join(format!("elora-update-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options: zip::write::SimpleFileOptions = Default::default();
            writer
                .start_file("EloraNotes.app/Contents/MacOS/elora", options)
                .unwrap();
            std::io::Write::write_all(&mut writer, b"binary").unwrap();
            writer.finish().unwrap();
        }
        extract_app(buffer.get_ref(), &dir).unwrap();
        let app = find_app(&dir).expect("the .app directory");
        assert!(app.join("Contents/MacOS/elora").is_file());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_unsafe_zip_paths_are_rejected() {
        let dir = std::env::temp_dir().join(format!("elora-update-evil-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut buffer = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options: zip::write::SimpleFileOptions = Default::default();
            writer.start_file("../escape", options).unwrap();
            std::io::Write::write_all(&mut writer, b"x").unwrap();
            writer.finish().unwrap();
        }
        assert!(extract_app(buffer.get_ref(), &dir).is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
