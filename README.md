# EloraNotes (`elora`)

[![CI](https://github.com/noxilith/EloraNotes/actions/workflows/ci.yml/badge.svg)](https://github.com/noxilith/EloraNotes/actions/workflows/ci.yml)
[![Latest release](https://img.shields.io/github/v/release/noxilith/EloraNotes?label=release&color=6aa86a)](https://github.com/noxilith/EloraNotes/releases/latest)
[![License](https://img.shields.io/github/license/noxilith/EloraNotes?color=6aa86a)](LICENSE)
[![Platforms](https://img.shields.io/badge/platforms-linux%20%7C%20macos%20%7C%20windows-6aa86a)](#running-eloranotes)

A calm, minimalistic cross-platform desktop notes application written in **Rust** (`eframe` / `egui`), featuring bundled **Tabler Icons** and designed for **Linux, macOS, and Windows**.

![EloraNotes screenshot](assets/screenshot.png)

## Features

- **Note Icons & Text Color**: Pick a Tabler icon for each note from the header (the sidebar card then shows that glyph), or insert one at the cursor from the formatting bar. Color selected text with the **A Text Color** button, which saves it as `{color:#RRGGBB}…{/color}`.
- **Bundled Tabler Icons**: Crisp outline glyphs from the Tabler Icons font embedded directly into the binary (`assets/tabler-icons.ttf`) across navigation, search, tags, checklists, formatting actions, and the command palette.
- **Stackable Inline Formatting**: Markers nest, so `**==bold inside a glow==**`, `==**glow then bold**==`, `***==triple stack==***` and `{color:#4ECDFF}**colored bold**{/color}` each render as one run carrying several styles at once. Headings are drawn in the bold face and can carry highlights and text colors too.
- **Collapsible Folders**: File any note into a folder from the header's folder menu (or type a new folder name there). Folders group the sidebar, show their note count, and fold shut with a click so the list takes less room. Fold state is remembered between runs. New notes join the folder of the note you had open.
- **Compact Sidebar with One Filter Menu**: The sidebar is a fixed, narrow panel — only the accent **New** button shares the logo row, while the daily note, command palette and settings actions sit in a footer row at the bottom. The notes folder path and keyboard shortcuts are listed in Settings. Every tag filter lives in a single **Filter** dropdown with counts, so a growing tag list never pushes the note list down.
- **Updates Itself from GitHub Releases**: On launch EloraNotes asks GitHub for the latest release. If one is newer it offers to download it, starts the downloaded build once to confirm it runs on this machine, then installs it the way this copy was installed and restarts. **Settings → Updates** has the same thing on demand: check, then **Update and restart**. "Later" dismisses the launch prompt for that run, and a failed launch-time check (for example while offline) stays silent.
- **Plain Markdown on Disk**: Every note is stored as a portable `.md` file with lightweight YAML frontmatter (`title`, `folder`, `pinned`, `trashed`, `tags`, `icon`, `created_at`, `updated_at`).
- **Command Palette & Quick Switcher (`Ctrl+K`)**: Fuzzy-jump to any note, create notes from templates, switch color themes, or toggle views without leaving the keyboard.
- **Bi-Directional `[[Wiki Links]]` & Backlinks**: Reference other notes with `[[Note Title]]` or `[[Note Title|Alias]]`. Click any wiki-link in Preview/Split mode to jump to that note (or create it on the fly), and see all referencing notes in the **Linked from** footer.
- **Edit, Split & Preview Modes**: Toggle between raw Markdown editing, side-by-side **Split View** (`Ctrl+Shift+E`), and clean **Preview Mode** (`Ctrl+E`).
- **Interactive Task Checklists**: Click `- [ ]` and `- [x]` checkboxes directly in Preview or Split mode to toggle completion, with live task progress badges (`2/3`) on note cards and the editor header.
- **Daily Note & Templates (`Ctrl+D`)**: Open or create today's dated journal note in one keystroke, or start from **Meeting Notes** and **Project Spec** templates via the Command Palette.
- **Quick Formatting Bar**: One-click insertion for `# H1`, `## H2`, `**bold**`, `*italic*`, bullet lists, `- [ ]` task checkboxes, blockquotes, code blocks, and `[[Wiki Links]]`.
- **Safe Trash & Restore**: Deleted notes move to the **Trash** view first where they can be restored with one click or permanently deleted.
- **Import Markdown Folder**: Copy `.md` files into an `Import` folder inside your notes folder, then run **Import Markdown From Import Folder** from the Command Palette. Each file becomes a new note with its frontmatter kept. Importing the same files twice creates duplicates.
- **Color Themes**: Choose **Obsidian** (dark slate), **Paper** (warm light), **Nordic** (arctic dark), **Matcha** (soft sage light), or **Matugen** (colors from your wallpaper). Preferences and folded folders persist automatically.

## Install

Every release ships an installer and a portable build per platform.

| Platform | Installer | Portable |
| :--- | :--- | :--- |
| **Linux** | `EloraNotes-<version>-linux-x86_64.flatpak` | `…-linux-x86_64.AppImage` or `…-linux-x86_64.tar.gz` |
| **Windows** | `EloraNotes-<version>-windows-x86_64-setup.exe` | `…-windows-x86_64-portable.exe` |
| **macOS (Apple Silicon)** | `EloraNotes-<version>-macos-arm64.dmg` | `…-macos-arm64.zip` (`EloraNotes.app`) |

### Linux

The Flatpak is the installable build: it appears in your app menu and updates itself from this repository's releases like every other build.

```bash
flatpak install --user ./EloraNotes-<version>-linux-x86_64.flatpak
flatpak run io.github.noxilith.EloraNotes
```

The bundle is self-hosted rather than published to Flathub, so there is no remote for `flatpak update` to pull from — that command will always say there is nothing to do. Updates arrive through the app's own **Update and restart** instead (the sandbox ships a launcher that runs a writable copy of the binary in `~/.local/share/EloraNotes`). To move to a newer bundle by hand anyway, download it and run the same `flatpak install --user` command again; the launcher swaps the writable copy for the one in the new bundle, so the version you install is the version that runs.

The AppImage is the portable build — a single file you can keep anywhere:

```bash
chmod +x EloraNotes-<version>-linux-x86_64.AppImage
./EloraNotes-<version>-linux-x86_64.AppImage
```

The tarball holds the bare `elora` binary for people who prefer to place it on their `PATH`:

```bash
tar -xzf EloraNotes-0.7.0-linux-x86_64.tar.gz
sudo install -Dm755 elora /usr/local/bin/elora
```

### Windows

Run `EloraNotes-0.7.0-windows-x86_64-setup.exe` and follow the wizard. It installs for the current user only (no administrator prompt), adds Start Menu and optional desktop shortcuts, and registers EloraNotes under **Settings → Apps → Installed apps** so it can be uninstalled from there.

The `-portable.exe` needs no install at all: put it in a folder and run it.

### macOS

Open the `.dmg` and drag **EloraNotes** into **Applications**. The `.zip` is the portable build — unzip it and run `EloraNotes.app` from anywhere, including a USB stick.

The builds are not notarised, so the first launch needs **right-click → Open** (or System Settings → Privacy & Security → Open Anyway) once.

### Uninstalling

- **Linux (Flatpak):** `flatpak uninstall io.github.noxilith.EloraNotes`
  (your notes stay in `~/Documents/EloraNotes`; the updatable copy of the app lives in
  `~/.local/share/EloraNotes` and can be deleted once the bundle is gone)
- **Linux (AppImage/tarball):** delete the file
- **Windows:** Settings → Apps → Installed apps → EloraNotes → Uninstall
- **macOS:** drag `EloraNotes.app` from Applications to the Trash

Your notes are plain `.md` files and stay where they are either way.

## Updates

On launch the app compares its version with the latest GitHub release and, when one is newer, offers **Update now** or **Later**. What "update" means depends on how the copy was installed:

- **AppImage** — the new image is downloaded beside the old one, started once to prove it runs, then swapped in and relaunched.
- **Windows setup** — the new setup program runs silently over the old install, so shortcuts and the uninstall entry stay correct; the app restarts itself afterwards.
- **macOS app bundle** — the new `EloraNotes.app` replaces the old one (which is moved aside and cleaned up on the next launch) and reopens.
- **Portable exe / tarball binary** — only the executable is replaced, so it has to live in a writable folder.
- **Flatpak** — the bundle's launcher runs a copy of the app from `~/.local/share/EloraNotes`, which the sandbox keeps writable, so the update replaces that binary and restarts exactly like the portable build. The bundle itself (the sandbox and its icons) stays as installed; `flatpak update` has nothing to fetch because this bundle has no remote.

**Settings → Updates** shows the version you are running, checks on demand and installs the result, so an update never has to wait for the next launch.

## Running From Source

### Quick Launch (Linux / NixOS / macOS)

```bash
./run.sh
```

### Standard Cargo Build (Linux, Windows, macOS)

```bash
cargo run --release
```

Pass a custom notes folder via `--dir` or `ELORA_NOTES_DIR`:

```bash
elora --dir ~/Notes
```

By default, notes and the settings file (`.elora-settings.json`) live in `Documents/EloraNotes` — `C:\Users\<you>\Documents\EloraNotes` on Windows. That folder is used by every install format, including the Flatpak, so the same notes open whichever build you launch. Notes and settings from the older default folders (`~/.config/eloranotes`, `~/eloranotes`, `~/.EloraNotes`) are copied there automatically on first launch. The old folders are left untouched. Files that already exist in the new folder are not overwritten.

## Matugen Theme (Wallpaper Colors)

The **Matugen** theme takes its colors from your wallpaper using [matugen](https://github.com/InioX/matugen).

1. Copy [matugen/config.toml](matugen/config.toml) into `~/.config/matugen/config.toml` and set `input_path` to the `matugen/elora.json` file in this repo.
2. Generate colors from a wallpaper:

   ```bash
   matugen image ~/Pictures/wallpaper.jpg
   ```

   This writes `~/.config/elora/matugen.json`.
3. In EloraNotes, pick **Matugen** in Settings or the Command Palette. The colors load when you select the theme and at startup. After regenerating colors, switch themes and back, or restart the app.

Missing colors fall back to the Obsidian theme.

## Keyboard Shortcuts

| Shortcut | Action |
| :--- | :--- |
| `Ctrl+K` / `Cmd+K` | Open Command Palette & Quick Note Switcher |
| `Ctrl+N` / `Cmd+N` | Create a new blank note |
| `Ctrl+D` / `Cmd+D` | Open or create today's Daily Note |
| `Ctrl+F` / `Cmd+F` | Focus quick search bar |
| `Ctrl+P` / `Cmd+P` | Pin or unpin the current note |
| `Ctrl+E` / `Cmd+E` | Toggle Markdown Preview mode |
| `Ctrl+Shift+E` | Toggle side-by-side Split Editor + Live Preview |
| `Ctrl+\` / `Cmd+\` | Toggle distraction-free Focus Mode |
| `Esc` | Close Command Palette or clear active filters |

## License

EloraNotes is licensed under the GNU General Public License v3.0 or later. See [LICENSE](LICENSE). Bundled fonts keep their own licenses: [DejaVu](assets/DejaVu-LICENSE.txt) and [Tabler Icons](assets/TABLER-ICONS-LICENSE.txt).
