//! Keeps the OpenAI API key out of the plain settings file.
//!
//! The key lives in the operating system's credential store (Windows
//! Credential Manager on the shipped app). Older versions wrote the key into
//! `settings.json`; on start the key is moved to the credential store, and the
//! plain copy is dropped only after that move succeeds.
//!
//! This file has no dependencies so its logic can be tested with
//! `rustc --test src/api_key_store.rs`. The real credential store is wired up
//! in `lib.rs`.

/// A place that holds one secret: the user's OpenAI API key.
pub trait SecretStore {
    /// Returns the stored key, or `None` when nothing is stored.
    fn read(&self) -> Result<Option<String>, String>;
    /// Stores `key`, replacing any earlier key.
    fn write(&self, key: &str) -> Result<(), String>;
    /// Removes the stored key. Succeeds when nothing is stored.
    fn delete(&self) -> Result<(), String>;
}

/// What happened while loading the key at start-up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadOutcome {
    /// The key came from the credential store, or there is no key.
    Ready,
    /// A plain key from the settings file was copied into the credential
    /// store. The caller must now rewrite the settings file without it.
    Migrated,
    /// The plain key could not be copied into the credential store. It stays
    /// in the settings file and is still used.
    MigrationFailed(String),
    /// The credential store could not be read, so no key is available.
    ReadFailed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedKey {
    /// The key the app should use (empty when there is none).
    pub key: String,
    /// True while the settings file must keep its plain copy, because the
    /// move to the credential store has not succeeded yet.
    pub keep_plain_copy: bool,
    pub outcome: LoadOutcome,
}

/// Loads the key, moving a plain key from the settings file into `store`.
///
/// A plain key always wins over a stored one: it is only present when an
/// earlier move did not finish, or when an older version saved a newer key.
pub fn load(store: &dyn SecretStore, plain_key: &str) -> LoadedKey {
    let plain_key = plain_key.trim();
    if !plain_key.is_empty() {
        return match store.write(plain_key) {
            Ok(()) => LoadedKey {
                key: plain_key.to_string(),
                keep_plain_copy: false,
                outcome: LoadOutcome::Migrated,
            },
            Err(err) => LoadedKey {
                key: plain_key.to_string(),
                keep_plain_copy: true,
                outcome: LoadOutcome::MigrationFailed(err),
            },
        };
    }

    match store.read() {
        Ok(stored) => LoadedKey {
            key: stored.map(|key| key.trim().to_string()).unwrap_or_default(),
            keep_plain_copy: false,
            outcome: LoadOutcome::Ready,
        },
        Err(err) => LoadedKey {
            key: String::new(),
            keep_plain_copy: false,
            outcome: LoadOutcome::ReadFailed(err),
        },
    }
}

