//! The Model Context Protocol server behind `elora --mcp`.
//!
//! It lets an AI assistant work in the notebook the way a person works in Obsidian: read and
//! search notes, follow `[[wiki links]]` and backlinks, browse tags and folders, write the daily
//! note. It speaks newline-delimited JSON-RPC over stdin/stdout and works on the same plain `.md`
//! files as the app, through the same `NoteStorage`, so both can be open on one vault. Notes in
//! the shared library are reachable too, through the server settings the app saved.
//!
//! What an assistant may change is limited three ways: the server can be started read-only or
//! confined to one folder, and any single note can be marked read-only for assistants. Before an
//! assistant overwrites or deletes a note, the old text is kept in the note's history.

use crate::mcp_shared::{is_shared, meta_with, note_from_meta, NewNote, Shared};
use crate::net::NoteMeta;
use crate::storage::{
    extract_wiki_links, normalize_folder, normalize_tag, sort_notes, AppSettings, Note,
    NoteStorage, NoteTemplate,
};
use chrono::Utc;
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, Write};

const PROTOCOL_VERSION: &str = "2025-06-18";

/// What a server may do, fixed when it is started.
#[derive(Debug, Clone, Default)]
pub struct VaultOptions {
    /// No tool may change anything.
    pub read_only: bool,
    /// As far as the assistant can tell, only this folder (and the folders inside it) exists.
    /// New notes are filed in it.
    pub folder: Option<String>,
    /// Leave the shared library out even when one is set up.
    pub no_shared: bool,
}

pub struct Vault {
    storage: NoteStorage,
    options: VaultOptions,
    shared: Shared,
    /// Why the shared library could not be reached during the current tool call, if it could not.
    shared_error: RefCell<Option<String>>,
}

type ToolResult = Result<String, String>;

/// The tools that change something, and so the ones a read-only server refuses.
const WRITE_TOOLS: [&str; 8] = [
    "create_note",
    "update_note",
    "rename_note",
    "set_note_properties",
    "trash_note",
    "restore_note",
    "delete_note",
    "daily_note",
];

impl Vault {
    pub fn new(storage: NoteStorage) -> Self {
        Self::with_options(storage, VaultOptions::default())
    }

    pub fn with_options(storage: NoteStorage, options: VaultOptions) -> Self {
        let options = VaultOptions {
            folder: options.folder.as_deref().and_then(normalize_folder),
            ..options
        };
        Self {
            storage,
            options,
            shared: Shared::new(),
            shared_error: RefCell::new(None),
        }
    }

    fn settings(&self) -> AppSettings {
        self.storage.load_settings()
    }

