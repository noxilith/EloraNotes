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
- **Collapsible Folders**: File any note into a folder from the header's folder menu (or type a new folder name there). Folders group the sidebar, show their note count, and fold shut with a click so the list takes less room. A folder's notes sit stepped in from its header, so the list shows what belongs to what. Fold state is remembered between runs. New notes join the folder of the note you had open.
- **Compact Sidebar with One Filter Menu**: The sidebar is a fixed, narrow panel — only the accent **New** button shares the logo row, while the daily note, command palette and settings actions sit in a footer row at the bottom. Note cards show just the title, date and tags, so more notes fit at once; **Settings** can add a preview line of each note to them. The notes folder path and keyboard shortcuts are listed in Settings. Every tag filter lives in a single **Filter** dropdown with counts, so a growing tag list never pushes the note list down.
- **Updates Itself from GitHub Releases**: On launch EloraNotes asks GitHub for the latest release. If one is newer it offers to download it, starts the downloaded build once to confirm it runs on this machine, then installs it the way this copy was installed and restarts. **Settings → Updates** has the same thing on demand: check, then **Update and restart**. "Later" dismisses the launch prompt for that run, and a failed launch-time check (for example while offline) stays silent.
- **Plain Markdown on Disk**: Every note is stored as a portable `.md` file with lightweight YAML frontmatter (`title`, `folder`, `pinned`, `trashed`, `tags`, `icon`, `created_at`, `updated_at`).
- **Command Palette & Quick Switcher (`Ctrl+K`)**: Fuzzy-jump to any note, create notes from templates, switch color themes, or toggle views without leaving the keyboard.
- **Bi-Directional `[[Wiki Links]]` & Backlinks**: Reference other notes with `[[Note Title]]` or `[[Note Title|Alias]]`. Click any wiki-link in Preview/Split mode to jump to that note (or create it on the fly), and see all referencing notes in the **Linked from** footer.
- **Edit, Split & Preview Modes**: Toggle between raw Markdown editing, side-by-side **Split View** (`Ctrl+Shift+E`), and clean **Preview Mode** (`Ctrl+E`).
- **Pictures, Added by Button, Drop or Paste and Resized by Dragging**: the picture button in the formatting bar opens the system's file chooser; drop a picture file on the window, or paste a screenshot with `Ctrl+V` while writing, and either way it is copied into an `attachments` folder beside your notes and written into the note at the caret. In a shared note the picture is also sent to the server, so everyone with the library sees it (up to 12 MB each). In Preview or Split mode, drag the square in a picture's bottom-right corner to resize it, the way a picture on a OneNote page behaves; the size is kept in the note as `{width=…}` after the picture, so it is there again next time. `![alt](picture.png)` is drawn the way GitHub draws it — paths are relative to the note's folder, absolute paths and `file://` work too, and `http(s)` links are downloaded in the background. PNG, JPEG and GIF.
- **Interactive Task Checklists**: Click `- [ ]` and `- [x]` checkboxes directly in Preview or Split mode to toggle completion, with live task progress badges (`2/3`) on note cards and the editor header.
- **Daily Note & Templates (`Ctrl+D`)**: Open or create today's dated journal note in one keystroke, or start from **Meeting Notes** and **Project Spec** templates via the Command Palette.
- **Quick Formatting Bar**: One-click insertion for `# H1`, `## H2`, `**bold**`, `*italic*`, bullet lists, `- [ ]` task checkboxes, blockquotes, code blocks, and `[[Wiki Links]]`.
- **Safe Trash & Restore**: Deleted notes move to the **Trash** view first where they can be restored with one click or permanently deleted.
- **Import Markdown Folder**: Copy `.md` files into an `Import` folder inside your notes folder, then run **Import Markdown From Import Folder** from the Command Palette. Each file becomes a new note with its frontmatter kept. Importing the same files twice creates duplicates.
- **Shared Notes (Google-Docs style)**: Point the app at an EloraNotes server — address, port, password — and the server's notes appear in the sidebar under its name. Everyone with a note open sees each other's edits as they are typed, with a coloured caret and a name tag at each person's cursor, a dot on the note's card while somebody is in it, and a presence list above the editor. Each person goes by an **identifier** from Settings, which also lets a note be closed to everybody but its owner or opened to a few names on it.
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

## Shared Notes

