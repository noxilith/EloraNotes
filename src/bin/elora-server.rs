//! `elora-server` — the machine that holds the shared library.
//!
//! `init` asks for the server name, port and password, writes the config and generates the TLS
//! certificate; `run` serves until it is stopped (this is what the installers register as a
//! boot service); `fingerprint` reprints the short code clients pin on first connect.

use elora_notes::net::{self, ServerConfig};
use elora_notes::people::Registry;
use elora_notes::server::Server;
use std::io::{BufRead, Write};
use std::process::ExitCode;
use std::sync::Arc;

const USAGE: &str = "\
elora-server — the EloraNotes collaboration server

USAGE:
    elora-server init [--name NAME] [--port PORT] [--password-stdin]
                      [--notes-dir DIR] [--bind ADDR]
    elora-server run
    elora-server fingerprint
    elora-server show
    elora-server version
    elora-server people [list]
    elora-server people release NAME
    elora-server people adopt [--write]

COMMANDS:
    init         Ask for (or take) the settings, write the config, create the TLS certificate.
    run          Serve the notes. This is what the installers start at boot.
    fingerprint  Print the short code clients are asked to confirm.
    version      Print the version of this server, so the build in use can be checked.
    show         Print the saved settings and the addresses other machines can use.
    people       Who goes by which name, and the key behind it: list, release a name that was
                 claimed by mistake, or give notes locked before keys existed their owner.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("run");
    let rest = if args.is_empty() {
        &args[..]
    } else {
        &args[1..]
    };

    let result = match command {
        "run" | "serve" => run(),
        "init" => init(rest),
        "fingerprint" | "fp" => fingerprint(),
        "show" | "info" => show(),
        "people" | "names" => people(rest),
        "version" | "--version" | "-V" => {
            println!("elora-server {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        "-h" | "--help" | "help" => {
            print!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown command '{other}'\n\n{USAGE}")),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("elora-server: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let config = ServerConfig::load()
        .map_err(|e| format!("{e}\n\nRun 'elora-server init' first to create the settings."))?;
    let server = Arc::new(Server::new(config)?);
    server.run()
}

fn fingerprint() -> Result<(), String> {
    let config = ServerConfig::load()?;
    let fingerprint = elora_notes::tls::read_fingerprint(&config)?;
    println!("{}", net::short_fingerprint(&fingerprint));
    println!("{fingerprint}");
    Ok(())
}

fn show() -> Result<(), String> {
    let config = ServerConfig::load()?;
    print_summary(&config, false);
    Ok(())
}

/// Who goes by what, and the two repairs the handshake cannot make by itself: a name claimed by
/// the wrong key, and notes that were locked before notes carried keys.
fn people(args: &[String]) -> Result<(), String> {
    let config = ServerConfig::load()
        .map_err(|e| format!("{e}\n\nRun 'elora-server init' first to create the settings."))?;
    let path = ServerConfig::data_dir().join("people.json");
    let mut people = Registry::load(path)?;
    let action = args.first().map(String::as_str).unwrap_or("list");

    match action {
        "list" | "show" => {
            let rows = people.pairs();
            if rows.is_empty() {
                println!(
                    "Nobody has connected yet: a name is bound to a key the first time it is used."
                );
                return Ok(());
            }
            println!("\n  Name                 Key");
            for (name, key) in rows {
                println!("  {:<20} {}", name, key);
            }
            println!(
                "\n  A name stays with the key that claimed it first. If one was claimed by the wrong\n  person, free it with: elora-server people release NAME"
            );
            Ok(())
        }
        "release" => {
            let Some(name) = args.get(1) else {
                return Err("which name?  elora-server people release NAME".to_string());
            };
            if people.release(name)? {
                println!(
                    "“{name}” is free again. Its owner can now connect and claim it; run \
                     `elora-server people adopt --write` afterwards if their notes were locked \
                     before keys existed."
                );
            } else {
                println!("Nobody had claimed “{name}”.");
            }
            Ok(())
        }
        "adopt" => {
            let write = args.iter().any(|arg| arg == "--write");
            let report = elora_notes::server::adopt_note_keys(&config.notes_dir, &people, write)?;
            if report.is_empty() {
                println!("Every locked note already has its keys.");
                return Ok(());
            }
            println!(
                "\n  {} locked note{} need{} attention:",
                report.len(),
                if report.len() == 1 { "" } else { "s" },
                if report.len() == 1 { "s" } else { "" }
            );
            for note in &report {
                println!("\n  “{}” ({})", note.title, note.id);
                for change in &note.changes {
                    println!("      {change}");
                }
            }
            if report.iter().any(|note| note.waiting) {
                println!(
                    "\n  A name nobody has claimed cannot be turned into a key: have that person \
                     connect once, then run this again.\n  If a name was claimed by the wrong key, \
                     free it first: elora-server people release NAME"
                );
            }
            println!(
                "\n{}",
                if write {
                    "Written. Restart the server: a running one would write its own copy back over \
                     these files."
                } else {
                    "Nothing was written. To apply this: stop the server, run it again with \
                     --write, and start the server afterwards."
                }
            );
            Ok(())
        }
        other => Err(format!("unknown people command '{other}'\n\n{USAGE}")),
    }
}

fn init(args: &[String]) -> Result<(), String> {
    let mut name = None;
    let mut port = None;
    let mut notes_dir = None;
    let mut bind = None;
    let mut password_stdin = false;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--name" => name = Some(value(args, &mut index, "--name")?),
            "--port" => {
                let raw = value(args, &mut index, "--port")?;
                port = Some(
                    raw.parse::<u16>()
                        .map_err(|_| format!("'{raw}' is not a port number"))?,
                );
            }
            "--notes-dir" => notes_dir = Some(value(args, &mut index, "--notes-dir")?),
            "--bind" => bind = Some(value(args, &mut index, "--bind")?),
            "--password-stdin" => {
                password_stdin = true;
                index += 1;
            }
            "--help" | "-h" => {
                print!("{USAGE}");
                return Ok(());
            }
            other => return Err(format!("unknown option '{other}'\n\n{USAGE}")),
        }
    }

    println!("EloraNotes server setup\n");

    let name = match name {
        Some(name) if !name.trim().is_empty() => name.trim().to_string(),
        _ => ask("Name people see in their sidebar", "EloraNotes")?,
    };
    let port = match port {
        Some(port) => port,
        None => loop {
            let raw = ask("Port", "7331")?;
            match raw.parse::<u16>() {
                Ok(port) if port >= 1024 => break port,
                Ok(_) => {
                    println!("  Pick a port above 1023 so the server does not need admin rights.")
                }
                Err(_) => println!("  That is not a port number."),
            }
        },
    };
    let bind = bind.unwrap_or_else(|| "0.0.0.0".to_string());
    let notes_dir = match notes_dir {
        Some(dir) => std::path::PathBuf::from(dir),
        None => ServerConfig::data_dir().join("notes"),
    };

    let password = if password_stdin {
        let mut line = String::new();
        std::io::stdin()
            .lock()
            .read_line(&mut line)
            .map_err(|e| format!("could not read the password: {e}"))?;
        let password = line.trim_end_matches(['\r', '\n']).to_string();
        if password.is_empty() {
            return Err("the password from stdin was empty".into());
        }
        password
    } else {
        let first = ask_password("Password everyone will connect with")?;
        if first.trim().is_empty() {
            return Err("the password cannot be empty".into());
        }
        let again = ask_password("Repeat the password")?;
        if first != again {
            return Err("the passwords did not match".into());
        }
        first
    };

    let config = ServerConfig {
        name,
        port,
        password,
        notes_dir,
        bind,
        ..ServerConfig::default()
    };

    std::fs::create_dir_all(&config.notes_dir)
        .map_err(|e| format!("could not create {}: {e}", config.notes_dir.display()))?;
    config.save()?;
    elora_notes::tls::ensure_identity(&config)?;

    println!(
        "\nSettings saved to {}",
        ServerConfig::config_path().display()
    );
    print_summary(&config, true);
    Ok(())
}