    /// The notes kept on this computer, freshly read, so edits made in the app are seen. Unlike
    /// the app, an empty folder stays empty rather than being filled with the starter notes.
    fn local_notes(&self) -> Result<Vec<Note>, String> {
        let has_notes = fs::read_dir(self.storage.root_dir())
            .map(|entries| {
                entries
                    .flatten()
                    .any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("md"))
            })
            .unwrap_or(false);
        if !has_notes {
            return Ok(Vec::new());
        }
        self.storage.load_all().map_err(|e| e.to_string())
    }

    /// Every note the assistant may see: those on this computer, and the shared library's (by
    /// their properties; a shared note's text is only fetched when it is asked for).
    fn notes(&self) -> Result<Vec<Note>, String> {
        *self.shared_error.borrow_mut() = None;
        let settings = self.settings();
        let mut notes = self.local_notes()?;
        if !self.options.no_shared {
            match self.shared.library(&settings) {
                Ok(Some(metas)) => notes.extend(metas.iter().map(|meta| {
                    let mut note = note_from_meta(meta, String::new());
                    note.ai.read_only = settings.mcp_read_only_ids.contains(&note.id);
                    note
                })),
                Ok(None) => {}
                Err(why) => *self.shared_error.borrow_mut() = Some(why),
            }
        }
        notes.retain(|n| self.in_scope(n));
        sort_notes(&mut notes);
        Ok(notes)
    }

    fn in_scope(&self, note: &Note) -> bool {
        match &self.options.folder {
            Some(scope) => note
                .folder
                .as_deref()
                .is_some_and(|folder| prefix_match(folder, scope)),
            None => true,
        }
    }

    /// Files a new or changed note inside the folder this server is confined to.
    fn place(&self, note: &mut Note) -> Result<(), String> {
        let Some(scope) = &self.options.folder else {
            return Ok(());
        };
        match note.folder.clone() {
            None => note.folder = Some(scope.clone()),
            Some(folder) if prefix_match(&folder, scope) => {}
            Some(_) => return Err(format!("this server is limited to the folder \"{scope}\"")),
        }
        Ok(())
    }

    /// Refuses to change a note its owner has marked read-only for assistants.
    fn writable(&self, note: &Note) -> Result<(), String> {
        if note.ai.read_only {
            return Err(format!(
                "\"{}\" is marked read-only for assistants; its owner can allow edits from the \
                 note's menu in EloraNotes",
                note.display_title()
            ));
        }
        Ok(())
    }

    /// Writes a note kept on this computer. What was there is kept in its history first, and a
    /// note that already existed is marked as last changed by an assistant.
    fn save(&self, note: &mut Note) -> Result<(), String> {
        if note.file_path.exists() {
            let _ = self.storage.snapshot(note);
            note.ai.edited = true;
        }
        self.storage
            .save_note(note)
            .map_err(|e| format!("could not save \"{}\": {e}", note.title))
    }

    /// The note with its text, fetching a shared note's text from the library.
    fn with_body(&self, note: &Note) -> Result<Note, String> {
        let mut full = note.clone();
        if is_shared(note) {
            full.body = self.shared.read_body(&self.settings(), &note.id)?;
        }
        Ok(full)
    }

    /// The shared library's own record of a note, for changing its properties.
    fn meta_of(&self, id: &str) -> Result<NoteMeta, String> {
        self.shared
            .library(&self.settings())?
            .and_then(|metas| metas.into_iter().find(|m| m.id == id))
            .ok_or_else(|| format!("the shared library no longer has a note with id {id}"))
    }

    /// Adds why the shared library was left out, when it was.
    fn annotate(&self, mut out: Value) -> String {
        if let Some(why) = self.shared_error.borrow().as_ref() {
            out["shared_library_unavailable"] = json!(why);
        }
        out.to_string()
    }

    pub fn call(&self, name: &str, args: &Value) -> ToolResult {
        if self.options.read_only && WRITE_TOOLS.contains(&name) {
            return Err(format!(
                "this server was started read-only, so {name} is not available"
            ));
        }
        match name {
            "vault_info" => self.vault_info(),
            "list_notes" => self.list_notes(args),
            "read_note" => self.read_note(args),
            "search_notes" => self.search_notes(args),
            "create_note" => self.create_note(args),
            "update_note" => self.update_note(args),
            "rename_note" => self.rename_note(args),
            "set_note_properties" => self.set_properties(args),
            "trash_note" => self.set_trashed(args, true),
            "restore_note" => self.set_trashed(args, false),
            "delete_note" => self.delete_note(args),
            "get_backlinks" => self.backlinks(args),
            "get_links" => self.links(args),
            "list_tags" => self.list_tags(),
            "list_folders" => self.list_folders(),
            "daily_note" => self.daily_note(args),
            "find_unresolved_links" => self.unresolved_links(),
            other => Err(format!("unknown tool \"{other}\"")),
        }
    }

    fn vault_info(&self) -> ToolResult {
        let notes = self.notes()?;
        let live = notes.iter().filter(|n| !n.trashed);
        let words: usize = live.clone().map(Note::word_count).sum();
        let mut out = json!({
            "path": self.storage.root_dir().display().to_string(),
            "notes": live.clone().count(),
            "trashed": notes.iter().filter(|n| n.trashed).count(),
            "pinned": live.filter(|n| n.pinned).count(),
            "words": words,
            "read_only_server": self.options.read_only,
            "folder_limit": self.options.folder,
        });
        if !self.options.no_shared {
            if let Some(status) = self.shared.status(&self.settings()) {
                out["shared_library"] = json!(status);
            }
        }
        Ok(self.annotate(out))
    }

    fn list_notes(&self, args: &Value) -> ToolResult {
        let folder = opt_str(args, "folder");
        let tag = opt_str(args, "tag").map(normalize_tag);
        let trashed = opt_bool(args, "trashed").unwrap_or(false);
        let limit = opt_usize(args, "limit").unwrap_or(200);
        let rows: Vec<Value> = self
            .notes()?
            .iter()
            .filter(|n| n.trashed == trashed)
            .filter(|n| match folder {
                Some(f) => n.folder.as_deref().is_some_and(|nf| prefix_match(nf, f)),
                None => true,
            })
            .filter(|n| match &tag {
                Some(t) => n.all_tags().contains(t),
                None => true,
            })
            .take(limit)
            .map(summary)
            .collect();
        Ok(self.annotate(json!({ "count": rows.len(), "notes": rows })))
    }

    fn read_note(&self, args: &Value) -> ToolResult {
        let notes = self.notes()?;
        let note = self.with_body(find(&notes, req_str(args, "note")?)?)?;
        let backlinks: Vec<String> = backlinks_of(&notes, &note)
            .iter()
            .map(|n| n.display_title())
            .collect();
        let mut out = summary(&note);
        out["body"] = json!(note.body);
        out["links"] = json!(note.outbound_links());
        out["backlinks"] = json!(backlinks);
        out["tasks"] = {
            let (done, total) = note.task_progress();
            json!({ "done": done, "total": total })
        };
        Ok(out.to_string())
    }

    fn search_notes(&self, args: &Value) -> ToolResult {
        let query = req_str(args, "query")?;
        let limit = opt_usize(args, "limit").unwrap_or(20);
        let include_trashed = opt_bool(args, "include_trashed").unwrap_or(false);
        let mut hits: Vec<(usize, Note)> = self
            .notes()?
            .into_iter()
            .filter(|n| include_trashed || !n.trashed)
            .filter(|n| n.matches_filter(query, None))
            .map(|n| (n.relevance_score(query), n))
            .collect();
        hits.sort_by_key(|hit| std::cmp::Reverse(hit.0));
        let rows: Vec<Value> = hits
            .iter()
            .take(limit)
            .map(|(score, n)| {
                let mut row = summary(n);
                row["score"] = json!(score);
                row["excerpt"] = json!(n.body_snippet(query).unwrap_or_else(|| n.snippet()));
                row
            })
            .collect();
        Ok(self.annotate(json!({ "count": rows.len(), "results": rows })))
    }

    fn create_note(&self, args: &Value) -> ToolResult {
        let title = req_str(args, "title")?.trim();
        if title.is_empty() {
            return Err("title must not be empty".into());
        }
        let shared = opt_bool(args, "shared").unwrap_or(false);
        if shared && self.options.no_shared {
            return Err("this server was started without the shared library".into());
        }
        let notes = self.notes()?;
        if note_for_title(&notes, title).is_some() {
            return Err(format!(
                "a note called \"{title}\" already exists; use update_note to change it"
            ));
        }
        let body = opt_str(args, "body").unwrap_or("");
        let mut note = Note::new(title, body, self.storage.root_dir());
        // A note an assistant made is marked with a robot until someone picks another icon.
        note.icon = Some(ROBOT_ICON.to_string());
        note.ai.created = true;
        apply_properties(&mut note, args)?;
        self.place(&mut note)?;
        if shared {
            let meta = self.shared.create(
                &self.settings(),
                &NewNote {
                    title,
                    folder: note.folder.as_deref(),
                    tags: &note.explicit_tags,
                    body,
                    pinned: note.pinned,
                    icon: note.icon.as_deref(),
                },
            )?;
            let made = note_from_meta(&meta, body.to_string());
            return Ok(json!({ "created": summary(&made) }).to_string());
        }
        self.save(&mut note)?;
        Ok(json!({ "created": summary(&note) }).to_string())
    }

    /// Replaces, appends to or prepends to a note's text. Exactly one of the three is given.
    fn update_note(&self, args: &Value) -> ToolResult {
        // Checked before anything is fetched, so a badly formed call fails at once.
        new_body("", args)?;
        let notes = self.notes()?;
        let mut note = find(&notes, req_str(args, "note")?)?.clone();
        self.writable(&note)?;
        if is_shared(&note) {
            let mut failure = None;
            note.body =
                self.shared.edit_body(&self.settings(), &note.id, |old| {
                    match new_body(old, args) {
                        Ok(text) => text,
                        Err(why) => {
                            failure = Some(why);
                            old.to_string()
                        }
                    }
                })?;
            if let Some(why) = failure {
                return Err(why);
            }
        } else {
            note.body = new_body(&note.body, args)?;
            note.updated_at = Utc::now();
            self.save(&mut note)?;
        }
        Ok(json!({ "updated": summary(&note) }).to_string())
    }

    /// Renames a note and rewrites the `[[links]]` that pointed at the old name, as Obsidian does.
    /// Links inside shared-library notes are not rewritten, and neither are those in notes
    /// marked read-only for assistants.
    fn rename_note(&self, args: &Value) -> ToolResult {
        let new_title = req_str(args, "new_title")?.trim();
        if new_title.is_empty() {
            return Err("new_title must not be empty".into());
        }
        let notes = self.notes()?;
        let target = find(&notes, req_str(args, "note")?)?.clone();
        self.writable(&target)?;
        if let Some(other) = note_for_title(&notes, new_title) {
            if other.id != target.id {
                return Err(format!("a note called \"{new_title}\" already exists"));
            }
        }
        let old_title = target.display_title();
        if is_shared(&target) {
            let mut renamed = target.clone();
            renamed.title = new_title.to_string();
            let meta = self.meta_of(&target.id)?;
            self.shared
                .update(&self.settings(), &meta_with(&meta, &renamed))?;
        }
        let mut relinked = Vec::new();
        let mut skipped = Vec::new();
        for mut note in notes.into_iter().filter(|n| !n.trashed && !is_shared(n)) {
            if note.id == target.id {
                note.title = new_title.to_string();
                note.updated_at = Utc::now();
                self.save(&mut note)?;
            } else if let Some(body) = rewrite_links(&note.body, &old_title, new_title) {
                if note.ai.read_only {
                    skipped.push(note.display_title());
                    continue;
                }
                note.body = body;
                note.updated_at = Utc::now();
                self.save(&mut note)?;
                relinked.push(note.display_title());
            }
        }
        let mut out = json!({ "renamed": { "from": old_title, "to": new_title }, "links_updated_in": relinked });
        if !skipped.is_empty() {
            out["links_not_updated_in_read_only_notes"] = json!(skipped);
        }
        Ok(out.to_string())
    }

    fn set_properties(&self, args: &Value) -> ToolResult {
        let notes = self.notes()?;
        let mut note = find(&notes, req_str(args, "note")?)?.clone();
        self.writable(&note)?;
        apply_properties(&mut note, args)?;
        self.place(&mut note)?;
        note.updated_at = Utc::now();
        if is_shared(&note) {
            let meta = self.meta_of(&note.id)?;
            let updated = self
                .shared
                .update(&self.settings(), &meta_with(&meta, &note))?;
            note = note_from_meta(&updated, String::new());
        } else {
            self.save(&mut note)?;
        }
        Ok(json!({ "updated": summary(&note) }).to_string())
    }

    fn set_trashed(&self, args: &Value, trashed: bool) -> ToolResult {
        let notes = self.notes()?;
        let key = req_str(args, "note")?;
        let mut note = if trashed {
            find(&notes, key)?.clone()
        } else {
            find_in(&notes, key, true)?.clone()
        };
        self.writable(&note)?;
        note.trashed = trashed;
        if trashed {
            note.pinned = false;
        }
        note.updated_at = Utc::now();
        if is_shared(&note) {
            let meta = self.meta_of(&note.id)?;
            let updated = self
                .shared
                .update(&self.settings(), &meta_with(&meta, &note))?;
            note = note_from_meta(&updated, String::new());
        } else {
            self.save(&mut note)?;
        }
        Ok(json!({ if trashed { "trashed" } else { "restored" }: summary(&note) }).to_string())
    }

    /// Permanent deletion is only for notes already in the trash, and has to be confirmed. A
    /// note kept on this computer is copied into its history first, so it can still be found.
    fn delete_note(&self, args: &Value) -> ToolResult {
        if opt_bool(args, "confirm") != Some(true) {
            return Err("deleting is permanent: pass confirm: true".into());
        }
        let notes = self.notes()?;
        let note = find_in(&notes, req_str(args, "note")?, true)?;
        if !note.trashed {
            return Err("only a note in the trash can be deleted; use trash_note first".into());
        }
        self.writable(note)?;
        if is_shared(note) {
            self.shared.delete(&self.settings(), &note.id)?;
        } else {
            let _ = self.storage.snapshot(note);
            self.storage.delete_note(note).map_err(|e| e.to_string())?;
        }
        Ok(json!({ "deleted": note.display_title() }).to_string())
    }

    fn backlinks(&self, args: &Value) -> ToolResult {
        let notes = self.notes()?;
        let note = find(&notes, req_str(args, "note")?)?;
        let rows: Vec<Value> = backlinks_of(&notes, note)
            .into_iter()
            .map(summary)
            .collect();
        Ok(json!({ "note": note.display_title(), "backlinks": rows }).to_string())
    }

    fn links(&self, args: &Value) -> ToolResult {
        let notes = self.notes()?;
        let note = self.with_body(find(&notes, req_str(args, "note")?)?)?;
        let rows: Vec<Value> = note
            .outbound_links()
            .iter()
            .map(|link| match note_for_title(&notes, link) {
                Some(t) => json!({ "link": link, "resolved": true, "id": t.id }),
                None => json!({ "link": link, "resolved": false }),
            })
            .collect();
        Ok(json!({ "note": note.display_title(), "links": rows }).to_string())
    }

    fn list_tags(&self) -> ToolResult {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for note in self.notes()?.iter().filter(|n| !n.trashed) {
            for tag in note.all_tags() {
                *counts.entry(tag).or_default() += 1;
            }
        }
        Ok(self.annotate(json!({ "tags": counts })))
    }

    fn list_folders(&self) -> ToolResult {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for note in self.notes()?.iter().filter(|n| !n.trashed) {
            if let Some(folder) = &note.folder {
                *counts.entry(folder.clone()).or_default() += 1;
            }
        }
        Ok(self.annotate(json!({ "folders": counts })))
    }

    /// Today's note, made from the app's daily template when it does not exist yet. With `append`
    /// the text is added to the end, which is how a log gets written through the day.
    fn daily_note(&self, args: &Value) -> ToolResult {
        let notes = self.notes()?;
        let template = NoteTemplate::DailyJournal.build(self.storage.root_dir());
        let (mut note, created) = match note_for_title(&notes, &template.title) {
            Some(existing) => (existing.clone(), false),
            None => (template, true),
        };
        let append = opt_str(args, "append");
        if created {
            if note.icon.is_none() {
                note.icon = Some(ROBOT_ICON.to_string());
            }
            note.ai.created = true;
            self.place(&mut note)?;
        } else if append.is_some() {
            self.writable(&note)?;
        }
        if let Some(text) = append {
            note.body = join(&note.body, text, true);
            note.updated_at = Utc::now();
        }
        if created || append.is_some() {
            self.save(&mut note)?;
        }
        let mut out = summary(&note);
        out["body"] = json!(note.body);
        out["created"] = json!(created);
        Ok(out.to_string())
    }

    fn unresolved_links(&self) -> ToolResult {
        let notes = self.notes()?;
        let mut rows = Vec::new();
        for note in notes.iter().filter(|n| !n.trashed) {
            for link in note.outbound_links() {
                if note_for_title(&notes, &link).is_none() {
                    rows.push(json!({ "from": note.display_title(), "link": link }));
                }
            }
        }
        Ok(self.annotate(json!({ "count": rows.len(), "unresolved": rows })))
    }
}

