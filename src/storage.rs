use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum NoteTemplate {
    Blank,
    DailyJournal,
    MeetingNotes,
    ProjectSpec,
}

impl NoteTemplate {
    pub fn build(self, dir: &Path) -> Note {
        let today = Local::now();
        match self {
            Self::Blank => Note::new("Untitled", "", dir),
            Self::DailyJournal => {
                let title = format!("Daily Note — {}", today.format("%Y-%m-%d"));
                let body = format!(
                    "## {} · Morning Intention\n\
                     - [ ] Primary focus for today\n\
                     - [ ] Secondary task\n\n\
                     ## Notes & Captures\n\
                     - \n\n\
                     ## End of Day Reflection\n\
                     > What went well today?\n\n\
                     #daily #journal",
                    today.format("%A, %b %d")
                );
                let mut note = Note::new(title, body, dir);
                note.explicit_tags = vec!["daily".to_string(), "journal".to_string()];
                note
            }
            Self::MeetingNotes => {
                let title = format!("Meeting Notes — {}", today.format("%b %d"));
                let body = "## Attendees\n\
                            - \n\n\
                            ## Agenda & Discussion\n\
                            - \n\n\
                            ## Action Items\n\
                            - [ ] Follow up on key decisions\n\n\
                            #meeting #notes"
                    .to_string();
                let mut note = Note::new(title, body, dir);
                note.explicit_tags = vec!["meeting".to_string()];
                note
            }
            Self::ProjectSpec => {
                let title = "Project Spec".to_string();
                let body = "## Overview\n\
                            Brief summary of the idea and why it matters.\n\n\
                            ## Goals\n\
                            - [ ] Milestone 1\n\
                            - [ ] Milestone 2\n\n\
                            ## Architecture & Links\n\
                            Related to [[Welcome to EloraNotes]].\n\n\
                            #project #spec"
                    .to_string();
                let mut note = Note::new(title, body, dir);
                note.explicit_tags = vec!["project".to_string(), "spec".to_string()];
                note
            }
        }
    }
}

/// What the notes inside each sidebar group are ordered by. The default is the order a notes
/// app shows: what was written last sits at the top.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NoteSort {
    #[default]
    Updated,
    Created,
    Title,
}

impl NoteSort {
    pub fn name(self) -> &'static str {
        match self {
            Self::Updated => "Last edited",
            Self::Created => "Created",
            Self::Title => "Title",
        }
    }
}

/// The window as it was left: its size, where it sat, and whether it was maximised. A maximised
/// window keeps the size and position it had before, so restoring it does not open it huge.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowState {
    pub width: f32,
    pub height: f32,
    pub x: f32,
    pub y: f32,
    #[serde(default)]
    pub maximized: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub theme_name: String,
    pub show_formatting_bar: bool,
    #[serde(default = "default_highlight_rgb")]
    pub highlight_rgb: [u8; 3],
    #[serde(default = "default_text_color_rgb")]
    pub text_color_rgb: [u8; 3],
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    #[serde(default = "default_glow_strength")]
    pub glow_strength: f32,
    #[cfg(feature = "gui")]
    #[serde(default)]
    pub colors: crate::theme::ColorOverrides,
    /// Folders the user folded shut in the sidebar.
    #[serde(default)]
    pub collapsed_folders: Vec<String>,
    /// The shared library this client connects to, if any.
    #[serde(default)]
    pub server: crate::collab::ServerProfile,
    /// The name everybody else sees: next to this client's caret, and the word somebody types
    /// to share a note with it. Public, and kept unique by the server.
    #[serde(default)]
    pub display_name: String,
    /// This client's key: a secret only the server ever reads, which proves the connection is
    /// the person the name belongs to and decides which restricted notes it is shown. A fresh
    /// install makes one; it is the same string on another computer to be the same person there.
    ///
    /// A settings file from before names and keys were separate had no key, and the old
    /// `display_name` was doing both jobs; an empty key is replaced with a generated one at
    /// startup.
    #[serde(default)]
    pub identifier: String,
    /// The colour this person's name and caret are drawn in on everybody else's screen. `None`
    /// lets the name pick one, which is what a fresh install and somebody who never opened the
    /// colour picker have. It only changes what other people see in a shared note.
    #[serde(default)]
    pub peer_color: Option<[u8; 3]>,

    /// Whether a note in the sidebar is drawn as the tall card — icon, title, date and tags — or
    /// as the one-line row a file list uses. Off by default: the compact row fits twice as many
    /// notes in the same list.
    #[serde(default)]
    pub roomy_cards: bool,

    /// What the notes inside each sidebar group are ordered by.
    #[serde(default)]
    pub sort: NoteSort,

    /// Where the window was last left, so the next launch opens in the same place.
    #[serde(default)]
    pub window: Option<WindowState>,

    /// The folder the last HTML export was written into, so the next one starts there. `None` means
    /// the downloads folder, where an export goes until somebody picks somewhere else.
    #[serde(default)]
    pub export_dir: Option<String>,

    /// Whether a card in the sidebar shows the first lines of its note. Off unless it is turned
    /// on, which leaves just the title, the date and the tags, so more notes fit in the list.
    ///
    /// The key is `note_previews` because it started out as `show_card_previews` with the line
    /// showing by default: a settings file written then still says `true`, and reading the old
    /// key would keep the old default for exactly the people who never asked for it. The new key
    /// is not in those files, so they open with the line off like a fresh install does.
    #[serde(rename = "note_previews", default = "default_show_card_previews")]
    pub show_card_previews: bool,
}

fn default_highlight_rgb() -> [u8; 3] {
    [255, 196, 77]
}

fn default_text_color_rgb() -> [u8; 3] {
    [78, 205, 255]
}

fn default_font_size() -> f32 {
    15.0
}

fn default_glow_strength() -> f32 {
    0.6
}