/// Replaces the stored key with `new_key`; an empty key deletes it.
///
/// Returns the trimmed key now in effect. On error nothing has changed, so the
/// caller must keep its previous key (and any plain copy) as they were.
pub fn replace(store: &dyn SecretStore, new_key: &str) -> Result<String, String> {
    let new_key = new_key.trim();
    if new_key.is_empty() {
        store.delete()?;
    } else {
        store.write(new_key)?;
    }
    Ok(new_key.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[derive(Default)]
    struct FakeStore {
        key: RefCell<Option<String>>,
        fail_reads: Cell<bool>,
        fail_writes: Cell<bool>,
        fail_deletes: Cell<bool>,
    }

    impl FakeStore {
        fn holding(key: &str) -> Self {
            let store = Self::default();
            *store.key.borrow_mut() = Some(key.to_string());
            store
        }

        fn stored(&self) -> Option<String> {
            self.key.borrow().clone()
        }
    }

    impl SecretStore for FakeStore {
        fn read(&self) -> Result<Option<String>, String> {
            if self.fail_reads.get() {
                return Err("store locked".into());
            }
            Ok(self.stored())
        }

        fn write(&self, key: &str) -> Result<(), String> {
            if self.fail_writes.get() {
                return Err("store unavailable".into());
            }
            *self.key.borrow_mut() = Some(key.to_string());
            Ok(())
        }

        fn delete(&self) -> Result<(), String> {
            if self.fail_deletes.get() {
                return Err("store unavailable".into());
            }
            *self.key.borrow_mut() = None;
            Ok(())
        }
    }

    #[test]
    fn plain_key_moves_into_the_store_and_drops_the_plain_copy() {
        let store = FakeStore::default();
        let loaded = load(&store, "  sk-legacy  ");
        assert_eq!(store.stored().as_deref(), Some("sk-legacy"));
        assert_eq!(
            loaded,
            LoadedKey {
                key: "sk-legacy".into(),
                keep_plain_copy: false,
                outcome: LoadOutcome::Migrated,
            }
        );
    }

    #[test]
    fn failed_move_keeps_the_plain_copy_and_still_uses_the_key() {
        let store = FakeStore::default();
        store.fail_writes.set(true);
        let loaded = load(&store, "sk-legacy");
        assert_eq!(store.stored(), None);
        assert_eq!(loaded.key, "sk-legacy");
        assert!(loaded.keep_plain_copy);
        assert_eq!(
            loaded.outcome,
            LoadOutcome::MigrationFailed("store unavailable".into())
        );
    }

    #[test]
    fn failed_move_is_retried_on_the_next_start() {
        let store = FakeStore::default();
        store.fail_writes.set(true);
        assert!(load(&store, "sk-legacy").keep_plain_copy);
        store.fail_writes.set(false);
        let loaded = load(&store, "sk-legacy");
        assert_eq!(loaded.outcome, LoadOutcome::Migrated);
        assert!(!loaded.keep_plain_copy);
        assert_eq!(store.stored().as_deref(), Some("sk-legacy"));
    }

    #[test]
    fn plain_key_replaces_an_older_stored_key() {
        let store = FakeStore::holding("sk-old");
        let loaded = load(&store, "sk-new");
        assert_eq!(loaded.key, "sk-new");
        assert_eq!(store.stored().as_deref(), Some("sk-new"));
    }

    #[test]
    fn stored_key_is_used_when_the_settings_file_has_none() {
        let store = FakeStore::holding("sk-secure");
        let loaded = load(&store, "");
        assert_eq!(
            loaded,
            LoadedKey {
                key: "sk-secure".into(),
                keep_plain_copy: false,
                outcome: LoadOutcome::Ready,
            }
        );
    }

    #[test]
    fn no_key_anywhere_loads_empty() {
        let loaded = load(&FakeStore::default(), "   ");
        assert_eq!(loaded.key, "");
        assert!(!loaded.keep_plain_copy);
        assert_eq!(loaded.outcome, LoadOutcome::Ready);
    }

    #[test]
    fn unreadable_store_loads_no_key() {
        let store = FakeStore::holding("sk-secure");
        store.fail_reads.set(true);
        let loaded = load(&store, "");
        assert_eq!(loaded.key, "");
        assert!(!loaded.keep_plain_copy);
        assert_eq!(
            loaded.outcome,
            LoadOutcome::ReadFailed("store locked".into())
        );
    }

    #[test]
    fn replacing_the_key_writes_the_trimmed_key() {
        let store = FakeStore::holding("sk-old");
        assert_eq!(replace(&store, "  sk-new ").as_deref(), Ok("sk-new"));
        assert_eq!(store.stored().as_deref(), Some("sk-new"));
    }

    #[test]
    fn failed_replace_leaves_the_old_key() {
        let store = FakeStore::holding("sk-old");
        store.fail_writes.set(true);
        assert_eq!(
            replace(&store, "sk-new"),
            Err("store unavailable".to_string())
        );
        assert_eq!(store.stored().as_deref(), Some("sk-old"));
    }

    #[test]
    fn empty_key_deletes_the_stored_key() {
        let store = FakeStore::holding("sk-old");
        assert_eq!(replace(&store, "   ").as_deref(), Ok(""));
        assert_eq!(store.stored(), None);
    }

    #[test]
    fn deleting_when_nothing_is_stored_succeeds() {
        let store = FakeStore::default();
        assert_eq!(replace(&store, "").as_deref(), Ok(""));
        assert_eq!(store.stored(), None);
    }

    #[test]
    fn failed_delete_leaves_the_old_key() {
        let store = FakeStore::holding("sk-old");
        store.fail_deletes.set(true);
        assert!(replace(&store, "").is_err());
        assert_eq!(store.stored().as_deref(), Some("sk-old"));
    }
}