/// The Tabler icon given to a note made through this server.
const ROBOT_ICON: &str = "robot";

// --- helpers ---------------------------------------------------------------------------------

fn summary(note: &Note) -> Value {
    json!({
        "id": note.id,
        "title": note.display_title(),
        "folder": note.folder,
        "tags": note.all_tags(),
        "icon": note.icon,
        "pinned": note.pinned,
        "trashed": note.trashed,
        "words": note.word_count(),
        "created": note.created_at.to_rfc3339(),
        "updated": note.updated_at.to_rfc3339(),
        "shared": is_shared(note),
        "path": (!is_shared(note)).then(|| note.file_path.display().to_string()),
        // Whether assistants may change this note, and whether one made it.
        "read_only": note.ai.read_only,
        "made_by_ai": note.ai.created,
    })
}

/// Joins two pieces of text with a blank line between them, unless one is empty.
fn join(body: &str, text: &str, after: bool) -> String {
    let (first, second) = if after { (body, text) } else { (text, body) };
    if first.trim().is_empty() {
        return second.to_string();
    }
    if second.trim().is_empty() {
        return first.to_string();
    }
    format!(
        "{}\n\n{}",
        first.trim_end(),
        second.trim_start_matches('\n')
    )
}

/// The note a `[[link]]` or a title means: a live note with that title, ignoring case.
fn note_for_title<'a>(notes: &'a [Note], title: &str) -> Option<&'a Note> {
    let wanted = title.trim();
    notes.iter().find(|n| {
        !n.trashed
            && (n.title.eq_ignore_ascii_case(wanted)
                || n.display_title().eq_ignore_ascii_case(wanted))
    })
}

