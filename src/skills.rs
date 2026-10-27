//! The memory skill: instructions that teach an AI assistant to use EloraNotes as its long-term
//! memory, and the code that puts them where the assistants look.
//!
//! A skill is a folder holding a `SKILL.md`: a few lines of metadata saying what it is for, then
//! the instructions. An assistant reads only the metadata until a task calls for the skill, so
//! having it installed costs almost nothing. Claude Code reads `~/.claude/skills`; Codex, Gemini
//! CLI and Cursor all read the shared `~/.agents/skills`. Claude Desktop and claude.ai take a
//! skill as an uploaded ZIP and have no folder to write to, so for those a ZIP is saved instead.
//!
//! Like the MCP setup, nothing is written until it is asked for, a file that is not ours is kept
//! as a backup before it is replaced, and the result is read back before it is called done.

use crate::mcp_clients::Environment;
use crate::storage::write_atomic;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The skill's name, which is also its folder.
pub const NAME: &str = "elora-memory";

/// The skill itself.
pub const SKILL_MD: &str = include_str!("../assets/skills/elora-memory/SKILL.md");

/// A folder assistants read skills from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Place {
    ClaudeCode,
    /// `~/.agents/skills`, read by Codex, Gemini CLI and Cursor.
    Shared,
}

impl Place {
    pub const ALL: [Place; 2] = [Place::ClaudeCode, Place::Shared];

    pub fn name(self) -> &'static str {
        match self {
            Place::ClaudeCode => "Claude Code",
            Place::Shared => "Codex, Gemini CLI and Cursor",
        }
    }

    pub fn after_adding(self) -> &'static str {
        match self {
            Place::ClaudeCode => "Start a new Claude Code session; the skill loads when a task calls for memory.",
            Place::Shared => "Start a new session in the app (in Gemini CLI, /skills reload); the skill loads when a task calls for memory.",
        }
    }

    fn folder(self, env: &Environment) -> PathBuf {
        match self {
            Place::ClaudeCode => env.home.join(".claude").join("skills"),
            Place::Shared => env.home.join(".agents").join("skills"),
        }
    }

    /// The file the skill lives in for this place.
    pub fn file(self, env: &Environment) -> PathBuf {
        self.folder(env).join(NAME).join("SKILL.md")
    }

    fn present(self, env: &Environment) -> bool {
        let has_dir = |name: &str| env.home.join(name).is_dir();
        match self {
            Place::ClaudeCode => has_dir(".claude") || env.find_program("claude").is_some(),
            Place::Shared => {
                [".codex", ".gemini", ".cursor", ".agents"]
                    .iter()
                    .any(|dir| has_dir(dir))
                    || ["codex", "gemini", "cursor"]
                        .iter()
                        .any(|program| env.find_program(program).is_some())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// None of the apps that read this folder is on the computer.
    NotFound,
    NotAdded,
    /// There, and the same as this version of EloraNotes carries.
    Added,
    /// There, but different: an older version of the skill, or one that was edited by hand.
    Outdated,
    Unreadable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub place: Place,
    pub state: State,
    pub file: PathBuf,
}

fn state_of(file: &Path) -> State {
    match std::fs::read_to_string(file) {
        Ok(text) if text == SKILL_MD => State::Added,
        Ok(_) => State::Outdated,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => State::NotAdded,
        Err(e) => State::Unreadable(format!("could not read {}: {e}", file.display())),
    }
}

pub fn scan(env: &Environment) -> Vec<Report> {
    Place::ALL
        .iter()
        .map(|&place| {
            let file = place.file(env);
            let state = if place.present(env) {
                state_of(&file)
            } else {
                State::NotFound
            };
            Report { place, state, file }
        })
        .collect()
}

/// Puts the skill where the assistants read it. A different `SKILL.md` already there is kept
/// beside it as `SKILL.md.elora-backup` first.
pub fn add(env: &Environment, place: Place) -> Result<String, String> {
    let file = place.file(env);
    match state_of(&file) {
        State::Added => {
            return Ok(format!(
                "{} already has the memory skill ({}).",
                place.name(),
                file.display()
            ))
        }
        State::Outdated => {
            let mut backup = file.as_os_str().to_owned();
            backup.push(".elora-backup");
            std::fs::copy(&file, PathBuf::from(backup))
                .map_err(|e| format!("could not keep a copy of {} first: {e}", file.display()))?;
        }
        State::Unreadable(why) => return Err(why),
        State::NotAdded | State::NotFound => {
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
            }
        }
    }
    write_atomic(&file, SKILL_MD)
        .map_err(|e| format!("could not write {}: {e}", file.display()))?;
    if state_of(&file) != State::Added {
        return Err(format!(
            "wrote {} but it does not read back as the skill, so nothing was added",
            file.display()
        ));
    }
    Ok(format!(
        "Added the memory skill for {} in {}. {}",
        place.name(),
        file.display(),
        place.after_adding()
    ))
}

/// Takes the skill out again. Only its own file and its own folder go; anything else a person put
/// in that folder stays.
pub fn remove(env: &Environment, place: Place) -> Result<String, String> {
    let file = place.file(env);
    if !file.exists() {
        return Ok(format!("{} did not have the memory skill.", place.name()));
    }
    std::fs::remove_file(&file).map_err(|e| format!("could not remove {}: {e}", file.display()))?;
    if let Some(folder) = file.parent() {
        let _ = std::fs::remove_dir(folder);
    }
    Ok(format!("Removed the memory skill from {}.", place.name()))
}

/// The skill as a ZIP, the form Claude Desktop and claude.ai take it in: a folder named for the
/// skill with `SKILL.md` inside.
pub fn zip_bytes() -> Result<Vec<u8>, String> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buffer);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip.start_file(format!("{NAME}/SKILL.md"), options)
            .map_err(|e| e.to_string())?;
        zip.write_all(SKILL_MD.as_bytes())
            .map_err(|e| e.to_string())?;
        zip.finish().map_err(|e| e.to_string())?;
    }
    Ok(buffer.into_inner())
}

