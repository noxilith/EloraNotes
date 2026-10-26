//! Setting EloraNotes up as an MCP server in the AI apps on this computer.
//!
//! Each app keeps its list of MCP servers somewhere of its own. For some it is a JSON file
//! (Claude Desktop, Cursor, VS Code, Windsurf, Gemini CLI); for Claude Code and Codex the app's
//! own `mcp add` command is the supported way in, so that is what is run. This module finds
//! which of them are installed, says whether the server is already registered and whether it is
//! registered with the command this copy of EloraNotes would use, and adds or removes it.
//!
//! It never touches anything until it is asked to. A JSON file is read as JSON and written back
//! with every other setting in it, in the order it had, after a copy of the old file is kept
//! beside it; a file that is not plain JSON (VS Code allows comments in its own) is left alone and
//! reported, because rewriting it could lose what the person wrote.

use crate::storage::write_atomic;
use serde_json::{json, Map, Value};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The name the server goes by in every app's list.
pub const SERVER_NAME: &str = "elora-notes";

/// The command that starts the server, as an app has to be told it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Client {
    ClaudeDesktop,
    ClaudeCode,
    Cursor,
    VsCode,
    Windsurf,
    GeminiCli,
    Codex,
}

impl Client {
    pub const ALL: [Client; 7] = [
        Client::ClaudeDesktop,
        Client::ClaudeCode,
        Client::Cursor,
        Client::VsCode,
        Client::Windsurf,
        Client::GeminiCli,
        Client::Codex,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Client::ClaudeDesktop => "Claude Desktop",
            Client::ClaudeCode => "Claude Code",
            Client::Cursor => "Cursor",
            Client::VsCode => "VS Code",
            Client::Windsurf => "Windsurf",
            Client::GeminiCli => "Gemini CLI",
            Client::Codex => "Codex",
        }
    }

    /// What to do once the server is added: apps read their MCP list when they start.
    pub fn after_adding(self) -> &'static str {
        match self {
            Client::ClaudeDesktop => "Quit Claude Desktop completely and open it again; closing the window is not enough.",
            Client::ClaudeCode => "Start a new Claude Code session to load it.",
            Client::Cursor => "Restart Cursor, or switch the server on under Settings → MCP.",
            Client::VsCode => "Reload the window, then check the Tools button in Copilot Chat.",
            Client::Windsurf => "Press refresh in the Cascade MCP menu, or restart Windsurf.",
            Client::GeminiCli => "Start a new gemini session to load it.",
            Client::Codex => "Start a new Codex session to load it.",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    MacOs,
    Windows,
}

/// Where things are on this computer. Everything is a parameter so the tests can build a
/// computer in a temporary folder.
#[derive(Debug, Clone)]
pub struct Environment {
    pub os: Os,
    pub home: PathBuf,
    /// `~/.config`, `~/Library/Application Support` or `%APPDATA%`.
    pub config_home: PathBuf,
    /// `%LOCALAPPDATA%` on Windows.
    pub local_data: Option<PathBuf>,
    /// Folders searched for the apps' command-line programs.
    pub path_dirs: Vec<PathBuf>,
    /// How long a call to an app's own command may take.
    pub cli_timeout: Duration,
}

impl Environment {
    pub fn current() -> Self {
        let os = if cfg!(target_os = "windows") {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Linux
        };
        let home = dirs::home_dir().unwrap_or_default();
        let config_home = dirs::config_dir().unwrap_or_else(|| home.join(".config"));
        let mut path_dirs: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|paths| std::env::split_paths(&paths).collect())
            .unwrap_or_default();
        // A program started from the desktop does not get the PATH a terminal has, and these are
        // where the apps' installers put their command-line programs.
        let mut usual = vec![
            home.join(".local/bin"),
            home.join(".claude/local"),
            home.join(".npm-global/bin"),
            home.join(".cargo/bin"),
            home.join(".bun/bin"),
            home.join(".volta/bin"),
            PathBuf::from("/usr/local/bin"),
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/bin"),
        ];
        if os == Os::Windows {
            usual.push(config_home.join("npm"));
        }
        for dir in usual {
            if !path_dirs.contains(&dir) {
                path_dirs.push(dir);
            }
        }
        Self {
            os,
            home,
            config_home,
            local_data: dirs::data_local_dir(),
            path_dirs,
            cli_timeout: Duration::from_secs(20),
        }
    }

    fn find_program(&self, name: &str) -> Option<PathBuf> {
        let names: Vec<String> = if self.os == Os::Windows {
            ["exe", "cmd", "bat"]
                .iter()
                .map(|ext| format!("{name}.{ext}"))
                .collect()
        } else {
            vec![name.to_string()]
        };
        self.path_dirs
            .iter()
            .flat_map(|dir| names.iter().map(move |n| dir.join(n)))
            .find(|candidate| candidate.is_file())
    }
}