One machine runs a small server; everyone else connects to it from the app. The server keeps the
notes as ordinary `.md` files in its own folder, and it speaks TLS with a self-signed certificate
that the client pins on the first connection, so the password never crosses the network in the
clear.

### Hosting the server

Every release ships the server binary plus an installer for Linux and Windows:

| File | What it is |
| :--- | :--- |
| `elora-server-linux-x86_64` | The server binary (no GUI libraries needed) |
| `install-elora-server.sh` | Linux installer |
| `elora-server-windows-x86_64.exe` | The server binary |
| `install-elora-server.ps1` | Windows installer |

**Linux**

```bash
chmod +x install-elora-server.sh
sudo ./install-elora-server.sh
```

**Windows** — open PowerShell as Administrator and run:

```powershell
powershell -ExecutionPolicy Bypass -File .\install-elora-server.ps1
```

The installer asks for the name people see in their sidebar, the port, and the password everyone
connects with. It installs the binary to `/usr/local/bin/elora-server`, or to
`/opt/elora-server/elora-server` on a system where `/usr/local` is not writable.

On a system whose `/etc/systemd/system` is read-only — NixOS and other immutable systems — a
system unit cannot be added, so the installer registers a service for your own account instead:
the notes move to `~/.local/share/elora-server`, lingering is switched on so the server still
starts at boot, and the summary prints the `systemctl --user` commands that manage it. A
certificate left by an earlier interrupted install is carried over, so the code the app asks you
to confirm stays the same. `ELORA_SERVICE_MODE=system` or `ELORA_SERVICE_MODE=user` forces one
of the two. It then installs the binary, creates the notes folder, generates the certificate and
reaches the server at boot (a systemd service on Linux, a startup task on Windows). The server
listens on all interfaces, so it answers on the LAN and on a public address; the installer opens
the port in `ufw`, `firewalld` or Windows Firewall when one of those is in charge. Finally it
prints the addresses to hand out and a short certificate code the first client will ask you to
confirm.

Running the installer again on a machine that already has a server is an update, not a second
install: it prints what it found (service, binary, settings, how many notes, the certificate code)
and asks whether to **update** it, **uninstall** it and keep the notes, **uninstall** it and delete
the notes and the certificate, or **cancel**. An update keeps the notes and the certificate, so the
code clients already trust does not change. `ELORA_ACTION=update`, `uninstall` or `purge` skips the
question for scripted installs (on Windows, set `$env:ELORA_ACTION` first); with no answer to it,
an existing install is updated.

From another network, forward the port on the router once and give people this machine's public IP
or a DNS name. If the machine sits behind CGNAT, tunnel the port (for example with a small VPS
reverse proxy or Tailscale) — the app only needs a host and port that reach it.

Keeping the server running by hand:

```bash
# Linux
systemctl status elora-server
journalctl -u elora-server -f

# Windows
Start-ScheduledTask -TaskName "EloraNotes Server"
Stop-ScheduledTask  -TaskName "EloraNotes Server"
```

On Linux the notes live in `/var/lib/elora-server/notes`; on Windows in
`C:\ProgramData\EloraNotes Server\notes`. Editing those files directly works too — the server
re-reads a note's body whenever it is next opened.

### Putting notes in the shared library

