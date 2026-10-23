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

/// The digest a client's secret identifier is remembered by. Compared, never shown.
pub fn key_digest(identifier: &str) -> String {
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
        std::fs::write(path, text).map_err(|e| format!("could not write {}: {e}", path.display()))
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

    #[test]
    fn test_a_name_belongs_to_the_key_that_claimed_it_first() {
        let path = temp_path("claim");
        let mut registry = Registry::load(path.clone()).unwrap();
        let nox = key_digest("a-long-secret-of-nox");
        let mira = key_digest("a-long-secret-of-mira");

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
        let key = key_digest("secret");
        registry.claim("Noxy", &key).unwrap();
        let names = registry.claim("Nox", &key).unwrap();
        assert!(names.contains(&"Noxy".to_string()));
        assert!(names.contains(&"Nox".to_string()));

        // A note written against the old name still belongs to the same person...
        assert!(crate::net::visible_to(
            &crate::net::NoteMeta {
                id: "n".into(),
                title: "Note".into(),
                folder: None,
                tags: Vec::new(),
                pinned: false,
                trashed: false,
                icon: None,
                updated_at: 0,
                owner: Some("Noxy".into()),
                private: true,
                shared_with: Vec::new(),
            },
            &names
        ));
        // ... and nobody else may take the old name on.
        let other = key_digest("another-secret");
        assert!(registry.claim("Noxy", &other).is_err());

        // A registry with no path is usable in memory, for tests that do not need a file.
        let mut memory = Registry::default();
        assert!(memory.claim("Solo", &key).is_ok());
        assert!(memory.path().is_none());
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