/// What a scan found about one app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Not on this computer, as far as can be told.
    NotFound,
    /// Found, but it cannot be set up from here; the text says why.
    Unavailable(String),
    /// Found, and EloraNotes is not in its list.
    NotAdded,
    /// In its list, with the command this copy would use.
    Added,
    /// In its list, with a different command: another copy of EloraNotes, an older path, or
    /// different limits.
    Outdated,
    /// Its settings file cannot be read as JSON, so it is left alone.
    Unreadable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub client: Client,
    pub state: State,
    /// Where its list lives: a file, or "the claude command".
    pub place: String,
}

/// How to reach an app's list of MCP servers.
enum Method {
    Json {
        path: PathBuf,
        /// `mcpServers` for most apps, `servers` for VS Code.
        key: &'static str,
        /// VS Code wants `"type": "stdio"` on each entry.
        typed: bool,
    },
    Cli {
        program: PathBuf,
        kind: CliKind,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CliKind {
    Claude,
    Codex,
}

struct Plan {
    present: bool,
    method: Option<Method>,
    /// Why there is no method, when the app is present.
    why_not: String,
}

fn plan(env: &Environment, client: Client) -> Plan {
    let json_in = |dir: PathBuf, file: &str, key: &'static str, typed: bool, present: bool| Plan {
        present,
        method: Some(Method::Json {
            path: dir.join(file),
            key,
            typed,
        }),
        why_not: String::new(),
    };
    match client {
        Client::ClaudeDesktop => {
            let mut dir = env.config_home.join("Claude");
            // The Windows packaged app keeps its settings inside its own package folder.
            if env.os == Os::Windows {
                if let Some(local) = &env.local_data {
                    let packages = local.join("Packages");
                    if let Ok(entries) = std::fs::read_dir(packages) {
                        for entry in entries.flatten() {
                            let name = entry.file_name().to_string_lossy().to_string();
                            let inner = entry.path().join("LocalCache/Roaming/Claude");
                            if name.starts_with("Claude_") && inner.is_dir() {
                                dir = inner;
                                break;
                            }
                        }
                    }
                }
            }
            let app_installed = match env.os {
                Os::MacOs => Path::new("/Applications/Claude.app").exists(),
                Os::Windows => env
                    .local_data
                    .as_ref()
                    .is_some_and(|local| local.join("AnthropicClaude").exists()),
                Os::Linux => false,
            };
            let present = dir.is_dir() || app_installed;
            json_in(
                dir,
                "claude_desktop_config.json",
                "mcpServers",
                false,
                present,
            )
        }
        Client::Cursor => {
            let dir = env.home.join(".cursor");
            let present = dir.is_dir();
            json_in(dir, "mcp.json", "mcpServers", false, present)
        }
        Client::VsCode => {
            let dir = env.config_home.join("Code").join("User");
            let present = dir.is_dir();
            json_in(dir, "mcp.json", "servers", true, present)
        }
        Client::Windsurf => {
            // Windsurf's own documents name two places; an existing file wins.
            let codeium = env.home.join(".codeium");
            let inner = codeium.join("windsurf");
            let present = codeium.is_dir();
            if codeium.join("mcp_config.json").is_file() && !inner.join("mcp_config.json").is_file()
            {
                json_in(codeium, "mcp_config.json", "mcpServers", false, present)
            } else {
                json_in(inner, "mcp_config.json", "mcpServers", false, present)
            }
        }
        Client::GeminiCli => {
            let dir = env.home.join(".gemini");
            let present = dir.is_dir() || env.find_program("gemini").is_some();
            json_in(dir, "settings.json", "mcpServers", false, present)
        }
        Client::ClaudeCode => cli_plan(env, "claude", CliKind::Claude, ".claude"),
        Client::Codex => cli_plan(env, "codex", CliKind::Codex, ".codex"),
    }
}

fn cli_plan(env: &Environment, command: &str, kind: CliKind, settings_dir: &str) -> Plan {
    match env.find_program(command) {
        Some(program) => Plan {
            present: true,
            method: Some(Method::Cli { program, kind }),
            why_not: String::new(),
        },
        None => Plan {
            present: env.home.join(settings_dir).is_dir(),
            method: None,
            why_not: format!(
                "its settings are here, but the `{command}` command was not found; add it by hand with the command below"
            ),
        },
    }
}

/// Looks at every app and says what state it is in.
pub fn scan(env: &Environment, launch: &Launch) -> Vec<Report> {
    Client::ALL
        .iter()
        .map(|&client| report(env, client, launch))
        .collect()
}

pub fn report(env: &Environment, client: Client, launch: &Launch) -> Report {
    let plan = plan(env, client);
    if !plan.present {
        return Report {
            client,
            state: State::NotFound,
            place: String::new(),
        };
    }
    match &plan.method {
        None => Report {
            client,
            state: State::Unavailable(plan.why_not),
            place: String::new(),
        },
        Some(Method::Json { path, key, .. }) => Report {
            client,
            state: json_state(path, key, launch),
            place: path.display().to_string(),
        },
        Some(Method::Cli { program, .. }) => Report {
            client,
            state: cli_state(env, program, launch),
            place: format!("the {} command", program_label(program)),
        },
    }
}

fn program_label(program: &Path) -> String {
    program
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

// --- JSON files -------------------------------------------------------------------------------

const NOT_PLAIN_JSON: &str =
    "its settings file is not plain JSON (it may contain comments), so it was left alone; add the server by hand";

fn read_json(path: &Path) -> Result<Value, String> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(json!({})),
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(value) if value.is_object() => Ok(value),
            _ => Err(NOT_PLAIN_JSON.to_string()),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(format!("could not read {}: {e}", path.display())),
    }
}