/// Cards start without the preview line: the sidebar is for finding a note, and titles do that
/// in less room. Settings turns the line back on for anyone who wants to skim the first words.
fn default_show_card_previews() -> bool {
    false
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme_name: "Obsidian".to_string(),
            show_formatting_bar: true,
            highlight_rgb: default_highlight_rgb(),
            text_color_rgb: default_text_color_rgb(),
            font_size: default_font_size(),
            glow_strength: default_glow_strength(),
            #[cfg(feature = "gui")]
            colors: Default::default(),
            collapsed_folders: Vec::new(),
            server: Default::default(),
            display_name: String::new(),
            identifier: String::new(),
            peer_color: None,
            roomy_cards: false,
            sort: NoteSort::default(),
            window: None,
            export_dir: None,
            show_card_previews: default_show_card_previews(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Note {
    pub id: String,
    pub title: String,
    pub body: String,
    pub pinned: bool,
    pub trashed: bool,
    /// Explicitly assigned tags (stored in frontmatter)
    pub explicit_tags: Vec<String>,
    /// Optional Tabler icon name (e.g. `flare`), stored in frontmatter
    pub icon: Option<String>,
    /// Optional folder the note is filed under, stored in frontmatter.
    pub folder: Option<String>,
    /// The name of the person the note belongs to: what the sidebar prints. It is only a label —
    /// the key below is what decides anything — so changing it moves no access.
    pub owner: Option<String>,
    /// The owner's public key: the digest of their identifier. It is what may change who sees
    /// the note, and it is safe to sit in a file anybody can read, because only the identifier
    /// behind
    /// it can present it at the server.
    pub owner_key: Option<String>,
    /// True when only the owner and the people in `shared_with` may see the note. Stored
    /// in the shared library's frontmatter, where the server reads it.
    pub private: bool,
    /// Display names allowed to see a private note.
    pub shared_with: Vec<String>,
    /// The public keys behind those names, which is what actually lets their holders in.
    pub shared_keys: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub file_path: PathBuf,
}

impl Note {
    pub fn new(title: impl Into<String>, body: impl Into<String>, dir: &Path) -> Self {
        let now = Utc::now();
        let title_str = title.into();
        let id = generate_unique_id(&title_str, dir, now);
        let file_path = dir.join(format!("{id}.md"));

        Self {
            id,
            title: title_str,
            body: body.into(),
            pinned: false,
            trashed: false,
            explicit_tags: Vec::new(),
            icon: None,
            folder: None,
            owner: None,
            owner_key: None,
            private: false,
            shared_with: Vec::new(),
            shared_keys: Vec::new(),
            created_at: now,
            updated_at: now,
            file_path,
        }
    }

    /// Returns a sorted, deduplicated list of all tags (both frontmatter tags and `#inline` tags).
    pub fn all_tags(&self) -> Vec<String> {
        let mut set = BTreeSet::new();
        for t in &self.explicit_tags {
            let cleaned = normalize_tag(t);
            if !cleaned.is_empty() {
                set.insert(cleaned);
            }
        }
        for t in extract_inline_tags(&self.body) {
            set.insert(t);
        }
        set.into_iter().collect()
    }

    /// Extracts all outbound `[[Wiki Links]]` from this note's body.
    pub fn outbound_links(&self) -> Vec<String> {
        extract_wiki_links(&self.body)
    }

    /// Adds an explicit tag if not already present.
    pub fn add_tag(&mut self, tag: &str) -> bool {
        let cleaned = normalize_tag(tag);
        if cleaned.is_empty() {
            return false;
        }
        if !self.all_tags().contains(&cleaned) {
            self.explicit_tags.push(cleaned);
            self.updated_at = Utc::now();
            true
        } else {
            false
        }
    }

    /// Removes an explicit tag. Returns true if removed.
    pub fn remove_explicit_tag(&mut self, tag: &str) -> bool {
        let cleaned = normalize_tag(tag);
        let before = self.explicit_tags.len();
        self.explicit_tags.retain(|t| normalize_tag(t) != cleaned);
        let changed = self.explicit_tags.len() != before;
        if changed {
            self.updated_at = Utc::now();
        }
        changed
    }

    /// Effective display title (falls back to first non-empty line or "Untitled").
    pub fn display_title(&self) -> String {
        let trimmed = self.title.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
        for line in self.body.lines() {
            let cleaned = line.trim().trim_start_matches('#').trim();
            if !cleaned.is_empty() {
                return cleaned.chars().take(48).collect();
            }
        }
        "Untitled note".to_string()
    }

    /// Short one-line preview snippet of the note body.
    pub fn snippet(&self) -> String {
        for line in self.body.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let cleaned = trimmed
                .trim_start_matches('#')
                .trim_start_matches("- [x] ")
                .trim_start_matches("- [X] ")
                .trim_start_matches("- [ ] ")
                .trim_start_matches("- ")
                .trim_start_matches("* ")
                .trim_start_matches("> ")
                .replace("**", "")
                .replace("~~", "")
                .replace("__", "")
                .replace("==", "");
            let cleaned = cleaned.trim();
            if !cleaned.is_empty() {
                let mut s: String = cleaned.chars().take(68).collect();
                if cleaned.chars().count() > 68 {
                    s.push('…');
                }
                return s;
            }
        }
        "No additional text".to_string()
    }

    pub fn word_count(&self) -> usize {
        self.body.split_whitespace().count()
    }

    pub fn char_count(&self) -> usize {
        self.body.chars().count()
    }

    /// Estimated reading time in minutes (minimum 1 min if non-empty).
    pub fn reading_time_minutes(&self) -> usize {
        let words = self.word_count();
        if words == 0 {
            0
        } else {
            (words.div_ceil(200)).max(1)
        }
    }

    /// Returns `(completed_tasks, total_tasks)` for Markdown `- [ ]` and `- [x]` items.
    pub fn task_progress(&self) -> (usize, usize) {
        let mut done = 0;
        let mut total = 0;
        let mut in_code = false;
        for line in self.body.lines() {
            let t = line.trim_start();
            if t.starts_with("```") {
                in_code = !in_code;
                continue;
            }
            if in_code {
                continue;
            }
            if t.starts_with("- [ ] ") || t.starts_with("* [ ] ") {
                total += 1;
            } else if t.starts_with("- [x] ")
                || t.starts_with("- [X] ")
                || t.starts_with("* [x] ")
                || t.starts_with("* [X] ")
            {
                done += 1;
                total += 1;
            }
        }
        (done, total)
    }

    /// Toggles the `nth` task item (`0`-indexed) in `self.body`.
    pub fn toggle_task_at(&mut self, target_task_idx: usize) -> bool {
        let mut current_idx = 0;
        let mut in_code = false;
        let mut modified = false;
        let mut new_lines: Vec<String> = Vec::new();

        for line in self.body.lines() {
            let trimmed = line.trim_start();
            let indent_len = line.len() - trimmed.len();
            let indent = &line[..indent_len];

            if trimmed.starts_with("```") {
                in_code = !in_code;
                new_lines.push(line.to_string());
                continue;
            }
            if !in_code {
                if let Some(rest) = trimmed.strip_prefix("- [ ] ") {
                    if current_idx == target_task_idx {
                        new_lines.push(format!("{indent}- [x] {rest}"));
                        modified = true;
                    } else {
                        new_lines.push(line.to_string());
                    }
                    current_idx += 1;
                    continue;
                } else if let Some(rest) = trimmed
                    .strip_prefix("- [x] ")
                    .or_else(|| trimmed.strip_prefix("- [X] "))
                {
                    if current_idx == target_task_idx {
                        new_lines.push(format!("{indent}- [ ] {rest}"));
                        modified = true;
                    } else {
                        new_lines.push(line.to_string());
                    }
                    current_idx += 1;
                    continue;
                }
            }
            new_lines.push(line.to_string());
        }

        if modified {
            self.body = new_lines.join("\n");
            self.updated_at = Utc::now();
        }
        modified
    }

    pub fn formatted_updated(&self) -> String {
        let local: DateTime<Local> = DateTime::from(self.updated_at);
        let now = Local::now();
        if local.date_naive() == now.date_naive() {
            // A 12-hour clock, the way a note reads on paper: 3:04 PM, not 15:04. The hour comes
            // out of `%I` zero-padded, and a leading zero on a clock is not how anybody says it.
            let clock = local.format("%I:%M %p").to_string();
            let clock = clock.strip_prefix('0').unwrap_or(&clock);
            format!("Today, {clock}")
        } else {
            local.format("%b %d, %Y").to_string()
        }
    }

    /// Serializes the note into Markdown with YAML frontmatter.
    pub fn to_markdown(&self) -> String {
        let escaped_title = escape_yaml_string(&self.title);
        let folder_line = self
            .folder
            .as_ref()
            .map(|f| format!("folder: \"{}\"\n", escape_yaml_string(f)))
            .unwrap_or_default();
        let tags_serialized = if self.explicit_tags.is_empty() {
            "[]".to_string()
        } else {
            let items: Vec<String> = self
                .explicit_tags
                .iter()
                .map(|t| normalize_tag(t))
                .filter(|t| !t.is_empty())
                .collect();
            format!("[{}]", items.join(", "))
        };

        // Who the note belongs to and who may see it. Only the shared library sets these, so a
        // note kept on this computer keeps its plain frontmatter.
        let owner_line = self
            .owner
            .as_ref()
            .map(|owner| format!("owner: \"{}\"\n", escape_yaml_string(owner)))
            .unwrap_or_default();
        let owner_key_line = self
            .owner_key
            .as_ref()
            .map(|key| format!("owner_key: \"{}\"\n", escape_yaml_string(key)))
            .unwrap_or_default();
        let private_line = if self.private { "private: true\n" } else { "" };
        let shared_line = if self.shared_with.is_empty() {
            String::new()
        } else {
            let names: Vec<String> = self
                .shared_with
                .iter()
                .map(|name| format!("\"{}\"", escape_yaml_string(name)))
                .collect();
            format!("shared_with: [{}]\n", names.join(", "))
        };
        let shared_keys_line = if self.shared_keys.is_empty() {
            String::new()
        } else {
            let keys: Vec<String> = self
                .shared_keys
                .iter()
                .map(|key| format!("\"{}\"", escape_yaml_string(key)))
                .collect();
            format!("shared_keys: [{}]\n", keys.join(", "))
        };

        format!(
            "---\ntitle: \"{}\"\npinned: {}\ntrashed: {}\ntags: {}\n{}{}{}{}{}{}{}created_at: {}\nupdated_at: {}\n---\n{}",
            escaped_title,
            self.pinned,
            self.trashed,
            tags_serialized,
            folder_line,
            self.icon
                .as_ref()
                .map(|i| format!("icon: {i}\n"))
                .unwrap_or_default(),
            owner_line,
            owner_key_line,
            private_line,
            shared_line,
            shared_keys_line,
            self.created_at.to_rfc3339(),
            self.updated_at.to_rfc3339(),
            self.body
        )
    }

    /// Parses a Markdown file (with or without YAML frontmatter) into a `Note`.
    pub fn from_markdown(id: String, raw: &str, file_path: PathBuf) -> Self {
        let fallback_time = fs::metadata(&file_path)
            .and_then(|m| m.modified())
            .map(DateTime::<Utc>::from)
            .unwrap_or_else(|_| Utc::now());

        if let Some((frontmatter, body)) = split_frontmatter(raw) {
            let mut title = String::new();
            let mut pinned = false;
            let mut trashed = false;
            let mut explicit_tags = Vec::new();
            let mut icon = None;
            let mut folder = None;
            let mut owner = None;
            let mut owner_key = None;
            let mut private = false;
            let mut shared_with = Vec::new();
            let mut shared_keys = Vec::new();
            let mut created_at = fallback_time;
            let mut updated_at = fallback_time;

            for line in frontmatter.lines() {
                let line = line.trim();
                if let Some((key, val)) = line.split_once(':') {
                    let key = key.trim();
                    let val = val.trim();
                    match key {
                        "title" => {
                            title = unquote_yaml_string(val);
                        }
                        "pinned" => {
                            pinned = val.eq_ignore_ascii_case("true");
                        }
                        "trashed" => {
                            trashed = val.eq_ignore_ascii_case("true");
                        }
                        "tags" => {
                            explicit_tags = parse_yaml_tags(val);
                        }
                        "folder" => {
                            folder = normalize_folder(&unquote_yaml_string(val));
                        }
                        "owner" => {
                            let name = crate::net::normalize_identifier(&unquote_yaml_string(val));
                            owner = (!name.is_empty()).then_some(name);
                        }
                        "owner_key" => {
                            let key = unquote_yaml_string(val);
                            owner_key = (!key.trim().is_empty()).then(|| key.trim().to_string());
                        }
                        "private" => {
                            private = val.eq_ignore_ascii_case("true");
                        }
                        "shared_with" => {
                            shared_with = parse_yaml_list(val)
                                .iter()
                                .map(|name| crate::net::normalize_identifier(name))
                                .filter(|name| !name.is_empty())
                                .collect();
                        }
                        "shared_keys" => {
                            shared_keys = parse_yaml_list(val)
                                .iter()
                                .map(|key| key.trim().to_string())
                                .filter(|key| !key.is_empty())
                                .collect();
                        }
                        "icon" => {
                            let name: String = val
                                .trim_matches('"')
                                .chars()
                                .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
                                .collect();
                            icon = (!name.is_empty()).then_some(name);
                        }
                        "created_at" => {
                            if let Ok(dt) = DateTime::parse_from_rfc3339(val.trim_matches('"')) {
                                created_at = dt.with_timezone(&Utc);
                            }
                        }
                        "updated_at" => {
                            if let Ok(dt) = DateTime::parse_from_rfc3339(val.trim_matches('"')) {
                                updated_at = dt.with_timezone(&Utc);
                            }
                        }
                        _ => {}
                    }
                }
            }

            Self {
                id,
                title,
                body: body.to_string(),
                pinned,
                trashed,
                explicit_tags,
                icon,
                folder,
                owner,
                owner_key,
                private,
                shared_with,
                shared_keys,
                created_at,
                updated_at,
                file_path,
            }
        } else {
            let mut lines = raw.lines();
            let mut title = String::new();
            let mut body_start = raw;

            if let Some(first) = lines.next() {
                if let Some(stripped) = first.strip_prefix("# ") {
                    title = stripped.trim().to_string();
                    let prefix_len = first.len();
                    body_start = raw
                        .get(prefix_len..)
                        .unwrap_or("")
                        .trim_start_matches(['\r', '\n']);
                }
            }

            if title.is_empty() {
                title = id.replace('-', " ");
            }

            Self {
                id,
                title,
                body: body_start.to_string(),
                pinned: false,
                trashed: false,
                explicit_tags: Vec::new(),
                icon: None,
                folder: None,
                owner: None,
                owner_key: None,
                private: false,
                shared_with: Vec::new(),
                shared_keys: Vec::new(),
                created_at: fallback_time,
                updated_at: fallback_time,
                file_path,
            }
        }
    }

    /// Matches note against a search query (supports free text and `#tag` tokens) and an optional active tag filter.
    pub fn matches_filter(&self, query: &str, active_tag: Option<&str>) -> bool {
        let tags = self.all_tags();

        if let Some(required_tag) = active_tag {
            let norm_req = normalize_tag(required_tag);
            if !tags.iter().any(|t| t == &norm_req) {
                return false;
            }
        }

        let query = query.trim();
        if query.is_empty() {
            return true;
        }

        let title_lower = self.title.to_lowercase();
        let body_lower = self.body.to_lowercase();

        for token in query.split_whitespace() {
            if let Some(tag_query) = token.strip_prefix('#') {
                let tag_query = normalize_tag(tag_query);
                if !tag_query.is_empty() && !tags.iter().any(|t| t.contains(&tag_query)) {
                    return false;
                }
            } else {
                let token_lower = token.to_lowercase();
                let in_title = title_lower.contains(&token_lower)
                    || fuzzy_subsequence(&title_lower, &token_lower);
                let in_body = body_lower.contains(&token_lower);
                let in_tags = tags.iter().any(|t| t.contains(&token_lower));

                if !(in_title || in_body || in_tags) {
                    return false;
                }
            }
        }

        true
    }

    /// Ranks how well a note matches a free-text query: title hits weigh most, then body occurrences.
    pub fn relevance_score(&self, query: &str) -> usize {
        let title_lower = self.title.to_lowercase();
        let body_lower = self.body.to_lowercase();
        let mut score = 0;
        for token in query.split_whitespace() {
            if token.starts_with('#') {
                continue;
            }
            let token_lower = token.to_lowercase();
            if title_lower.contains(&token_lower) {
                score += 10;
            }
            score += body_lower.matches(&token_lower).count().min(20);
        }
        score
    }

    /// Returns a short, single-line excerpt of the body around the first free-text query match.
    pub fn body_snippet(&self, query: &str) -> Option<String> {
        let needle = query.split_whitespace().find(|t| !t.starts_with('#'))?;
        let needle_lower = needle.to_lowercase();
        // Lowercasing can change byte offsets for some Unicode; only trust the lowered index when lengths match.
        let body_lower = self.body.to_lowercase();
        let idx = if body_lower.len() == self.body.len() {
            body_lower.find(&needle_lower)?
        } else {
            self.body.find(needle)?
        };

        let mut start = idx.saturating_sub(40);
        while start > 0 && !self.body.is_char_boundary(start) {
            start -= 1;
        }
        let mut end = (idx + needle.len() + 60).min(self.body.len());
        while end < self.body.len() && !self.body.is_char_boundary(end) {
            end += 1;
        }

        let flat = self.body[start..end]
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let prefix = if start > 0 { "\u{2026}" } else { "" };
        let suffix = if end < self.body.len() {
            "\u{2026}"
        } else {
            ""
        };
        Some(format!("{prefix}{flat}{suffix}"))
    }
}

/// Checks if `needle` characters appear in order inside `haystack` (lightweight fuzzy match for titles).
pub fn fuzzy_subsequence(haystack: &str, needle: &str) -> bool {
    if needle.len() < 2 {
        return haystack.contains(needle);
    }
    let mut hay_chars = haystack.chars();
    for nc in needle.chars() {
        if !hay_chars.any(|hc| hc == nc) {
            return false;
        }
    }
    true
}

/// Normalizes a tag string: strips leading `#`, lowercases, and keeps alphanumeric, `-`, `_`, `/`.
pub fn normalize_tag(raw: &str) -> String {
    raw.trim()
        .trim_start_matches('#')
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_' || *c == '/')
        .collect::<String>()
        .to_lowercase()
}

/// Extracts `[[Wiki Link]]` targets from Markdown text (ignoring fenced code blocks).
pub fn extract_wiki_links(body: &str) -> Vec<String> {
    let mut links = BTreeSet::new();
    let mut in_code_block = false;

    for line in body.lines() {
        if line.trim_start().starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block {
            continue;
        }

        let mut rest = line;
        while let Some(start) = rest.find("[[") {
            let after_open = &rest[start + 2..];
            if let Some(end) = after_open.find("]]") {
                let raw_target = after_open[..end].trim();
                // Support [[Note Title|Alias]] syntax by taking the part before `|`
                let target = raw_target.split('|').next().unwrap_or(raw_target).trim();
                if !target.is_empty() && !target.contains('\n') {
                    links.insert(target.to_string());
                }
                rest = &after_open[end + 2..];
            } else {
                break;
            }
        }
    }

    links.into_iter().collect()
}

/// Extracts `#tags` from Markdown body text, ignoring Markdown headings (`# Heading`) and hex colors.
pub fn extract_inline_tags(body: &str) -> Vec<String> {
    let mut tags = BTreeSet::new();
    let mut in_code_block = false;

    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block {
            continue;
        }

        for (idx, word) in line.split_whitespace().enumerate() {
            if idx == 0 && word.chars().all(|c| c == '#') {
                continue;
            }
            let word = word.trim_matches(|c: char| {
                matches!(
                    c,
                    ',' | '.' | ';' | ':' | '(' | ')' | '[' | ']' | '"' | '\''
                )
            });
            if let Some(rest) = word.strip_prefix('#') {
                if rest.is_empty() || rest.starts_with('#') {
                    continue;
                }
                if let Some(first) = rest.chars().next() {
                    if !first.is_alphabetic() {
                        continue;
                    }
                }
                let cleaned = normalize_tag(rest);
                if !cleaned.is_empty() {
                    tags.insert(cleaned);
                }
            }
        }
    }

    tags.into_iter().collect()
}