fn print_summary(config: &ServerConfig, during_install: bool) {
    let fingerprint = elora_notes::tls::read_fingerprint(config).unwrap_or_default();
    println!("\n  Server name: {}", config.name);
    println!("  Port:        {}", config.port);
    println!("  Notes:       {}", config.notes_dir.display());
    println!("  Bind:        {}", config.bind);
    if !fingerprint.is_empty() {
        println!("  Fingerprint: {}", net::short_fingerprint(&fingerprint));
    }

    let addresses = net::local_ipv4_addresses();
    if !addresses.is_empty() {
        println!("\n  Machines on the same network can use:");
        for ip in &addresses {
            println!("    {ip} / port {}", config.port);
        }
    }
    println!(
        "\n  From other networks, use this machine's public IP or hostname and port {};",
        config.port
    );
    println!(
        "  routers need TCP {} forwarded to this machine once.",
        config.port
    );

    if during_install {
        println!("\n  Start it with:  elora-server run");
        println!("  Clients will ask you to confirm the fingerprint above on the first connect.");
    }
}

fn value(args: &[String], index: &mut usize, flag: &str) -> Result<String, String> {
    let value = args
        .get(*index + 1)
        .ok_or_else(|| format!("{flag} needs a value"))?;
    *index += 2;
    Ok(value.clone())
}

/// A prompt whose answer is optional: an empty line keeps the default.
fn ask(question: &str, default: &str) -> Result<String, String> {
    print!("{question} [{default}]: ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| format!("could not read the answer: {e}"))?;
    let answer = line.trim();
    if answer.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(answer.to_string())
    }
}

/// The password is not hidden while typing, so people can see a typo before it is saved; it is
/// never echoed back.
fn ask_password(question: &str) -> Result<String, String> {
    print!("{question}: ");
    std::io::stdout().flush().ok();
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| format!("could not read the password: {e}"))?;
    println!();
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}