fn entry_for(launch: &Launch, typed: bool) -> Value {
    let mut entry = Map::new();
    if typed {
        entry.insert("type".into(), json!("stdio"));
    }
    entry.insert("command".into(), json!(launch.program));
    entry.insert("args".into(), json!(launch.args));
    Value::Object(entry)
}

fn json_state(path: &Path, key: &str, launch: &Launch) -> State {
    let root = match read_json(path) {
        Ok(root) => root,
        Err(why) => return State::Unreadable(why),
    };
    let Some(entry) = root.get(key).and_then(|servers| servers.get(SERVER_NAME)) else {
        return State::NotAdded;
    };
    let same_program =
        entry.get("command").and_then(Value::as_str) == Some(launch.program.as_str());
    let same_args = entry
        .get("args")
        .and_then(Value::as_array)
        .is_some_and(|args| {
            args.len() == launch.args.len()
                && args
                    .iter()
                    .zip(&launch.args)
                    .all(|(have, want)| have.as_str() == Some(want.as_str()))
        });
    if same_program && same_args {
        State::Added
    } else {
        State::Outdated
    }
}

/// Changes the list of servers in a settings file and writes the file back, keeping everything
/// else in it. The old file is copied beside it first. Returns whether anything changed.
fn edit_json(
    path: &Path,
    key: &str,
    change: impl FnOnce(&mut Map<String, Value>) -> bool,
) -> Result<bool, String> {
    let mut root = read_json(path)?;
    let top = root
        .as_object_mut()
        .ok_or_else(|| NOT_PLAIN_JSON.to_string())?;
    let servers = top.entry(key.to_string()).or_insert_with(|| json!({}));
    let servers = servers
        .as_object_mut()
        .ok_or_else(|| format!("\"{key}\" in {} is not a list of servers", path.display()))?;
    if !change(servers) {
        return Ok(false);
    }
    if path.exists() {
        let mut backup = path.as_os_str().to_owned();
        backup.push(".elora-backup");
        std::fs::copy(path, PathBuf::from(backup))
            .map_err(|e| format!("could not keep a copy of {} first: {e}", path.display()))?;
    } else if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
    }
    let mut text = serde_json::to_string_pretty(&root).map_err(|e| e.to_string())?;
    text.push('\n');
    write_atomic(path, text).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    Ok(true)
}

// --- the apps' own commands -------------------------------------------------------------------

struct CommandResult {
    ok: bool,
    text: String,
    timed_out: bool,
}