fn split_frontmatter(raw: &str) -> Option<(&str, &str)> {
    let raw = raw
        .strip_prefix("---\n")
        .or_else(|| raw.strip_prefix("---\r\n"))?;
    if let Some(idx) = raw.find("\n---\n") {
        let fm = &raw[..idx];
        let body = &raw[idx + 5..];
        Some((fm, body))
    } else if let Some(idx) = raw.find("\r\n---\r\n") {
        let fm = &raw[..idx];
        let body = &raw[idx + 7..];
        Some((fm, body))
    } else {
        None
    }
}

fn unquote_yaml_string(val: &str) -> String {
    let val = val.trim();
    if val.len() >= 2 && val.starts_with('"') && val.ends_with('"') {
        val[1..val.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
    } else if val.len() >= 2 && val.starts_with('\'') && val.ends_with('\'') {
        val[1..val.len() - 1].to_string()
    } else {
        val.to_string()
    }
}

/// Reads a `[a, b]` list of plain strings, keeping the case and wording of each entry. Tags
/// have their own normalisation; identifiers must stay exactly as they were typed.
fn parse_yaml_list(val: &str) -> Vec<String> {
    let inner = val.trim().trim_start_matches('[').trim_end_matches(']');
    if inner.trim().is_empty() {
        return Vec::new();
    }
    inner
        .split(',')
        .map(|s| unquote_yaml_string(s.trim()))
        .filter(|s| !s.trim().is_empty())
        .collect()
}

fn parse_yaml_tags(val: &str) -> Vec<String> {
    let inner = val.trim().trim_start_matches('[').trim_end_matches(']');
    if inner.trim().is_empty() {
        return Vec::new();
    }
    inner
        .split(',')
        .map(|s| normalize_tag(s.trim_matches('"').trim_matches('\'')))
        .filter(|s| !s.is_empty())
        .collect()
}

fn slugify(title: &str) -> String {
    let mut slug = String::new();
    let mut prev_dash = false;
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !slug.is_empty() {
            slug.push('-');
            prev_dash = true;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "note".to_string()
    } else {
        slug.chars()
            .take(36)
            .collect::<String>()
            .trim_end_matches('-')
            .to_string()
    }
}

fn generate_unique_id(title: &str, dir: &Path, now: DateTime<Utc>) -> String {
    let base = slugify(title);
    let timestamp = now.format("%Y%m%d-%H%M%S");
    let candidate = format!("{base}-{timestamp}");
    if !dir.join(format!("{candidate}.md")).exists() {
        return candidate;
    }
    for i in 1..1000 {
        let alt = format!("{candidate}-{i}");
        if !dir.join(format!("{alt}.md")).exists() {
            return alt;
        }
    }
    format!("note-{}", now.timestamp_millis())
}

/// Copies Markdown notes and settings from earlier default folders into `target`.
/// Existing files in `target` are never overwritten and the old folders are left intact.
/// Returns the number of files copied.
pub fn migrate_legacy_notes(target: &Path) -> usize {
    let Some(home) = dirs::home_dir() else {
        return 0;
    };
    // `~/.config/eloranotes` is where every release up to 0.7.0 kept notes, so it comes first.
    let legacy_dirs = [
        home.join(".config").join("eloranotes"),
        home.join("eloranotes"),
        home.join(".EloraNotes"),
    ];
    migrate_from_dirs(target, &legacy_dirs)
}

fn migrate_from_dirs(target: &Path, legacy_dirs: &[PathBuf]) -> usize {
    let mut copied = 0;
    for legacy in legacy_dirs {
        if !legacy.is_dir() || legacy == target {
            continue;
        }
        let Ok(entries) = fs::read_dir(legacy) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let is_note = path.extension().is_some_and(|ext| ext == "md");
            let is_settings = path
                .file_name()
                .is_some_and(|name| name == ".elora-settings.json");
            if !(is_note || is_settings) || !path.is_file() {
                continue;
            }
            let dest = target.join(entry.file_name());
            if dest.exists() {
                continue;
            }
            if fs::copy(&path, &dest).is_ok() {
                copied += 1;
            }
        }
    }
    copied
}