fn find<'a>(notes: &'a [Note], key: &str) -> Result<&'a Note, String> {
    find_in(notes, key, false)
}

/// Finds a note by id (its file name) or by title. A live note wins over one in the trash with
/// the same title; the trash is only searched with `trashed_ok`.
fn find_in<'a>(notes: &'a [Note], key: &str, trashed_ok: bool) -> Result<&'a Note, String> {
    let key = key.trim().trim_start_matches("[[").trim_end_matches("]]");
    let key = key.split('|').next().unwrap_or(key).trim();
    let key = key.strip_suffix(".md").unwrap_or(key);
    notes
        .iter()
        .find(|n| n.id == key && (trashed_ok || !n.trashed))
        .or_else(|| note_for_title(notes, key))
        .or_else(|| {
            trashed_ok
                .then(|| {
                    notes
                        .iter()
                        .find(|n| n.display_title().eq_ignore_ascii_case(key))
                })
                .flatten()
        })
        .ok_or_else(|| format!("no note called \"{key}\""))
}

fn backlinks_of<'a>(notes: &'a [Note], target: &Note) -> Vec<&'a Note> {
    let title = target.display_title().to_lowercase();
    notes
        .iter()
        .filter(|n| !n.trashed && n.id != target.id && n.body.contains("[["))
        .filter(|n| n.outbound_links().iter().any(|l| l.to_lowercase() == title))
        .collect()
}

/// Applies the optional `tags`, `folder`, `pinned` and `icon` arguments. An empty folder or an
/// empty icon clears it. The read-only mark is not among them: an assistant can never lift the
/// protection a person put on a note.
fn apply_properties(note: &mut Note, args: &Value) -> Result<(), String> {
    if let Some(tags) = args.get("tags").and_then(Value::as_array) {
        let mut cleaned: Vec<String> = Vec::new();
        for tag in tags.iter().filter_map(Value::as_str).map(normalize_tag) {
            if !tag.is_empty() && !cleaned.contains(&tag) {
                cleaned.push(tag);
            }
        }
        note.explicit_tags = cleaned;
    }
    if let Some(folder) = opt_str(args, "folder") {
        note.folder = normalize_folder(folder);
    }
    if let Some(pinned) = opt_bool(args, "pinned") {
        note.pinned = pinned && !note.trashed;
    }
    if let Some(icon) = opt_str(args, "icon") {
        let name: String = icon
            .trim()
            .to_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        if name.is_empty() {
            note.icon = None;
        } else if known_icon(&name) {
            note.icon = Some(name);
        } else {
            return Err(format!(
                "unknown icon \"{icon}\"; use a Tabler icon name such as \"robot\" or \"flare\""
            ));
        }
    }
    Ok(())
}