/// Runs a program with no input and a time limit, and collects what it printed.
fn run(program: &Path, args: &[&str], limit: Duration) -> Result<CommandResult, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not run {}: {e}", program.display()))?;
    let (sender, receiver) = std::sync::mpsc::channel::<String>();
    let mut readers = 0;
    if let Some(mut out) = child.stdout.take() {
        let sender = sender.clone();
        readers += 1;
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = out.read_to_string(&mut text);
            let _ = sender.send(text);
        });
    }
    if let Some(mut err) = child.stderr.take() {
        let sender = sender.clone();
        readers += 1;
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = err.read_to_string(&mut text);
            let _ = sender.send(text);
        });
    }
    drop(sender);
    let started = Instant::now();
    let (ok, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(status)) => break (status.success(), false),
            Ok(None) if started.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                break (false, true);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(e) => return Err(e.to_string()),
        }
    };
    // What a program started by the command (such as the server it checks) leaves holding the
    // pipes open must not hold this up, so the output is only waited for a moment.
    let mut parts = Vec::new();
    for _ in 0..readers {
        match receiver.recv_timeout(Duration::from_millis(400)) {
            Ok(text) => parts.push(text),
            Err(_) => break,
        }
    }
    Ok(CommandResult {
        ok,
        text: parts.join("\n"),
        timed_out,
    })
}

/// The options that change what an assistant may do. An entry is out of date if it has one this
/// copy would not use, or lacks one it would.
const LIMIT_FLAGS: [&str; 3] = ["--read-only", "--no-shared", "--folder"];

fn cli_state(env: &Environment, program: &Path, launch: &Launch) -> State {
    match run(program, &["mcp", "get", SERVER_NAME], env.cli_timeout) {
        Err(why) => State::Unreadable(why),
        Ok(result) if result.timed_out => {
            State::Unreadable("it did not answer in time".to_string())
        }
        Ok(result) if !result.ok => State::NotAdded,
        Ok(result) => {
            // The app prints the command it holds, in its own layout, so it is searched for the
            // pieces of ours rather than compared as one line.
            let has_all = result.text.contains(&launch.program)
                && launch
                    .args
                    .iter()
                    .all(|arg| result.text.contains(arg.as_str()));
            let same_limits = LIMIT_FLAGS
                .iter()
                .all(|flag| result.text.contains(flag) == launch.args.iter().any(|a| a == flag));
            if has_all && same_limits {
                State::Added
            } else {
                State::Outdated
            }
        }
    }
}

fn cli_add(
    env: &Environment,
    program: &Path,
    kind: CliKind,
    launch: &Launch,
) -> Result<(), String> {
    // An entry with the same name would make `add` refuse, so any old one is removed first.
    let _ = cli_remove(env, program, kind);
    let mut args: Vec<&str> = vec!["mcp", "add"];
    if kind == CliKind::Claude {
        // For every project, not only the folder this happens to run from.
        args.extend(["--scope", "user"]);
    }
    args.push(SERVER_NAME);
    args.push("--");
    args.push(&launch.program);
    args.extend(launch.args.iter().map(String::as_str));
    let result = run(program, &args, env.cli_timeout)?;
    if result.timed_out {
        return Err("it did not answer in time".to_string());
    }
    if !result.ok {
        return Err(format!(
            "{} could not add it: {}",
            program_label(program),
            result.text.trim()
        ));
    }
    Ok(())
}

fn cli_remove(env: &Environment, program: &Path, kind: CliKind) -> Result<(), String> {
    let mut args: Vec<&str> = vec!["mcp", "remove", SERVER_NAME];
    if kind == CliKind::Claude {
        args.extend(["--scope", "user"]);
    }
    let result = run(program, &args, env.cli_timeout)?;
    if result.ok {
        Ok(())
    } else {
        Err(result.text.trim().to_string())
    }
}

// --- adding and removing ----------------------------------------------------------------------

