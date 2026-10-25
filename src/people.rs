//! Who is who on a server.
//!
//! A client goes by two things: a **display name** everybody sees, and an **identifier** that is
//! its key — a long random secret it only ever gives the server. This file is the server's side
//! of that: it remembers which name belongs to which key, so a note can name a person by the
//! name everybody knows while access is decided by the key only its owner has.
//!
//! The rule that keeps it safe is that a name never moves: the first key to claim a name keeps
//! it for good. Somebody who types a name that is already taken is refused at the handshake,
//! instead of being let in as that person. Keys are stored as their SHA-256 digest, so the
//! server's own file does not carry the secrets either.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;

/// What the registry remembers: the key each name belongs to, and the names each key has used.
///
/// A person may change their display name; the old one stays bound to them, so a note written
/// against it still finds them and nobody else can take it over afterwards.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Registry {
    /// `name (lowercased)` -> `key digest`.
    #[serde(default)]
    names: HashMap<String, String>,
    /// `key digest` -> the names it has claimed, as they were typed.
    #[serde(default)]
    identities: HashMap<String, Vec<String>>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

/// The shortest identifier accepted, in characters. Anything shorter is a name again rather
/// than a key: somebody who knows the person's display name could guess it, which is the whole
/// thing this keeps apart. The app makes 32-character identifiers, so this only turns away a
/// typo or a hand-written key.
pub const MIN_IDENTIFIER: usize = 16;

/// The most names one key may hold. A person renames themselves now and then, and every old
/// name stays theirs; without a limit, one key could reconnect under name after name and hold
/// every name the rest of the server is going to want.
pub const MAX_NAMES_PER_KEY: usize = 8;

/// The public key behind a client's secret identifier: a one-way digest of it.
///
/// This is the value the world is allowed to see. It goes into the server's name list and into
/// the notes a person owns or is named on, and it decides who may open them, because only the
/// holder of the identifier can produce the key again at the handshake. Seeing somebody's key
/// is therefore of no use to anybody: the identifier behind it is what grants access, and it
/// never leaves the computer it was made on.
pub fn public_key(identifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(identifier.trim().as_bytes());
    format!("{:x}", hasher.finalize())
}