/// `Documents/EloraNotes` under the user's home. Documents is used because every install
/// format can reach it — a Flatpak exposes the real folder, not a sandboxed copy — so notes
/// and settings are the same ones whichever build of the app you open. The settings file
/// (`.elora-settings.json`) lives in this folder too.
fn default_notes_dir_for(documents: Option<PathBuf>, home: Option<PathBuf>) -> PathBuf {
    match (documents, home) {
        (Some(documents), _) => documents.join("EloraNotes"),
        (None, Some(home)) => home.join("Documents").join("EloraNotes"),
        (None, None) => PathBuf::from("EloraNotes"),
    }
}

pub struct NoteStorage {
    root_dir: PathBuf,
}

impl NoteStorage {
    pub fn new(root_dir: PathBuf) -> io::Result<Self> {
        fs::create_dir_all(&root_dir)?;
        Ok(Self { root_dir })
    }

    pub fn default_notes_dir() -> PathBuf {
        if let Ok(custom) = std::env::var("ELORA_NOTES_DIR") {
            let p = PathBuf::from(custom);
            if !p.as_os_str().is_empty() {
                return p;
            }
        }
        default_notes_dir_for(dirs::document_dir(), dirs::home_dir())
    }

    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    fn settings_path(&self) -> PathBuf {
        self.root_dir.join(".elora-settings.json")
    }