/// True for a name in the Tabler set the app draws. A build without the window has no list to
/// check against, so it takes the name as it is.
fn known_icon(name: &str) -> bool {
    #[cfg(feature = "gui")]
    {
        crate::icon_names::TABLER_ICON_NAMES
            .binary_search_by_key(&name, |(n, _)| n)
            .is_ok()
    }
    #[cfg(not(feature = "gui"))]
    {
        !name.is_empty()
    }
}

/// The text a note should have after `update_note`: exactly one of `body` (replace), `append`
/// or `prepend` is given.
fn new_body(old: &str, args: &Value) -> Result<String, String> {
    match (
        opt_str(args, "body"),
        opt_str(args, "append"),
        opt_str(args, "prepend"),
    ) {
        (Some(text), None, None) => Ok(text.to_string()),
        (None, Some(text), None) => Ok(join(old, text, true)),
        (None, None, Some(text)) => Ok(join(old, text, false)),
        _ => Err("give exactly one of body, append or prepend".into()),
    }
}

/// True when `folder` is `wanted` or sits inside it (`Work/Clients` is inside `Work`).
fn prefix_match(folder: &str, wanted: &str) -> bool {
    let (folder, wanted) = (
        folder.to_lowercase(),
        wanted.trim_matches('/').to_lowercase(),
    );
    folder == wanted || folder.starts_with(&format!("{wanted}/"))
}

/// The body with every `[[old]]` / `[[old|alias]]` turned into `[[new]]` / `[[new|alias]]`, or
/// `None` when nothing pointed at `old`. Fenced code is left alone, as when links are read.
fn rewrite_links(body: &str, old: &str, new: &str) -> Option<String> {
    let mut changed = false;
    let mut in_code = false;
    let mut out = String::with_capacity(body.len());
    for line in body.split_inclusive('\n') {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
        }
        if in_code || !line.contains("[[") {
            out.push_str(line);
            continue;
        }
        let mut rest = line;
        while let Some(start) = rest.find("[[") {
            let after = &rest[start + 2..];
            let Some(end) = after.find("]]") else { break };
            let inner = &after[..end];
            let (target, alias) = match inner.split_once('|') {
                Some((t, a)) => (t, Some(a)),
                None => (inner, None),
            };
            out.push_str(&rest[..start + 2]);
            if target.trim().eq_ignore_ascii_case(old) {
                changed = true;
                out.push_str(new);
                if let Some(alias) = alias {
                    out.push('|');
                    out.push_str(alias);
                }
            } else {
                out.push_str(inner);
            }
            out.push_str("]]");
            rest = &after[end + 2..];
        }
        out.push_str(rest);
    }
    debug_assert!(!changed || extract_wiki_links(&out).iter().any(|l| l == new));
    changed.then_some(out)
}

fn req_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    opt_str(args, key).ok_or_else(|| format!("missing required argument \"{key}\""))
}

fn opt_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

fn opt_bool(args: &Value, key: &str) -> Option<bool> {
    args.get(key).and_then(Value::as_bool)
}

fn opt_usize(args: &Value, key: &str) -> Option<usize> {
    args.get(key).and_then(Value::as_u64).map(|n| n as usize)
}

// --- protocol --------------------------------------------------------------------------------

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "properties": properties, "required": required },
    })
}

fn note_arg() -> Value {
    json!({ "type": "string", "description": "A note's title (case-insensitive) or its id (the file name without .md)" })
}

pub fn tool_list() -> Vec<Value> {
    let tags = json!({ "type": "array", "items": { "type": "string" }, "description": "Tags, without the #. Replaces the note's tags." });
    let folder = json!({ "type": "string", "description": "Folder to file the note under, e.g. \"Work/Clients\". Empty string clears it." });
    let pinned = json!({ "type": "boolean" });
    let icon = json!({ "type": "string", "description": "A Tabler icon name, e.g. \"robot\" or \"flare\". Empty string clears it." });
    vec![
        tool("vault_info", "Where the vault is, how many notes it holds, and what this server may do (read-only, limited to a folder, shared library status).", json!({}), &[]),
        tool(
            "list_notes",
            "List notes (newest first, pinned first), optionally within a folder or carrying a tag. Does not return note bodies. Notes in the shared library are listed too, marked shared.",
            json!({
                "folder": { "type": "string", "description": "Only this folder and the folders inside it" },
                "tag": { "type": "string" },
                "trashed": { "type": "boolean", "description": "List the trash instead" },
                "limit": { "type": "integer", "description": "Default 200" },
            }),
            &[],
        ),
        tool(
            "read_note",
            "Read a note: its Markdown body, tags, folder, outgoing [[links]] and the notes that link back to it. Also reads shared-library notes.",
            json!({ "note": note_arg() }),
            &["note"],
        ),
        tool(
            "search_notes",
            "Search titles, bodies and tags. Words are ANDed; a #tag word filters by tag. Returns ranked results with an excerpt. Shared-library notes are matched on title, tags and folder only; read_note shows their text.",
            json!({
                "query": { "type": "string" },
                "limit": { "type": "integer", "description": "Default 20" },
                "include_trashed": { "type": "boolean" },
            }),
            &["query"],
        ),
        tool(
            "create_note",
            "Create a Markdown note. Link to other notes with [[Note Title]]; #tags in the body count as tags. Made notes get a robot icon unless icon is given. With shared: true the note is made in the shared library instead of on this computer.",
            json!({ "title": { "type": "string" }, "body": { "type": "string" }, "tags": tags, "folder": folder, "pinned": pinned, "icon": icon, "shared": { "type": "boolean", "description": "Make the note in the shared library" } }),
            &["title"],
        ),
        tool(
            "update_note",
            "Change a note's text. Give exactly one of body (replace everything), append or prepend (add to the end or start). The previous text of a note on this computer is kept in its history. Refused for notes marked read-only for assistants.",
            json!({
                "note": note_arg(),
                "body": { "type": "string" },
                "append": { "type": "string" },
                "prepend": { "type": "string" },
            }),
            &["note"],
        ),
        tool(
            "rename_note",
            "Rename a note and rewrite every [[link]] to it in notes on this computer (links inside shared-library notes are left as they are).",
            json!({ "note": note_arg(), "new_title": { "type": "string" } }),
            &["note", "new_title"],
        ),
        tool(
            "set_note_properties",
            "Set a note's tags, folder, pinned state or icon. Only the arguments given are changed.",
            json!({ "note": note_arg(), "tags": tags, "folder": folder, "pinned": pinned, "icon": icon }),
            &["note"],
        ),
        tool("trash_note", "Move a note to the trash. It can be restored.", json!({ "note": note_arg() }), &["note"]),
        tool("restore_note", "Bring a note back from the trash.", json!({ "note": note_arg() }), &["note"]),
        tool(
            "delete_note",
            "Permanently delete a note that is already in the trash. Cannot be undone.",
            json!({ "note": note_arg(), "confirm": { "type": "boolean", "description": "Must be true" } }),
            &["note", "confirm"],
        ),
        tool("get_backlinks", "The notes that link to this note with [[its title]].", json!({ "note": note_arg() }), &["note"]),
        tool(
            "get_links",
            "The [[links]] a note makes, each marked resolved or not (a link to a note that does not exist yet).",
            json!({ "note": note_arg() }),
            &["note"],
        ),
        tool("list_tags", "Every tag in the vault with the number of notes carrying it.", json!({}), &[]),
        tool("list_folders", "Every folder with the number of notes in it.", json!({}), &[]),
        tool(
            "daily_note",
            "Today's daily note (\"Daily Note — YYYY-MM-DD\"), created from the app's template if it does not exist. With append, adds text to the end.",
            json!({ "append": { "type": "string" } }),
            &[],
        ),
        tool("find_unresolved_links", "Every [[link]] that points at a note that does not exist.", json!({}), &[]),
    ]
}