/// Adds EloraNotes to an app's list, replacing an entry with the same name. Returns a sentence
/// to show.
pub fn add(env: &Environment, client: Client, launch: &Launch) -> Result<String, String> {
    let plan = plan(env, client);
    match plan.method {
        None => Err(plan.why_not),
        Some(Method::Json { path, key, typed }) => {
            let entry = entry_for(launch, typed);
            let changed = edit_json(&path, key, |servers| {
                if servers.get(SERVER_NAME) == Some(&entry) {
                    return false;
                }
                servers.insert(SERVER_NAME.to_string(), entry);
                true
            })?;
            Ok(if changed {
                format!("Added to {}. {}", client.name(), client.after_adding())
            } else {
                format!("{} already has it.", client.name())
            })
        }
        Some(Method::Cli { program, kind }) => {
            cli_add(env, &program, kind, launch)?;
            Ok(format!(
                "Added to {}. {}",
                client.name(),
                client.after_adding()
            ))
        }
    }
}

/// Takes EloraNotes out of an app's list and leaves everything else there.
pub fn remove(env: &Environment, client: Client) -> Result<String, String> {
    let plan = plan(env, client);
    match plan.method {
        None => Err(plan.why_not),
        Some(Method::Json { path, key, .. }) => {
            if !path.exists() {
                return Ok(format!("{} did not have it.", client.name()));
            }
            let changed = edit_json(&path, key, |servers| {
                servers.shift_remove(SERVER_NAME).is_some()
            })?;
            Ok(if changed {
                format!("Removed from {}.", client.name())
            } else {
                format!("{} did not have it.", client.name())
            })
        }
        Some(Method::Cli { program, kind }) => {
            cli_remove(env, &program, kind)?;
            Ok(format!("Removed from {}.", client.name()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn computer(name: &str) -> (Environment, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "elora-clients-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let home = root.join("home");
        let bin = root.join("bin");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&bin).unwrap();
        let env = Environment {
            os: Os::Linux,
            config_home: home.join(".config"),
            home,
            local_data: None,
            path_dirs: vec![bin],
            cli_timeout: Duration::from_secs(5),
        };
        (env, root)
    }

    fn launch(read_only: bool) -> Launch {
        let mut args = vec![
            "--mcp".to_string(),
            "--dir".to_string(),
            "/notes".to_string(),
        ];
        if read_only {
            args.push("--read-only".to_string());
        }
        Launch {
            program: "/opt/EloraNotes/elora".to_string(),
            args,
        }
    }

    fn state(env: &Environment, client: Client, l: &Launch) -> State {
        report(env, client, l).state
    }

    #[test]
    fn a_computer_with_none_of_the_apps_has_nothing_to_set_up() {
        let (env, root) = computer("empty");
        for found in scan(&env, &launch(false)) {
            assert_eq!(found.state, State::NotFound, "{:?}", found.client);
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn claude_desktop_is_added_updated_and_removed_without_disturbing_the_file() {
        let (env, root) = computer("desktop");
        let dir = env.config_home.join("Claude");
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("claude_desktop_config.json");
        // The person's own settings, in their own order, with another server already in it.
        fs::write(
            &file,
            r#"{
  "zeta": true,
  "mcpServers": { "other": { "command": "node", "args": ["x.js"] } },
  "alpha": { "keep": [1, 2, 3] }
}"#,
        )
        .unwrap();
        let l = launch(false);
        assert_eq!(state(&env, Client::ClaudeDesktop, &l), State::NotAdded);

        let said = add(&env, Client::ClaudeDesktop, &l).unwrap();
        assert!(said.contains("Quit Claude Desktop completely"), "{said}");
        assert_eq!(state(&env, Client::ClaudeDesktop, &l), State::Added);
        let written: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(
            written["mcpServers"]["elora-notes"]["command"],
            "/opt/EloraNotes/elora"
        );
        assert_eq!(
            written["mcpServers"]["elora-notes"]["args"],
            json!(["--mcp", "--dir", "/notes"])
        );
        assert_eq!(
            written["mcpServers"]["other"]["args"],
            json!(["x.js"]),
            "the other server stays"
        );
        assert_eq!(written["alpha"], json!({ "keep": [1, 2, 3] }));
        let keys: Vec<&String> = written.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["zeta", "mcpServers", "alpha"], "their order is kept");
        let backup = dir.join("claude_desktop_config.json.elora-backup");
        assert!(fs::read_to_string(backup)
            .unwrap()
            .contains("\"zeta\": true"));

        // The same again changes nothing.
        assert!(add(&env, Client::ClaudeDesktop, &l)
            .unwrap()
            .contains("already has it"));

        // Different limits make the entry out of date, and adding again brings it up to date.
        let limited = launch(true);
        assert_eq!(
            state(&env, Client::ClaudeDesktop, &limited),
            State::Outdated
        );
        add(&env, Client::ClaudeDesktop, &limited).unwrap();
        assert_eq!(state(&env, Client::ClaudeDesktop, &limited), State::Added);

        let gone = remove(&env, Client::ClaudeDesktop).unwrap();
        assert!(gone.contains("Removed"));
        let after: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert!(after["mcpServers"].get("elora-notes").is_none());
        assert!(
            after["mcpServers"].get("other").is_some(),
            "only ours is removed"
        );
        assert!(remove(&env, Client::ClaudeDesktop)
            .unwrap()
            .contains("did not have it"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_missing_settings_file_is_created_and_an_empty_one_is_filled() {
        let (env, root) = computer("create");
        fs::create_dir_all(env.home.join(".cursor")).unwrap();
        let l = launch(false);
        assert_eq!(state(&env, Client::Cursor, &l), State::NotAdded);
        add(&env, Client::Cursor, &l).unwrap();
        let file = env.home.join(".cursor/mcp.json");
        assert_eq!(state(&env, Client::Cursor, &l), State::Added);
        assert!(
            !env.home.join(".cursor/mcp.json.elora-backup").exists(),
            "nothing to keep a copy of"
        );

        fs::write(&file, "  \n").unwrap();
        assert_eq!(state(&env, Client::Cursor, &l), State::NotAdded);
        add(&env, Client::Cursor, &l).unwrap();
        assert_eq!(state(&env, Client::Cursor, &l), State::Added);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_file_that_is_not_plain_json_is_left_exactly_as_it_was() {
        let (env, root) = computer("comments");
        let dir = env.config_home.join("Code/User");
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("mcp.json");
        let original = "{\n  // my servers\n  \"servers\": {}\n}\n";
        fs::write(&file, original).unwrap();
        let l = launch(false);
        assert!(matches!(
            state(&env, Client::VsCode, &l),
            State::Unreadable(_)
        ));
        let why = add(&env, Client::VsCode, &l).unwrap_err();
        assert!(why.contains("not plain JSON"), "{why}");
        assert_eq!(fs::read_to_string(&file).unwrap(), original);
        assert!(!dir.join("mcp.json.elora-backup").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn each_app_gets_the_entry_in_its_own_shape_and_place() {
        let (env, root) = computer("shapes");
        let l = launch(false);
        for dir in [".cursor", ".gemini", ".codeium", ".config/Code/User"] {
            fs::create_dir_all(env.home.join(dir)).unwrap();
        }
        for client in [
            Client::Cursor,
            Client::GeminiCli,
            Client::Windsurf,
            Client::VsCode,
        ] {
            assert_eq!(state(&env, client, &l), State::NotAdded, "{client:?}");
            add(&env, client, &l).unwrap();
            assert_eq!(state(&env, client, &l), State::Added, "{client:?}");
        }
        let read = |path: &str| -> Value {
            serde_json::from_str(&fs::read_to_string(env.home.join(path)).unwrap()).unwrap()
        };
        assert!(read(".gemini/settings.json")["mcpServers"]["elora-notes"].is_object());
        assert!(read(".codeium/windsurf/mcp_config.json")["mcpServers"]["elora-notes"].is_object());
        let vscode = read(".config/Code/User/mcp.json");
        assert_eq!(
            vscode["servers"]["elora-notes"]["type"], "stdio",
            "VS Code wants a type"
        );
        assert!(vscode.get("mcpServers").is_none(), "and its own key");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn windsurf_uses_the_settings_file_that_already_exists() {
        let (env, root) = computer("windsurf");
        let codeium = env.home.join(".codeium");
        fs::create_dir_all(&codeium).unwrap();
        fs::write(codeium.join("mcp_config.json"), r#"{ "mcpServers": {} }"#).unwrap();
        add(&env, Client::Windsurf, &launch(false)).unwrap();
        assert!(fs::read_to_string(codeium.join("mcp_config.json"))
            .unwrap()
            .contains("elora-notes"));
        assert!(
            !codeium.join("windsurf").exists(),
            "the second place was not made"
        );
        let _ = fs::remove_dir_all(root);
    }

    /// A stand-in for an app's command-line program: it keeps its list in a file beside it and
    /// records what it was asked.
    #[cfg(unix)]
    fn fake_cli(dir: &Path, name: &str) {
        use std::os::unix::fs::PermissionsExt;
        let script = format!(
            r#"#!/bin/sh
echo "$@" >> "{dir}/{name}.calls"
case "$2" in
  get) [ -f "{dir}/{name}.entry" ] || {{ echo "No MCP server found with name: $3" >&2; exit 1; }}
       echo "elora-notes:"; sed 's/^/  Command: /' "{dir}/{name}.entry" ;;
  add) shift 2
       while [ "$1" != "--" ]; do shift; done; shift
       [ -f "{dir}/{name}.entry" ] && {{ echo "already exists" >&2; exit 1; }}
       echo "$*" > "{dir}/{name}.entry" ;;
  remove) [ -f "{dir}/{name}.entry" ] || exit 1; rm "{dir}/{name}.entry" ;;
  *) exit 2 ;;
esac
"#,
            dir = dir.display(),
            name = name
        );
        let path = dir.join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn claude_code_and_codex_are_set_up_through_their_own_commands() {
        let (env, root) = computer("cli");
        let bin = env.path_dirs[0].clone();
        fake_cli(&bin, "claude");
        fake_cli(&bin, "codex");
        let l = launch(false);
        for (client, name) in [(Client::ClaudeCode, "claude"), (Client::Codex, "codex")] {
            assert_eq!(state(&env, client, &l), State::NotAdded, "{client:?}");
            add(&env, client, &l).unwrap();
            assert_eq!(state(&env, client, &l), State::Added, "{client:?}");

            let calls = fs::read_to_string(bin.join(format!("{name}.calls"))).unwrap();
            let add_call = calls
                .lines()
                .find(|line| line.starts_with("mcp add"))
                .unwrap();
            let expected = if client == Client::ClaudeCode {
                "mcp add --scope user elora-notes -- /opt/EloraNotes/elora --mcp --dir /notes"
            } else {
                "mcp add elora-notes -- /opt/EloraNotes/elora --mcp --dir /notes"
            };
            assert_eq!(add_call, expected);

            // A change of limits shows as out of date; adding again replaces the old entry.
            let limited = launch(true);
            assert_eq!(
                state(&env, client, &limited),
                State::Outdated,
                "{client:?}: new limits"
            );
            add(&env, client, &limited).unwrap();
            assert!(fs::read_to_string(bin.join(format!("{name}.entry")))
                .unwrap()
                .contains("--read-only"));
            assert_eq!(state(&env, client, &limited), State::Added);
            assert_eq!(
                state(&env, client, &l),
                State::Outdated,
                "{client:?}: limits taken away"
            );

            remove(&env, client).unwrap();
            assert_eq!(state(&env, client, &l), State::NotAdded);
        }
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn a_command_found_only_in_the_usual_install_places_is_still_found() {
        let (mut env, root) = computer("usual");
        env.path_dirs = vec![env.home.join("nowhere")];
        let local_bin = env.home.join(".local/bin");
        fs::create_dir_all(&local_bin).unwrap();
        fake_cli(&local_bin, "claude");
        // `Environment::current` adds these places; here they are added the same way.
        env.path_dirs.push(local_bin);
        assert_eq!(
            state(&env, Client::ClaudeCode, &launch(false)),
            State::NotAdded
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn an_app_whose_settings_exist_but_whose_command_does_not_is_reported_not_guessed_at() {
        let (env, root) = computer("nocmd");
        fs::create_dir_all(env.home.join(".claude")).unwrap();
        match state(&env, Client::ClaudeCode, &launch(false)) {
            State::Unavailable(why) => {
                assert!(why.contains("`claude` command was not found"), "{why}")
            }
            other => panic!("{other:?}"),
        }
        assert!(add(&env, Client::ClaudeCode, &launch(false)).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn a_command_that_hangs_is_given_up_on() {
        use std::os::unix::fs::PermissionsExt;
        let (mut env, root) = computer("hang");
        let bin = env.path_dirs[0].clone();
        let path = bin.join("claude");
        fs::write(&path, "#!/bin/sh\nsleep 30\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        env.cli_timeout = Duration::from_millis(300);
        let started = Instant::now();
        assert!(matches!(
            state(&env, Client::ClaudeCode, &launch(false)),
            State::Unreadable(_)
        ));
        assert!(started.elapsed() < Duration::from_secs(5));
        let _ = fs::remove_dir_all(root);
    }
}