    pub fn load_settings(&self) -> AppSettings {
        fs::read_to_string(self.settings_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save_settings(&self, settings: &AppSettings) {
        if let Ok(json) = serde_json::to_string_pretty(settings) {
            let _ = fs::write(self.settings_path(), json);
        }
    }

    pub fn load_all(&self) -> io::Result<Vec<Note>> {
        let mut notes = Vec::new();
        if !self.root_dir.exists() {
            fs::create_dir_all(&self.root_dir)?;
        }

        for entry in fs::read_dir(&self.root_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("md") {
                let id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("note")
                    .to_string();
                if let Ok(raw) = fs::read_to_string(&path) {
                    let note = Note::from_markdown(id, &raw, path);
                    notes.push(note);
                }
            }
        }

        if notes.is_empty() {
            notes = self.seed_starter_notes()?;
        }

        sort_notes(&mut notes);
        Ok(notes)
    }

    pub fn save_note(&self, note: &Note) -> io::Result<()> {
        fs::create_dir_all(&self.root_dir)?;
        let content = note.to_markdown();
        fs::write(&note.file_path, content)
    }

    /// Copies every `.md` file directly inside `src` into the notes folder as a new note, keeping
    /// its frontmatter and body. Files get fresh IDs so existing notes are never overwritten.
    pub fn import_markdown_dir(&self, src: &Path) -> io::Result<usize> {
        let mut imported = 0;
        for entry in fs::read_dir(src)? {
            let path = entry?.path();
            if !(path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("md")) {
                continue;
            }
            let raw = fs::read_to_string(&path)?;
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Imported note")
                .to_string();
            let mut note = Note::from_markdown(stem.clone(), &raw, path.clone());
            let fresh = Note::new(stem, "", &self.root_dir);
            note.id = fresh.id;
            note.file_path = fresh.file_path;
            self.save_note(&note)?;
            imported += 1;
        }
        Ok(imported)
    }

    pub fn delete_note(&self, note: &Note) -> io::Result<()> {
        if note.file_path.exists() {
            fs::remove_file(&note.file_path)?;
        }
        Ok(())
    }

    fn seed_starter_notes(&self) -> io::Result<Vec<Note>> {
        let mut welcome = Note::new(
            "Welcome to EloraNotes",
            "Elora is a calm, minimalistic workspace for your thoughts — now enhanced with **Tabler Icons**, **Wiki-links**, and a **Command Palette**.\n\n\
             ## What's New\n\
             - **Command Palette (`Ctrl+K`)**: Jump to any note, create from templates, switch themes, or run actions without leaving the keyboard.\n\
             - **Bi-Directional `[[Wiki Links]]`**: Reference other notes like [[Reading & Project Ideas]] or [[Daily Scratchpad]]. Click any link in Preview/Split mode to jump right to it, or create it on the fly.\n\
             - **Editor Views**: Switch between **Edit**, **Split View** (`Ctrl+Shift+E`), and **Preview** (`Ctrl+E`).\n\
             - **Interactive Checklists**: Click tasks directly in Preview or Split mode to check them off.\n\
             - **Daily Note (`Ctrl+D`)**: Open or create today's dated journal note in one keystroke.\n\n\
             ## Quick Checklist\n\
             - [x] Store notes as clean `.md` files on disk\n\
             - [x] Integrate crisp Tabler Icons across the interface\n\
             - [ ] Press `Ctrl+K` to try the Command Palette\n\n\
             Write freely — your notes auto-save as you type. #welcome #guide",
            &self.root_dir,
        );
        welcome.pinned = true;
        welcome.explicit_tags = vec!["welcome".to_string(), "guide".to_string()];
        self.save_note(&welcome)?;

        let mut ideas = Note::new(
            "Reading & Project Ideas",
            "A lightweight capture space for books, essays, and weekend projects. Connected to [[Welcome to EloraNotes]].\n\n\
             ## Books to Read\n\
             - The Art of Doing Science and Engineering\n\
             - A Pattern Language — Christopher Alexander\n\
             - Zen and the Art of Motorcycle Maintenance\n\n\
             ## Design Philosophy\n\
             > Simplicity is about subtracting the obvious and adding the meaningful.\n\n\
             Tagged with #ideas and #reading for instant filtering.",
            &self.root_dir,
        );
        ideas.explicit_tags = vec!["ideas".to_string(), "reading".to_string()];
        self.save_note(&ideas)?;

        let mut journal = Note::new(
            "Daily Scratchpad",
            "## Today's Priorities\n\
             - [x] Design a quiet, distraction-free workspace\n\
             - [x] Add Tabler Icons and 4 minimalist color palettes\n\
             - [ ] Link notes together with [[Welcome to EloraNotes]]\n\n\
             #journal #daily",
            &self.root_dir,
        );
        journal.explicit_tags = vec!["journal".to_string()];
        self.save_note(&journal)?;

        Ok(vec![welcome, ideas, journal])
    }
}

/// Sorts notes with Trash last, Pinned first, and everything else newest-edited first.
pub fn sort_notes(notes: &mut [Note]) {
    notes.sort_by(|a, b| {
        a.trashed
            .cmp(&b.trashed)
            .then_with(|| b.pinned.cmp(&a.pinned))
            .then_with(|| b.updated_at.cmp(&a.updated_at))
            .then_with(|| a.title.cmp(&b.title))
    });
}

/// Trims a folder name and collapses inner whitespace. Returns `None` for a blank name.
pub fn normalize_folder(name: &str) -> Option<String> {
    let cleaned = name.split_whitespace().collect::<Vec<_>>().join(" ");
    (!cleaned.is_empty()).then_some(cleaned)
}

/// Escapes a value for a quoted YAML scalar.
fn escape_yaml_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_migrate_copies_missing_files_without_overwriting() {
        let base = env::temp_dir().join(format!(
            "elora_migrate_test_{}",
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let legacy = base.join("old");
        let target = base.join("new");
        fs::create_dir_all(&legacy).unwrap();
        fs::create_dir_all(&target).unwrap();
        fs::write(legacy.join("a.md"), "old a").unwrap();
        fs::write(legacy.join("b.md"), "old b").unwrap();
        fs::write(legacy.join("notes.txt"), "ignored").unwrap();
        fs::write(target.join("a.md"), "new a").unwrap();

        let copied = migrate_from_dirs(&target, &[legacy.clone(), base.join("missing")]);

        assert_eq!(copied, 1);
        assert_eq!(fs::read_to_string(target.join("a.md")).unwrap(), "new a");
        assert_eq!(fs::read_to_string(target.join("b.md")).unwrap(), "old b");
        assert!(!target.join("notes.txt").exists());
        let _ = fs::remove_dir_all(&base);
    }

    /// A note updated today says the time the way a clock does: 12-hour, and no leading zero.
    #[test]
    fn test_a_note_says_the_time_the_way_a_clock_does() {
        use chrono::TimeZone;
        let dir = std::path::Path::new("/tmp");
        let mut note = Note::new("Clock", "body", dir);
        let today = chrono::Local::now().date_naive();
        let stamp = |hour: u32, minute: u32| {
            chrono::Local
                .from_local_datetime(&today.and_hms_opt(hour, minute, 0).unwrap())
                .earliest()
                .unwrap()
                .with_timezone(&Utc)
        };

        note.updated_at = stamp(15, 4);
        assert_eq!(note.formatted_updated(), "Today, 3:04 PM");
        note.updated_at = stamp(9, 7);
        assert_eq!(note.formatted_updated(), "Today, 9:07 AM");
        note.updated_at = stamp(0, 30);
        assert_eq!(note.formatted_updated(), "Today, 12:30 AM");
        note.updated_at = stamp(12, 5);
        assert_eq!(note.formatted_updated(), "Today, 12:05 PM");

        // Anything older keeps its date instead of a time.
        note.updated_at = stamp(15, 4) - chrono::Duration::days(3);
        let older = note.formatted_updated();
        assert!(
            !older.contains(':') && older.contains(", 20"),
            "an older note shows its date: {older}"
        );
    }

    #[test]
    fn test_note_icon_round_trip() {
        let dir = std::path::Path::new("/tmp");
        let mut note = Note::new("Icon note", "body", dir);
        note.icon = Some("flare".to_string());
        let md = note.to_markdown();
        assert!(md.contains("\nicon: flare\n"));
        let parsed = Note::from_markdown(note.id.clone(), &md, note.file_path.clone());
        assert_eq!(parsed.icon.as_deref(), Some("flare"));

        note.icon = None;
        let md = note.to_markdown();
        assert!(!md.contains("icon:"));
        let parsed = Note::from_markdown(note.id.clone(), &md, note.file_path.clone());
        assert_eq!(parsed.icon, None);
    }

    /// A shared library's note remembers who it belongs to and who it was opened to, in plain
    /// frontmatter the server can read back after a restart.
    #[test]
    fn test_note_owner_and_sharing_round_trip() {
        let dir = temp_test_dir("share_roundtrip");
        let mut note = Note::new("Private plan", "body", &dir);
        note.owner = Some("Nox".to_string());
        note.private = true;
        note.shared_with = vec!["SuperSaiyan".to_string(), "Mira".to_string()];

        let md = note.to_markdown();
        assert!(md.contains("\nowner: \"Nox\"\n"), "{md}");
        assert!(md.contains("\nprivate: true\n"), "{md}");
        assert!(
            md.contains("\nshared_with: [\"SuperSaiyan\", \"Mira\"]\n"),
            "{md}"
        );
        let parsed = Note::from_markdown(note.id.clone(), &md, note.file_path.clone());
        assert_eq!(parsed.owner.as_deref(), Some("Nox"));
        assert!(parsed.private);
        assert_eq!(parsed.shared_with, vec!["SuperSaiyan", "Mira"]);

        // A note kept on this computer has no sharing lines at all, and an older note with none
        // reads as open to everyone.
        let plain = Note::new("Mine", "body", &dir);
        let md = plain.to_markdown();
        assert!(!md.contains("owner:"), "{md}");
        assert!(!md.contains("private:"), "{md}");
        assert!(!md.contains("shared_with:"), "{md}");
        let parsed = Note::from_markdown(plain.id.clone(), &md, plain.file_path.clone());
        assert_eq!(parsed.owner, None);
        assert!(!parsed.private);
        assert!(parsed.shared_with.is_empty());
    }

    #[test]
    fn test_note_folder_round_trip() {
        let dir = temp_test_dir("folder_roundtrip");
        let mut note = Note::new("Filed note", "body", &dir);
        note.folder = normalize_folder("  Client   Work ");
        assert_eq!(note.folder.as_deref(), Some("Client Work"));

        let md = note.to_markdown();
        assert!(md.contains("\nfolder: \"Client Work\"\n"), "{md}");
        let parsed = Note::from_markdown(note.id.clone(), &md, note.file_path.clone());
        assert_eq!(parsed.folder.as_deref(), Some("Client Work"));

        note.folder = None;
        let md = note.to_markdown();
        assert!(!md.contains("folder:"), "{md}");
        let parsed = Note::from_markdown(note.id.clone(), &md, note.file_path.clone());
        assert_eq!(parsed.folder, None);

        assert_eq!(normalize_folder("   "), None);
        let _ = fs::remove_dir_all(&dir);
    }

    use super::*;
    use std::env;

    fn temp_test_dir(suffix: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!(
            "elora_notes_test_{}_{}",
            suffix,
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_frontmatter_roundtrip() {
        let dir = temp_test_dir("roundtrip");
        let mut note = Note::new(
            "Minimalist Design \"Notes\"",
            "Hello #rust and #design world!\n\nSecond paragraph.",
            &dir,
        );
        note.pinned = true;
        note.trashed = true;
        note.add_tag("architecture");

        let raw = note.to_markdown();
        let parsed = Note::from_markdown(note.id.clone(), &raw, note.file_path.clone());

        assert_eq!(parsed.title, "Minimalist Design \"Notes\"");
        assert!(parsed.pinned);
        assert!(parsed.trashed);
        assert_eq!(parsed.body, note.body);
        assert_eq!(
            parsed.all_tags(),
            vec![
                "architecture".to_string(),
                "design".to_string(),
                "rust".to_string()
            ]
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_inline_tag_extraction_ignores_headings_and_codeblocks() {
        let body = "\
# Main Heading
## Subheading
Here is an inline #thought and #project-alpha, plus (#work).
```rust
// #ignored_in_code
let color = \"#ff00aa\";
```
And a number #123 should be ignored, while #valid_tag is kept.";

        let tags = extract_inline_tags(body);
        assert_eq!(tags, vec!["project-alpha", "thought", "valid_tag", "work"]);
    }

    #[test]
    fn test_wiki_links_extraction_and_task_toggle() {
        let dir = temp_test_dir("wikilinks");
        let mut note = Note::new(
            "Knowledge Base",
            "See [[Project Roadmap]] and [[Reading & Project Ideas|Reading List]].\n\
             ```md\n\
             [[Ignored Link In Code]]\n\
             ```\n\
             - [ ] First task\n\
             - [x] Second task",
            &dir,
        );

        let links = note.outbound_links();
        assert_eq!(
            links,
            vec![
                "Project Roadmap".to_string(),
                "Reading & Project Ideas".to_string()
            ]
        );
        assert_eq!(note.task_progress(), (1, 2));

        assert!(note.toggle_task_at(0));
        assert_eq!(note.task_progress(), (2, 2));
        assert!(note.toggle_task_at(1));
        assert_eq!(note.task_progress(), (1, 2));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_search_and_tag_filtering() {
        let dir = temp_test_dir("filter");
        let mut note = Note::new(
            "Rust Cross-Platform GUI",
            "Building EloraNotes with egui and #rust for Linux, macOS, and Windows.",
            &dir,
        );
        note.add_tag("desktop");

        assert!(note.matches_filter("egui", None));
        assert!(note.matches_filter("cross", Some("desktop")));
        assert!(note.matches_filter("#rust", None));
        assert!(!note.matches_filter("egui", Some("nonexistent")));
        assert!(!note.matches_filter("python", None));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_relevance_ranks_title_hits_above_body_hits() {
        let dir = temp_test_dir("relevance");
        let title_hit = Note::new("Garden planning", "Notes about beds.", &dir);
        let body_hit = Note::new("Weekend", "garden garden garden tools", &dir);
        let miss = Note::new("Recipes", "Flour and eggs.", &dir);

        assert!(title_hit.relevance_score("garden") > 0);
        assert!(body_hit.relevance_score("garden") > 0);
        assert_eq!(miss.relevance_score("garden"), 0);
        assert!(title_hit.relevance_score("garden") > body_hit.relevance_score("garden"));
        // Tag tokens are not free text and should not affect the score.
        assert_eq!(miss.relevance_score("#garden"), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_body_snippet_returns_excerpt_around_match() {
        let dir = temp_test_dir("snippet");
        let note = Note::new(
            "Journal",
            "Long intro line.\nToday I finally fixed the Penguin bug in the renderer.\nMore text follows here.",
            &dir,
        );

        let snippet = note.body_snippet("penguin").expect("match expected");
        assert!(snippet.contains("Penguin"));
        assert!(!snippet.contains('\n'));
        assert_eq!(note.body_snippet("absent"), None);
        assert_eq!(note.body_snippet("#tag-only"), None);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_wiki_link_alias_and_code_block_handling() {
        let body = "See [[Alpha|Shown]] and [[Beta]].\n```\n[[Hidden]]\n```\nEmpty [[ ]] here.";
        assert_eq!(
            extract_wiki_links(body),
            vec!["Alpha".to_string(), "Beta".to_string()]
        );
    }

    #[test]
    fn test_fuzzy_subsequence_and_tag_normalization() {
        assert!(fuzzy_subsequence("meeting notes", "mtng"));
        assert!(!fuzzy_subsequence("meeting notes", "xyz"));
        assert_eq!(normalize_tag("  #Project/Alpha! "), "project/alpha");
    }

    #[test]
    fn test_pinned_sorting_and_storage_persistence() {
        let dir = temp_test_dir("storage");
        let storage = NoteStorage::new(dir.clone()).unwrap();

        let notes = storage.load_all().unwrap();
        assert_eq!(notes.len(), 3);
        assert!(notes[0].pinned);

        let mut custom = Note::new("Pinned Top Note", "Content #top", &dir);
        custom.pinned = true;
        storage.save_note(&custom).unwrap();

        let reloaded = storage.load_all().unwrap();
        assert_eq!(reloaded.len(), 4);
        assert!(reloaded
            .iter()
            .any(|n| n.title == "Pinned Top Note" && n.pinned));

        storage.delete_note(&custom).unwrap();
        let after_delete = storage.load_all().unwrap();
        assert_eq!(after_delete.len(), 3);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_import_markdown_keeps_frontmatter_and_new_ids() {
        let root = temp_test_dir("import_root");
        let src = temp_test_dir("import_src");
        let storage = NoteStorage::new(root.clone()).unwrap();
        fs::write(
            src.join("Ideas.md"),
            "---\ntitle: Ideas\ntags: [work]\n---\nBody with [[Other]]",
        )
        .unwrap();
        fs::write(src.join("notes.txt"), "ignored").unwrap();

        assert_eq!(storage.import_markdown_dir(&src).unwrap(), 1);
        let notes = storage.load_all().unwrap();
        let imported = notes.iter().find(|n| n.title == "Ideas").unwrap();
        assert!(imported.body.contains("Body with [[Other]]"));

        // Importing again adds a second copy instead of overwriting the first.
        assert_eq!(storage.import_markdown_dir(&src).unwrap(), 1);
        let again = storage.load_all().unwrap();
        assert_eq!(again.iter().filter(|n| n.title == "Ideas").count(), 2);

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&src);
    }

    #[test]
    fn test_default_notes_dir_is_documents_eloranotes() {
        assert_eq!(
            default_notes_dir_for(
                Some(PathBuf::from("/home/me/Documents")),
                Some(PathBuf::from("/home/me")),
            ),
            PathBuf::from("/home/me/Documents/EloraNotes")
        );
        // No Documents entry at all: fall back to <home>/Documents rather than giving up.
        assert_eq!(
            default_notes_dir_for(None, Some(PathBuf::from("/home/me"))),
            PathBuf::from("/home/me/Documents/EloraNotes")
        );
    }

    #[test]
    fn test_settings_and_notes_from_the_old_config_folder_are_copied() {
        let root = std::env::temp_dir().join("elora-migrate-test");
        let target = root.join("Documents/EloraNotes");
        let legacy = root.join(".config/eloranotes");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&target).unwrap();
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("Old.md"), "---\ntitle: Old\n---\nbody").unwrap();
        fs::write(legacy.join(".elora-settings.json"), "{}").unwrap();

        assert_eq!(migrate_from_dirs(&target, std::slice::from_ref(&legacy)), 2);
        assert!(target.join("Old.md").is_file());
        assert!(target.join(".elora-settings.json").is_file());
        // The old folder is left alone.
        assert!(legacy.join("Old.md").is_file());

        let _ = fs::remove_dir_all(&root);
    }

    /// Cards start without the preview line, and a settings file written while the line showed
    /// by default does not quietly keep it on.
    #[test]
    fn test_note_previews_start_off_and_old_settings_do_not_turn_them_on() {
        assert!(!AppSettings::default().show_card_previews);

        let old = serde_json::json!({
            "theme_name": "Obsidian",
            "show_formatting_bar": true,
            "show_card_previews": true,
        });
        let settings: AppSettings = serde_json::from_value(old).unwrap();
        assert!(
            !settings.show_card_previews,
            "the key from when the line was on by default is not read"
        );

        // A choice made now is kept, under the new key.
        let chosen = AppSettings {
            show_card_previews: true,
            ..AppSettings::default()
        };
        let text = serde_json::to_string(&chosen).unwrap();
        assert!(text.contains("\"note_previews\":true"), "{text}");
        let back: AppSettings = serde_json::from_str(&text).unwrap();
        assert!(back.show_card_previews);
    }

    /// A settings file from before the compact list existed opens in the compact style, sorts by
    /// last edited and has no window to restore; the chosen style, sort and window round-trip.
    #[test]
    fn test_cards_sort_and_window_start_compact_and_are_kept() {
        let old = serde_json::json!({
            "theme_name": "Obsidian",
            "show_formatting_bar": true,
        });
        let settings: AppSettings = serde_json::from_value(old).unwrap();
        assert!(!settings.roomy_cards, "the compact list is the default");
        assert_eq!(settings.sort, NoteSort::Updated);
        assert_eq!(settings.window, None);

        let chosen = AppSettings {
            roomy_cards: true,
            sort: NoteSort::Title,
            window: Some(WindowState {
                width: 1200.0,
                height: 800.0,
                x: 40.0,
                y: 60.0,
                maximized: true,
            }),
            ..AppSettings::default()
        };
        let text = serde_json::to_string(&chosen).unwrap();
        let back: AppSettings = serde_json::from_str(&text).unwrap();
        assert!(back.roomy_cards);
        assert_eq!(back.sort, NoteSort::Title);
        assert_eq!(back.window, chosen.window);
    }

    /// A colour picked for a person's caret is kept, and a settings file from before colours
    /// existed opens with none chosen, which is what leaves the pick to the name.
    #[test]
    fn test_a_chosen_colour_is_kept_and_no_colour_means_the_name_picks() {
        let settings = AppSettings::default();
        assert_eq!(settings.peer_color, None);

        let old = serde_json::json!({
            "theme_name": "Obsidian",
            "show_formatting_bar": true,
            "display_name": "nova",
        });
        let settings: AppSettings = serde_json::from_value(old).unwrap();
        assert_eq!(settings.peer_color, None);

        let chosen = AppSettings {
            peer_color: Some([12, 34, 56]),
            ..AppSettings::default()
        };
        let text = serde_json::to_string(&chosen).unwrap();
        let back: AppSettings = serde_json::from_str(&text).unwrap();
        assert_eq!(back.peer_color, Some([12, 34, 56]));
    }
}
