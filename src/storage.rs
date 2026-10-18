use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SortOrder {
    UpdatedDesc,
    CreatedDesc,
    TitleAsc,
}

impl SortOrder {
    pub fn next(self) -> Self {
        match self {
            Self::UpdatedDesc => Self::CreatedDesc,
            Self::CreatedDesc => Self::TitleAsc,
            Self::TitleAsc => Self::UpdatedDesc,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::UpdatedDesc => "Modified",
            Self::CreatedDesc => "Created",
            Self::TitleAsc => "Title A–Z",
        }
    }
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub theme_name: String,
    pub sort_order: SortOrder,
    pub show_formatting_bar: bool,
    #[serde(default = "default_highlight_rgb")]
    pub highlight_rgb: [u8; 3],
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    #[serde(default = "default_glow_strength")]
    pub glow_strength: f32,
    #[serde(default)]
    pub colors: crate::theme::ColorOverrides,
}

fn default_highlight_rgb() -> [u8; 3] {
    [255, 196, 77]
}

fn default_font_size() -> f32 {
    15.0
}

fn default_glow_strength() -> f32 {
    0.6
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme_name: "Obsidian".to_string(),
            sort_order: SortOrder::UpdatedDesc,
            show_formatting_bar: true,
            highlight_rgb: default_highlight_rgb(),
            font_size: default_font_size(),
            glow_strength: default_glow_strength(),
            colors: Default::default(),
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
            local.format("Today, %H:%M").to_string()
        } else {
            local.format("%b %d, %Y").to_string()
        }
    }

    /// Serializes the note into Markdown with YAML frontmatter.
    pub fn to_markdown(&self) -> String {
        let escaped_title = self.title.replace('\\', "\\\\").replace('"', "\\\"");
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

        format!(
            "---\ntitle: \"{}\"\npinned: {}\ntrashed: {}\ntags: {}\ncreated_at: {}\nupdated_at: {}\n---\n{}",
            escaped_title,
            self.pinned,
            self.trashed,
            tags_serialized,
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

        let flat = self.body[start..end].split_whitespace().collect::<Vec<_>>().join(" ");
        let prefix = if start > 0 { "\u{2026}" } else { "" };
        let suffix = if end < self.body.len() { "\u{2026}" } else { "" };
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
                let target = raw_target
                    .split('|')
                    .next()
                    .unwrap_or(raw_target)
                    .trim();
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
                matches!(c, ',' | '.' | ';' | ':' | '(' | ')' | '[' | ']' | '"' | '\'')
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
        if let Some(doc_dir) = dirs::document_dir() {
            return doc_dir.join("EloraNotes");
        }
        if let Some(home_dir) = dirs::home_dir() {
            return home_dir.join("EloraNotes");
        }
        PathBuf::from("notes")
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

    pub fn load_all(&self, sort_order: SortOrder) -> io::Result<Vec<Note>> {
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

        sort_notes(&mut notes, sort_order);
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

/// Sorts notes with Pinned notes first (among non-trashed notes), then according to `SortOrder`.
pub fn sort_notes(notes: &mut [Note], order: SortOrder) {
    notes.sort_by(|a, b| {
        a.trashed
            .cmp(&b.trashed)
            .then_with(|| b.pinned.cmp(&a.pinned))
            .then_with(|| match order {
                SortOrder::UpdatedDesc => b.updated_at.cmp(&a.updated_at),
                SortOrder::CreatedDesc => b.created_at.cmp(&a.created_at),
                SortOrder::TitleAsc => a
                    .display_title()
                    .to_lowercase()
                    .cmp(&b.display_title().to_lowercase()),
            })
            .then_with(|| a.title.cmp(&b.title))
    });
}

#[cfg(test)]
mod tests {
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

        let notes = storage.load_all(SortOrder::UpdatedDesc).unwrap();
        assert_eq!(notes.len(), 3);
        assert!(notes[0].pinned);

        let mut custom = Note::new("Pinned Top Note", "Content #top", &dir);
        custom.pinned = true;
        storage.save_note(&custom).unwrap();

        let reloaded = storage.load_all(SortOrder::UpdatedDesc).unwrap();
        assert_eq!(reloaded.len(), 4);
        assert!(reloaded
            .iter()
            .any(|n| n.title == "Pinned Top Note" && n.pinned));

        storage.delete_note(&custom).unwrap();
        let after_delete = storage.load_all(SortOrder::UpdatedDesc).unwrap();
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
        let notes = storage.load_all(SortOrder::UpdatedDesc).unwrap();
        let imported = notes.iter().find(|n| n.title == "Ideas").unwrap();
        assert!(imported.body.contains("Body with [[Other]]"));

        // Importing again adds a second copy instead of overwriting the first.
        assert_eq!(storage.import_markdown_dir(&src).unwrap(), 1);
        let again = storage.load_all(SortOrder::UpdatedDesc).unwrap();
        assert_eq!(again.iter().filter(|n| n.title == "Ideas").count(), 2);

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&src);
    }
}