/// Answers one JSON-RPC message. `None` for a notification, which gets no reply.
pub fn handle(vault: &Vault, message: &Value) -> Option<Value> {
    let id = message.get("id")?.clone();
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let reply = |result: Value| json!({ "jsonrpc": "2.0", "id": id, "result": result });
    let failure = |code: i64, text: &str| json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": text } });
    Some(match method {
        "initialize" => reply(json!({
            "protocolVersion": params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(PROTOCOL_VERSION),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "elora-notes", "version": env!("CARGO_PKG_VERSION") },
            "instructions": "Tools for an EloraNotes vault: Markdown notes with YAML frontmatter, [[wiki links]], backlinks, #tags and folders, like Obsidian. Refer to notes by title or id. Notes marked read-only (read_only: true) cannot be changed, and the server itself may be read-only or limited to a folder; vault_info says which. The shared library's notes are included when one is set up.",
        })),
        "ping" => reply(json!({})),
        "tools/list" => reply(json!({ "tools": tool_list() })),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(failure(-32602, "tools/call needs a tool name"));
            };
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let (text, is_error) = match vault.call(name, &args) {
                Ok(text) => (text, false),
                Err(text) => (text, true),
            };
            reply(json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }))
        }
        other => failure(-32601, &format!("method not found: {other}")),
    })
}