/// Saves the ZIP to `file`.
pub fn save_zip(file: &Path) -> Result<String, String> {
    let bytes = zip_bytes()?;
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    write_atomic(file, bytes).map_err(|e| format!("could not write {}: {e}", file.display()))?;
    Ok(format!(
        "Saved {}. In Claude Desktop or claude.ai, open Customize → Skills and upload it.",
        file.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp_clients::Os;
    use std::fs;
    use std::io::Read;
    use std::time::Duration;

    fn computer(name: &str) -> (Environment, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "elora-skills-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let home = root.join("home");
        fs::create_dir_all(&home).unwrap();
        let env = Environment {
            os: Os::Linux,
            config_home: home.join(".config"),
            home,
            local_data: None,
            path_dirs: vec![root.join("bin")],
            cli_timeout: Duration::from_secs(5),
            sandboxed: false,
        };
        (env, root)
    }

    fn state(env: &Environment, place: Place) -> State {
        scan(env)
            .into_iter()
            .find(|r| r.place == place)
            .unwrap()
            .state
    }

    #[test]
    fn the_skill_is_well_formed() {
        let text = SKILL_MD;
        assert!(text.starts_with("---\n"), "it opens with metadata");
        let end = text[4..].find("\n---\n").expect("the metadata is closed") + 4;
        let meta = &text[4..end];
        let field = |key: &str| {
            meta.lines()
                .find_map(|line| line.strip_prefix(&format!("{key}: ")))
                .unwrap_or_else(|| panic!("no {key}"))
        };
        let name = field("name");
        assert_eq!(name, NAME);
        assert!(
            name.len() <= 64
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        );
        assert!(
            !name.contains("claude") && !name.contains("anthropic"),
            "reserved words"
        );
        let description = field("description");
        assert!(
            description.len() >= 80 && description.len() <= 1024,
            "{}",
            description.len()
        );
        assert!(text.lines().count() < 500, "a skill is meant to be short");
    }

    #[test]
    fn every_tool_the_skill_names_exists() {
        let tools: Vec<String> = crate::mcp::tool_list()
            .iter()
            .filter_map(|tool| tool["name"].as_str().map(str::to_string))
            .collect();
        // Words in the skill that are fields of a tool's answer, not tools.
        let fields = ["read_only_server", "folder_limit", "read_only"];
        let mut mentioned = 0;
        for word in SKILL_MD.split('`').skip(1).step_by(2) {
            if word.contains('_') && word.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                mentioned += 1;
                assert!(
                    tools.iter().any(|t| t == word) || fields.contains(&word),
                    "the skill names `{word}`, which is not an EloraNotes tool"
                );
            }
        }
        assert!(mentioned >= 6, "it names the tools it uses");
        for needed in [
            "vault_info",
            "search_notes",
            "read_note",
            "create_note",
            "update_note",
            "trash_note",
        ] {
            assert!(SKILL_MD.contains(&format!("`{needed}`")), "{needed}");
        }
    }

    #[test]
    fn apps_that_are_not_there_are_not_offered_the_skill() {
        let (env, root) = computer("none");
        for report in scan(&env) {
            assert_eq!(report.state, State::NotFound, "{:?}", report.place);
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_skill_is_added_kept_up_to_date_and_removed() {
        let (env, root) = computer("lifecycle");
        fs::create_dir_all(env.home.join(".claude")).unwrap();
        fs::create_dir_all(env.home.join(".codex")).unwrap();
        assert_eq!(state(&env, Place::ClaudeCode), State::NotAdded);
        assert_eq!(state(&env, Place::Shared), State::NotAdded);

        let said = add(&env, Place::ClaudeCode).unwrap();
        let claude_file = env.home.join(".claude/skills/elora-memory/SKILL.md");
        assert_eq!(fs::read_to_string(&claude_file).unwrap(), SKILL_MD);
        assert!(said.contains(&claude_file.display().to_string()), "{said}");
        assert_eq!(state(&env, Place::ClaudeCode), State::Added);
        assert_eq!(
            state(&env, Place::Shared),
            State::NotAdded,
            "the other folder is its own"
        );
        assert!(add(&env, Place::ClaudeCode)
            .unwrap()
            .contains("already has"));

        // An older or hand-edited copy is out of date, and replacing it keeps what was there.
        fs::write(&claude_file, "my own version").unwrap();
        assert_eq!(state(&env, Place::ClaudeCode), State::Outdated);
        add(&env, Place::ClaudeCode).unwrap();
        assert_eq!(state(&env, Place::ClaudeCode), State::Added);
        let backup = claude_file.with_file_name("SKILL.md.elora-backup");
        assert_eq!(fs::read_to_string(backup).unwrap(), "my own version");

        add(&env, Place::Shared).unwrap();
        assert!(env
            .home
            .join(".agents/skills/elora-memory/SKILL.md")
            .exists());

        // Removing takes the skill out and leaves a person's own files alone.
        fs::write(claude_file.with_file_name("notes.txt"), "mine").unwrap();
        remove(&env, Place::ClaudeCode).unwrap();
        assert!(!claude_file.exists());
        assert!(
            claude_file.with_file_name("notes.txt").exists(),
            "their file stays, and so does the folder"
        );
        assert!(remove(&env, Place::ClaudeCode)
            .unwrap()
            .contains("did not have"));
        remove(&env, Place::Shared).unwrap();
        assert!(
            !env.home.join(".agents/skills/elora-memory").exists(),
            "an empty folder of ours goes too"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn the_zip_holds_the_skill_in_a_folder_of_its_own_name() {
        let bytes = zip_bytes().unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(archive.len(), 1);
        let mut entry = archive.by_index(0).unwrap();
        assert_eq!(&*entry.name().unwrap(), "elora-memory/SKILL.md");
        let mut text = String::new();
        entry.read_to_string(&mut text).unwrap();
        assert_eq!(text, SKILL_MD);

        let (_, root) = computer("zip");
        let file = root.join("Downloads/elora-memory-skill.zip");
        let said = save_zip(&file).unwrap();
        assert!(said.contains("Customize → Skills"), "{said}");
        assert!(zip::ZipArchive::new(fs::File::open(&file).unwrap()).is_ok());
        let _ = fs::remove_dir_all(root);
    }
}