impl Registry {
    /// Reads the registry beside the server's config. A file that is there but unreadable is an
    /// error rather than an empty registry: starting over would hand every taken name back to
    /// whoever claimed it next.
    pub fn load(path: PathBuf) -> Result<Self, String> {
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self {
                    path: Some(path),
                    ..Self::default()
                });
            }
            Err(error) => {
                return Err(format!("could not read {}: {error}", path.display()));
            }
        };
        let mut registry: Self = serde_json::from_str(&text)
            .map_err(|error| format!("{} is not valid: {error}", path.display()))?;
        registry.path = Some(path);
        Ok(registry)
    }

    /// Binds `name` to the key, or checks that it already belongs to it, and answers with every
    /// name that key goes by. The first key to claim a name keeps it.
    pub fn claim(&mut self, name: &str, key: &str) -> Result<Vec<String>, String> {
        let name = crate::net::normalize_identifier(name);
        if name.is_empty() {
            return Err("this app did not say what to call you".to_string());
        }
        let mut bound = false;
        match self.names.get(&name.to_lowercase()) {
            Some(owner) if owner != key => {
                return Err(format!(
                    "the name “{name}” belongs to somebody else on this server. Set a different \
                     name in Settings."
                ));
            }
            Some(_) => {}
            None => {
                if self.names_of(key).len() >= MAX_NAMES_PER_KEY {
                    return Err(format!(
                        "you already go by {MAX_NAMES_PER_KEY} names on this server, which is \
                         the most it keeps for one person. Use one of them in Settings, or ask \
                         the server's operator to release one."
                    ));
                }
                self.names.insert(name.to_lowercase(), key.to_string());
                bound = true;
            }
        }
        let used = self.identities.entry(key.to_string()).or_default();
        if !used.iter().any(|known| known.eq_ignore_ascii_case(&name)) {
            used.push(name);
            bound = true;
        }
        if bound {
            self.save()?;
        }
        Ok(self.names_of(key))
    }

    /// Every name this key has claimed. A note written against any of them is that person's.
    pub fn names_of(&self, key: &str) -> Vec<String> {
        self.identities.get(key).cloned().unwrap_or_default()
    }

    /// The public key a name belongs to, if it has been claimed. A note stores this beside the
    /// name: the name is the label, the key is who may open it.
    pub fn key_of(&self, name: &str) -> Option<&str> {
        let name = crate::net::normalize_identifier(name).to_lowercase();
        self.names.get(&name).map(String::as_str)
    }

    /// Frees a name that was claimed by the wrong key — the one thing the handshake cannot
    /// decide on its own, since it has no way to know who is sitting at the keyboard. Only the
    /// server's own operator runs this.
    pub fn release(&mut self, name: &str) -> Result<bool, String> {
        let name = crate::net::normalize_identifier(name);
        let Some(key) = self.names.remove(&name.to_lowercase()) else {
            return Ok(false);
        };
        if let Some(used) = self.identities.get_mut(&key) {
            used.retain(|known| !known.eq_ignore_ascii_case(&name));
            if used.is_empty() {
                self.identities.remove(&key);
            }
        }
        self.save()?;
        Ok(true)
    }

    /// Every claimed name with the key it belongs to, sorted by name, for the operator to
    /// check before notes are handed their keys.
    pub fn pairs(&self) -> Vec<(String, String)> {
        let mut rows: Vec<(String, String)> = self
            .names
            .iter()
            .map(|(lower, key)| {
                let typed = self
                    .identities
                    .get(key)
                    .and_then(|names| {
                        names
                            .iter()
                            .find(|name| name.to_lowercase() == *lower)
                            .cloned()
                    })
                    .unwrap_or_else(|| lower.clone());
                (typed, key.clone())
            })
            .collect();
        rows.sort_by_key(|(name, _)| name.to_lowercase());
        rows
    }

    fn save(&self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| format!("could not write who is who: {e}"))?;
        crate::storage::write_atomic(path, text)
            .map_err(|e| format!("could not write {}: {e}", path.display()))
    }

    /// The path this registry is kept at, for the tests.
    #[cfg(test)]
    fn path(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "elora-people-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("people.json")
    }

    /// One key cannot reconnect under name after name and sit on every one of them.
    #[test]
    fn test_one_key_holds_a_limited_number_of_names() {
        let path = temp_path("limit");
        let mut registry = Registry::load(path).unwrap();
        let nox = public_key("a-long-secret-of-nox");
        for n in 0..MAX_NAMES_PER_KEY {
            registry.claim(&format!("nox{n}"), &nox).unwrap();
        }
        assert!(registry.claim("mira", &nox).is_err());
        assert!(registry.key_of("mira").is_none());
        // A name it already holds still works.
        assert!(registry.claim("nox0", &nox).is_ok());
    }

    #[test]
    fn test_a_name_belongs_to_the_key_that_claimed_it_first() {
        let path = temp_path("claim");
        let mut registry = Registry::load(path.clone()).unwrap();
        let nox = public_key("a-long-secret-of-nox");
        let mira = public_key("a-long-secret-of-mira");

        assert_eq!(
            registry.claim("SuperSaiyan", &nox).unwrap(),
            ["SuperSaiyan"]
        );

        // The same key coming back under its name is fine, whatever the capitalisation.
        assert_eq!(
            registry.claim("supersaiyan", &nox).unwrap(),
            ["SuperSaiyan"],
            "the name is kept as it was first typed"
        );

        // Another key cannot take the name, and is told so.
        let refused = registry.claim("SuperSaiyan", &mira).unwrap_err();
        assert!(refused.contains("belongs to somebody else"), "{refused}");
        assert!(registry.names_of(&mira).is_empty());

        // The refusal survives a restart: it is on disk.
        let mut reloaded = Registry::load(path.clone()).unwrap();
        assert!(reloaded.claim("SuperSaiyan", &mira).is_err());
        assert_eq!(reloaded.names_of(&nox), ["SuperSaiyan"]);

        // The digest of the secret, not the secret, is what was written.
        let raw = std::fs::read_to_string(reloaded.path().unwrap()).unwrap();
        assert!(!raw.contains("a-long-secret-of-nox"), "{raw}");
        assert!(raw.contains(&nox), "{raw}");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn test_changing_a_name_keeps_the_old_one_bound() {
        let path = temp_path("rename");
        let mut registry = Registry::load(path.clone()).unwrap();
        let key = public_key("secret");
        registry.claim("Noxy", &key).unwrap();
        let names = registry.claim("Nox", &key).unwrap();
        assert!(names.contains(&"Noxy".to_string()));
        assert!(names.contains(&"Nox".to_string()));

        // Both names still point at the same key, so a note written under either one is theirs.
        assert_eq!(registry.key_of("Noxy"), Some(key.as_str()));
        assert_eq!(registry.key_of("NoX"), Some(key.as_str()));
        // ... and nobody else may take the old name on.
        let other = public_key("another-secret");
        assert!(registry.claim("Noxy", &other).is_err());

        // A registry with no path is usable in memory, for tests that do not need a file.
        let mut memory = Registry::default();
        assert!(memory.claim("Solo", &key).is_ok());
        assert!(memory.path().is_none());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A name claimed by the wrong person can be freed by the operator, and the person it
    /// really belongs to can then claim it.
    #[test]
    fn test_a_name_can_be_released_for_its_real_owner() {
        let path = temp_path("release");
        let mut registry = Registry::load(path.clone()).unwrap();
        let wrong = public_key("somebody-elses-identifier");
        let right = public_key("the-real-skorp-identifier");
        registry.claim("skorp", &wrong).unwrap();
        assert!(registry.claim("skorp", &right).is_err());

        assert!(registry.release("Skorp").unwrap(), "the name was taken");
        assert_eq!(registry.key_of("skorp"), None);
        assert!(registry.names_of(&wrong).is_empty(), "nothing is left over");
        assert!(
            registry.claim("skorp", &right).is_ok(),
            "the real owner can claim it now"
        );
        assert_eq!(registry.key_of("skorp"), Some(right.as_str()));
        assert!(!registry.release("nobody").unwrap(), "nothing to free");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// The list an operator reads: every name with the key behind it.
    #[test]
    fn test_the_operator_can_see_who_is_who() {
        let path = temp_path("pairs");
        let mut registry = Registry::load(path.clone()).unwrap();
        let nox = public_key("nox-secret");
        let mira = public_key("mira-secret");
        registry.claim("nox", &nox).unwrap();
        registry.claim("Mira", &mira).unwrap();
        registry.claim("Noxy", &nox).unwrap();

        let rows = registry.pairs();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], ("Mira".to_string(), mira.clone()));
        assert!(rows.iter().any(|(name, key)| name == "Noxy" && *key == nox));
        assert!(rows.iter().any(|(name, key)| name == "nox" && *key == nox));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn test_an_unreadable_registry_is_an_error_not_an_empty_one() {
        let path = temp_path("broken");
        std::fs::write(&path, "{not json").unwrap();
        assert!(Registry::load(path.clone()).is_err());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