/// Reads requests from stdin until it closes and writes each reply to stdout.
pub fn serve(vault: Vault) -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(&line) {
            Ok(Value::Array(batch)) => {
                let replies: Vec<Value> = batch.iter().filter_map(|m| handle(&vault, m)).collect();
                (!replies.is_empty()).then_some(Value::Array(replies))
            }
            Ok(message) => handle(&vault, &message),
            Err(_) => Some(json!({
                "jsonrpc": "2.0", "id": null,
                "error": { "code": -32700, "message": "parse error" },
            })),
        };
        if let Some(response) = response {
            writeln!(stdout, "{response}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    fn vault() -> Vault {
        let dir = env::temp_dir().join(format!(
            "elora_mcp_test_{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        Vault::new(NoteStorage::new(dir).unwrap())
    }

    fn call(v: &Vault, tool: &str, args: Value) -> Value {
        serde_json::from_str(&v.call(tool, &args).expect("tool failed")).unwrap()
    }

    #[test]
    fn an_empty_vault_stays_empty() {
        let v = vault();
        assert_eq!(call(&v, "vault_info", json!({}))["notes"], 0);
    }

    #[test]
    fn create_read_and_backlinks() {
        let v = vault();
        call(
            &v,
            "create_note",
            json!({ "title": "Alpha", "body": "Hello #one", "tags": ["x"] }),
        );
        call(
            &v,
            "create_note",
            json!({ "title": "Beta", "body": "See [[alpha]]" }),
        );
        let alpha = call(&v, "read_note", json!({ "note": "ALPHA" }));
        assert_eq!(alpha["backlinks"], json!(["Beta"]));
        assert_eq!(alpha["tags"], json!(["one", "x"]));
        assert!(v.call("create_note", &json!({ "title": "alpha" })).is_err());
    }

    #[test]
    fn notes_made_here_wear_a_robot_icon() {
        let v = vault();
        let made = call(&v, "create_note", json!({ "title": "Bot note" }));
        assert_eq!(made["created"]["icon"], "robot");
        let on_disk = fs::read_to_string(made["created"]["path"].as_str().unwrap()).unwrap();
        assert!(on_disk.contains("\nicon: robot\n"));
        // Editing it later keeps the icon.
        call(
            &v,
            "update_note",
            json!({ "note": "Bot note", "append": "more" }),
        );
        assert_eq!(
            call(&v, "read_note", json!({ "note": "Bot note" }))["icon"],
            "robot"
        );
        let daily = call(&v, "daily_note", json!({}));
        assert_eq!(daily["icon"], "robot");
    }

    /// A vault with the given options, and a second handle on its folder for setting up notes
    /// the way the app would have written them.
    fn vault_with(options: VaultOptions) -> (Vault, NoteStorage) {
        let dir = env::temp_dir().join(format!(
            "elora_mcp_opts_{}_{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let vault = Vault::with_options(NoteStorage::new(dir.clone()).unwrap(), options);
        (vault, NoteStorage::new(dir).unwrap())
    }

    fn fails(v: &Vault, tool: &str, args: Value) -> String {
        v.call(tool, &args)
            .expect_err("the call should have been refused")
    }

    #[test]
    fn a_read_only_server_reads_but_never_writes() {
        let (v, disk) = vault_with(VaultOptions {
            read_only: true,
            ..Default::default()
        });
        let note = Note::new("Existing", "text", disk.root_dir());
        disk.save_note(&note).unwrap();
        assert_eq!(call(&v, "vault_info", json!({}))["read_only_server"], true);
        assert_eq!(
            call(&v, "read_note", json!({ "note": "Existing" }))["body"],
            "text"
        );
        for tool in WRITE_TOOLS {
            let why = fails(
                &v,
                tool,
                json!({ "note": "Existing", "title": "x", "confirm": true }),
            );
            assert!(why.contains("read-only"), "{tool}: {why}");
        }
        assert_eq!(
            fs::read_to_string(&note.file_path).unwrap(),
            note.to_markdown()
        );
    }

    #[test]
    fn a_folder_limited_server_only_sees_and_writes_that_folder() {
        let (v, disk) = vault_with(VaultOptions {
            folder: Some("AI".into()),
            ..Default::default()
        });
        let mut inside = Note::new("Inside", "in", disk.root_dir());
        inside.folder = Some("AI/Sub".into());
        let outside = Note::new("Outside", "out", disk.root_dir());
        disk.save_note(&inside).unwrap();
        disk.save_note(&outside).unwrap();
        assert_eq!(call(&v, "list_notes", json!({}))["count"], 1);
        assert!(fails(&v, "read_note", json!({ "note": "Outside" })).contains("no note"));
        assert_eq!(
            call(&v, "search_notes", json!({ "query": "out" }))["count"],
            0
        );
        // New notes land in the folder, and nothing can be moved out of it.
        let made = call(&v, "create_note", json!({ "title": "Fresh" }));
        assert_eq!(made["created"]["folder"], "AI");
        assert!(fails(
            &v,
            "create_note",
            json!({ "title": "Bad", "folder": "Elsewhere" })
        )
        .contains("limited to the folder"));
        assert!(fails(
            &v,
            "set_note_properties",
            json!({ "note": "Fresh", "folder": "Elsewhere" })
        )
        .contains("limited to the folder"));
        let daily = call(&v, "daily_note", json!({}));
        assert_eq!(daily["folder"], "AI");
    }

    #[test]
    fn a_note_marked_read_only_cannot_be_changed_by_an_assistant() {
        let (v, disk) = vault_with(VaultOptions::default());
        let mut guarded = Note::new("Guarded", "keep me", disk.root_dir());
        guarded.ai.read_only = true;
        disk.save_note(&guarded).unwrap();
        disk.save_note(&Note::new("Linker", "see [[Guarded]]", disk.root_dir()))
            .unwrap();
        let read = call(&v, "read_note", json!({ "note": "Guarded" }));
        assert_eq!(read["body"], "keep me");
        assert_eq!(read["read_only"], true);
        for (tool, args) in [
            ("update_note", json!({ "note": "Guarded", "append": "x" })),
            (
                "rename_note",
                json!({ "note": "Guarded", "new_title": "Other" }),
            ),
            (
                "set_note_properties",
                json!({ "note": "Guarded", "pinned": true }),
            ),
            ("trash_note", json!({ "note": "Guarded" })),
        ] {
            let why = fails(&v, tool, args);
            assert!(why.contains("marked read-only"), "{tool}: {why}");
        }
        // Not even its trashed copy can be deleted: the mark travels with the note.
        let mut trashed = guarded.clone();
        trashed.trashed = true;
        disk.save_note(&trashed).unwrap();
        assert!(fails(
            &v,
            "delete_note",
            json!({ "note": "Guarded", "confirm": true })
        )
        .contains("marked read-only"));
        assert!(guarded.file_path.exists());
        // There is no argument that lifts the mark, and unrelated notes still work.
        let ok = call(
            &v,
            "update_note",
            json!({ "note": "Linker", "append": "fine" }),
        );
        assert_eq!(ok["updated"]["read_only"], false);
        // Renaming a note links to is skipped for read-only notes that link to it.
        let mut linked = Note::new("Target", "t", disk.root_dir());
        let mut guarded_link = Note::new("Guarded link", "to [[Target]]", disk.root_dir());
        guarded_link.ai.read_only = true;
        linked.body = "t".into();
        disk.save_note(&linked).unwrap();
        disk.save_note(&guarded_link).unwrap();
        let out = call(
            &v,
            "rename_note",
            json!({ "note": "Target", "new_title": "Renamed" }),
        );
        assert_eq!(
            out["links_not_updated_in_read_only_notes"],
            json!(["Guarded link"])
        );
        assert!(fs::read_to_string(&guarded_link.file_path)
            .unwrap()
            .contains("[[Target]]"));
    }

    #[test]
    fn what_an_assistant_overwrites_or_deletes_is_kept_in_the_history() {
        let (v, disk) = vault_with(VaultOptions::default());
        let made = call(
            &v,
            "create_note",
            json!({ "title": "Draft", "body": "first" }),
        );
        let id = made["created"]["id"].as_str().unwrap().to_string();
        assert!(made["created"]["made_by_ai"].as_bool().unwrap());
        // The first write of a new note has nothing to keep.
        assert!(disk.versions(&id).is_empty());
        call(
            &v,
            "update_note",
            json!({ "note": "Draft", "body": "second" }),
        );
        call(
            &v,
            "update_note",
            json!({ "note": "Draft", "body": "third" }),
        );
        let probe = Note::new("Draft", "", disk.root_dir());
        let kept: Vec<String> = disk
            .versions(&id)
            .iter()
            .map(|version| {
                let mut of = probe.clone();
                of.id = id.clone();
                disk.read_version(version, &of).unwrap().body
            })
            .collect();
        assert_eq!(kept, vec!["second", "first"]);
        // The file itself says an assistant wrote last.
        let raw = fs::read_to_string(disk.root_dir().join(format!("{id}.md"))).unwrap();
        assert!(raw.contains("edited_by: ai") && raw.contains("author: ai"));
        // Deleting keeps a copy too.
        call(&v, "trash_note", json!({ "note": "Draft" }));
        call(
            &v,
            "delete_note",
            json!({ "note": "Draft", "confirm": true }),
        );
        assert!(!disk.root_dir().join(format!("{id}.md")).exists());
        assert!(!disk.versions(&id).is_empty());
    }

    #[test]
    fn icons_can_be_set_cleared_and_are_checked() {
        let (v, _) = vault_with(VaultOptions::default());
        call(&v, "create_note", json!({ "title": "Iconic" }));
        let set = call(
            &v,
            "set_note_properties",
            json!({ "note": "Iconic", "icon": "Flare" }),
        );
        assert_eq!(set["updated"]["icon"], "flare");
        let bad = fails(
            &v,
            "set_note_properties",
            json!({ "note": "Iconic", "icon": "no-such-icon-xyz" }),
        );
        assert!(bad.contains("unknown icon"));
        let cleared = call(
            &v,
            "set_note_properties",
            json!({ "note": "Iconic", "icon": "" }),
        );
        assert!(cleared["updated"]["icon"].is_null());
        let chosen = call(
            &v,
            "create_note",
            json!({ "title": "Own icon", "icon": "bolt" }),
        );
        assert_eq!(chosen["created"]["icon"], "bolt");
    }

    #[test]
    fn a_trashed_note_can_then_be_deleted() {
        let (v, disk) = vault_with(VaultOptions::default());
        let made = call(&v, "create_note", json!({ "title": "Doomed" }));
        let file = disk
            .root_dir()
            .join(format!("{}.md", made["created"]["id"].as_str().unwrap()));
        call(&v, "trash_note", json!({ "note": "Doomed" }));
        assert!(file.exists());
        let out = call(
            &v,
            "delete_note",
            json!({ "note": "Doomed", "confirm": true }),
        );
        assert_eq!(out["deleted"], "Doomed");
        assert!(!file.exists());
        assert_eq!(call(&v, "vault_info", json!({}))["trashed"], 0);
    }

    #[test]
    fn rename_rewrites_links_and_keeps_aliases() {
        let v = vault();
        call(&v, "create_note", json!({ "title": "Old" }));
        call(
            &v,
            "create_note",
            json!({ "title": "Ref", "body": "[[Old]] and [[old|the alias]]\n```\n[[Old]]\n```" }),
        );
        let out = call(
            &v,
            "rename_note",
            json!({ "note": "Old", "new_title": "New" }),
        );
        assert_eq!(out["links_updated_in"], json!(["Ref"]));
        let ref_note = call(&v, "read_note", json!({ "note": "Ref" }));
        assert_eq!(
            ref_note["body"],
            "[[New]] and [[New|the alias]]\n```\n[[Old]]\n```"
        );
    }

    #[test]
    fn update_modes_and_trash_flow() {
        let v = vault();
        call(&v, "create_note", json!({ "title": "N", "body": "one" }));
        call(&v, "update_note", json!({ "note": "N", "append": "two" }));
        call(&v, "update_note", json!({ "note": "N", "prepend": "zero" }));
        assert_eq!(
            call(&v, "read_note", json!({ "note": "N" }))["body"],
            "zero\n\none\n\ntwo"
        );
        assert!(v.call("update_note", &json!({ "note": "N" })).is_err());
        assert!(v
            .call("delete_note", &json!({ "note": "N", "confirm": true }))
            .is_err());
        call(&v, "trash_note", json!({ "note": "N" }));
        assert!(v.call("read_note", &json!({ "note": "N" })).is_err());
        assert!(v.call("delete_note", &json!({ "note": "N" })).is_err());
        call(&v, "delete_note", json!({ "note": "N", "confirm": true }));
        assert_eq!(call(&v, "vault_info", json!({}))["trashed"], 0);
    }

    #[test]
    fn daily_note_is_made_once_and_appended_to() {
        let v = vault();
        assert_eq!(call(&v, "daily_note", json!({}))["created"], true);
        let again = call(&v, "daily_note", json!({ "append": "- called Sam" }));
        assert_eq!(again["created"], false);
        assert!(again["body"].as_str().unwrap().ends_with("- called Sam"));
        assert_eq!(call(&v, "vault_info", json!({}))["notes"], 1);
    }

    #[test]
    fn search_ranks_and_unresolved_links_are_found() {
        let v = vault();
        call(
            &v,
            "create_note",
            json!({ "title": "Cooking", "body": "pasta pasta [[Missing]]" }),
        );
        call(
            &v,
            "create_note",
            json!({ "title": "Other", "body": "one pasta" }),
        );
        let found = call(&v, "search_notes", json!({ "query": "pasta" }));
        assert_eq!(found["results"][0]["title"], "Cooking");
        assert_eq!(call(&v, "find_unresolved_links", json!({}))["count"], 1);
    }

    #[test]
    fn protocol_handshake_and_errors() {
        let v = vault();
        let init = handle(
            &v,
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }),
        )
        .unwrap();
        assert_eq!(init["result"]["serverInfo"]["name"], "elora-notes");
        assert!(handle(
            &v,
            &json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })
        )
        .is_none());
        let bad = handle(&v, &json!({ "jsonrpc": "2.0", "id": 2, "method": "nope" })).unwrap();
        assert_eq!(bad["error"]["code"], -32601);
        let failed = handle(
            &v,
            &json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "read_note", "arguments": { "note": "ghost" } } }),
        )
        .unwrap();
        assert_eq!(failed["result"]["isError"], true);
    }
}
