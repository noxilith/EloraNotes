# EloraNotes (`elora`)

A calm, minimalistic cross-platform desktop notes application written in **Rust** (`eframe` / `egui`), featuring bundled **Tabler Icons** and designed for **Linux, macOS, and Windows**.

![EloraNotes screenshot](assets/screenshot.png)

## Features

- **Bundled Tabler Icons**: Crisp outline glyphs from the Tabler Icons font embedded directly into the binary (`assets/tabler-icons.ttf`) across navigation, search, tags, checklists, formatting actions, and the command palette.
- **Plain Markdown on Disk**: Every note is stored as a portable `.md` file with lightweight YAML frontmatter (`title`, `pinned`, `trashed`, `tags`, `created_at`, `updated_at`).
- **Command Palette & Quick Switcher (`Ctrl+K`)**: Fuzzy-jump to any note, create notes from templates, switch color themes, change sort order, or toggle views without leaving the keyboard.
- **Bi-Directional `[[Wiki Links]]` & Backlinks**: Reference other notes with `[[Note Title]]` or `[[Note Title|Alias]]`. Click any wiki-link in Preview/Split mode to jump to that note (or create it on the fly), and see all referencing notes in the **Linked from** footer.
- **Edit, Split & Preview Modes**: Toggle between raw Markdown editing, side-by-side **Split View** (`Ctrl+Shift+E`), and clean **Preview Mode** (`Ctrl+E`).
- **Interactive Task Checklists**: Click `- [ ]` and `- [x]` checkboxes directly in Preview or Split mode to toggle completion, with live task progress badges (`2/3`) on note cards and the editor header.
- **Daily Note & Templates (`Ctrl+D`)**: Open or create today's dated journal note in one keystroke, or start from **Meeting Notes** and **Project Spec** templates via the Command Palette.
- **Quick Formatting Bar**: One-click insertion for `# H1`, `## H2`, `**bold**`, `*italic*`, bullet lists, `- [ ]` task checkboxes, blockquotes, code blocks, and `[[Wiki Links]]`.
- **Safe Trash & Restore**: Deleted notes move to the **Trash** view first where they can be restored with one click or permanently deleted.
- **Import Markdown Folder**: Copy `.md` files into an `Import` folder inside your notes folder, then run **Import Markdown From Import Folder** from the Command Palette. Each file becomes a new note with its frontmatter kept. Importing the same files twice creates duplicates.
- **Color Themes**: Choose **Obsidian** (dark slate), **Paper** (warm light), **Nordic** (arctic dark), **Matcha** (soft sage light), or **Matugen** (colors from your wallpaper). Preferences and sort order persist automatically.

## Running EloraNotes

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

By default, notes are saved in `~/Documents/EloraNotes`.

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