With a server connected, the folder menu on a note (**No folder** in the note's header) lists the
server under **Shared libraries**. Picking it moves the note there: it arrives on the server with
its body, folder, tags, pin and icon, and the local file goes away once it exists on the server.
The other way round, a shared note's folder menu offers **Copy to this computer**, which keeps a
local copy and leaves the shared note in place. While a shared note is open, **New** creates the
next note directly on the server.

Pictures in a shared note are kept on the server too, in an `attachments` folder inside its notes folder, so everyone with the library sees them and they are still there after the computer that added them is gone. A picture added before a note is moved to the server goes with it, and **Copy to this computer** brings copies of its pictures along. The server's notes folder stays an ordinary EloraNotes library, pictures included, so copying or backing it up keeps everything.

Everyone in the library, and the server itself, needs a build that speaks the same protocol (0.9.19 or newer). An older server used to drop a shared note's pictures without saying anything, so a client that can share them refuses to connect to one and says to update whichever side is older. The same check now covers note privacy: a server from before it could not keep a note to itself, so the two are never paired.

### Connecting from the app

**Settings → Shared library**: enter the address (IP or DNS name), the port and the password, and
press **Connect**. On the first connection EloraNotes shows the server's certificate code; it must
match the code the installer printed. After that the key is remembered and a different certificate
is refused, which is what makes a self-signed server safe to use over the internet.

The server's notes appear in the sidebar in a group titled with the server's name, marked with a
cloud glyph. That group has folders of its own: file a shared note from its folder menu and the
folder appears under the server, with its notes stepped in beneath it — further in than the
server's own notes, so a folder on the server is never mistaken for one on this computer.

While a shared note is open, **New** (and the daily-note, template and wiki-link actions) creates
the note in that library instead of on your disk: it joins the same folder, keeps the tag you are
filtering by, and appears on everyone's sidebar straight away.

Your username is your **identifier** from Settings: a fresh install makes one up (the account name
with a short random tail), and it can be replaced with any word. It is the name others see next to
your caret, it is what a note is opened to, and the same identifier on another computer is you
there too, so a note closed to everybody else follows you between machines.

While somebody else has a note open, its card shows a coloured dot; when you open it too you see
their caret with their name, and the words they type appear as they type them.

Which notes are synced: the note bodies and the library structure (titles, folders, tags, pins,
trash). Settings, themes and the update check stay on your own machine.

### Who can see a note

A note made in the shared library is open to everybody on that server, as before. The lock button
in the note's header opens **Who can see this note**:

- **Everyone on <server>** — the whole library, which is where a new note starts.
- **Only me** — nobody else's sidebar lists it, nobody else can open it, and anyone reading it
  when it is closed is taken out of it, so nothing typed afterwards reaches them.
- **Only these people** — the same, plus any identifiers added to the list. Adding a name is all
  it takes: **Their identifier, e.g. SuperSaiyan**. Removing the chip takes the note away again.

Only the note's owner changes this; everybody else sees whose note it is and that it was shared
with them. Ownership is recorded on every note the library creates, in its frontmatter
(`owner:`, `private:`, `shared_with:`) which the server keeps in the `.md` file, so a restriction
survives a restart and stays visible to anyone who edits the folder by hand.

Two things this is not: the identifiers are chosen by the people using them, not proven, and the
server password is still what decides who may connect at all. Treat the lock as *which notes the
server hands out*, for a household or a group who trust each other with the password — not as
protection against somebody who has it and wants to pretend to be you.

### If it stays on "Connecting…"

A wrong password or an outdated client says so by itself, so being stuck at *Connecting…* means
the connection never got through. The app now gives up on an address after a few seconds and
shows why under the state line in Settings; the usual reasons, in the order worth checking:

1. **The server is not running.** On the server machine: `systemctl status elora-server`, then
   `journalctl -u elora-server -n 30 --no-pager`.
2. **It is listening on the wrong address.** `ss -ltn | grep 7331` should show `0.0.0.0:7331`
   (or `[::]:7331`). `127.0.0.1:7331` means only that machine can reach it: reinstall with
   `--bind 0.0.0.0`, or fix `bind` in `~/.config/elora-server/config.json` and restart it.
3. **A firewall drops the port.** The installer opens it in ufw or firewalld; check
   `sudo ufw status` on the server if you use another one.
4. **The router does not forward it.** Forward TCP 7331 to the server machine's LAN address, and
   give that machine a fixed address so the rule keeps pointing at it.
5. **Your connection is behind carrier-grade NAT.** If the WAN address in the router differs from
   `curl -4 ifconfig.me`, no port forward can work; put both machines on a VPN such as Tailscale
   and use the server's address there instead.
6. **The public IP does not work from inside your own network.** Many routers cannot loop back to
   their own WAN address; test from a phone on mobile data before assuming the port is closed.

A DNS name works anywhere an IP does, and the app takes `address:port` in the address field if
you would rather type the port there.

## Running From Source

### Quick Launch (Linux / NixOS / macOS)

```bash
./run.sh
```

### Standard Cargo Build (Linux, Windows, macOS)

```bash
cargo run --release
```

The collaboration server builds on its own, without the GUI libraries a headless machine would
not have:

```bash
cargo build --release --no-default-features --bin elora-server
ELORA_SERVER_DIR=~/.config/elora-server ./target/release/elora-server init
./target/release/elora-server run
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

The Flatpak reads the same `~/.config/elora/matugen.json` (mounted read-only), so the wallpaper colors carry over without a second copy.

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
