//! Provider credentials in the OS credential store.
//!
//! [`KeyringCredentialStore`] implements core's [`CredentialReader`] and
//! [`CredentialWriter`] on top of the `keyring` crate (v4, "v1" mode), which
//! picks the native store per OS:
//!
//! * **Windows** – Credential Manager (generic credentials).
//! * **macOS** – the login Keychain.
//! * **Linux / BSD** – the freedesktop Secret Service (GNOME Keyring,
//!   KWallet ≥ 5.97, KeePassXC…) over D-Bus, using the pure-Rust `zbus`
//!   client, so building needs **no** system development packages. At run
//!   time a Secret Service provider must be running and unlocked; when it is
//!   not, every call returns a [`CredentialError`] explaining that the
//!   store is unavailable, and the UI should say so rather than fall back to
//!   storing tokens in plain files.
//!
//! Entries use service name [`SERVICE`] (`buildrail`) and the connection
//! alias as the account name, so users can find and remove them with the
//! OS's own tools ("Credential Manager", "Keychain Access", "Seahorse").
//!
//! Error messages never contain the secret: errors are rebuilt from the
//! `keyring` error kind, and any platform detail text is scrubbed of the
//! secret before it is kept.
//!
//! All calls block on IPC with the credential service, so call them from a
//! blocking context (`tokio::task::spawn_blocking`), not from a UI frame.
//!
//! [`InMemoryCredentialStore`] is a drop-in fake for tests and for the UI's
//! tests.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use buildrail_core::domain::ConnectionAlias;
use buildrail_core::ports::{CredentialError, CredentialReader, CredentialWriter, Secret};

/// Service name every BuildRail credential is stored under.
pub const SERVICE: &str = "buildrail";

/// The tiny slice of a keyring we need. A seam so the error mapping in
/// [`KeyringCredentialStore`] can be tested without a running credential
/// service; production code uses [`OsKeyring`].
pub trait KeyringBackend: Send + Sync {
    /// Reads the password stored for `(service, account)`.
    fn get(&self, service: &str, account: &str) -> keyring::Result<String>;
    /// Stores or replaces the password for `(service, account)`.
    fn set(&self, service: &str, account: &str, password: &str) -> keyring::Result<()>;
    /// Deletes the password for `(service, account)`.
    fn delete(&self, service: &str, account: &str) -> keyring::Result<()>;
}

/// The native credential store of the running OS, via `keyring`.
#[derive(Debug, Clone, Copy, Default)]
pub struct OsKeyring;

impl KeyringBackend for OsKeyring {
    fn get(&self, service: &str, account: &str) -> keyring::Result<String> {
        keyring::Entry::new(service, account)?.get_password()
    }

    fn set(&self, service: &str, account: &str, password: &str) -> keyring::Result<()> {
        keyring::Entry::new(service, account)?.set_password(password)
    }

    fn delete(&self, service: &str, account: &str) -> keyring::Result<()> {
        keyring::Entry::new(service, account)?.delete_credential()
    }
}

/// Credential store backed by the OS keyring. See the module docs.
#[derive(Debug, Clone, Default)]
pub struct KeyringCredentialStore<B = OsKeyring> {
    backend: B,
    service: String,
}

impl KeyringCredentialStore<OsKeyring> {
    /// The production store: OS keyring, service [`SERVICE`].
    pub fn new() -> Self {
        Self::with_backend(OsKeyring)
    }
}

impl<B: KeyringBackend> KeyringCredentialStore<B> {
    /// A store over any backend, using service [`SERVICE`].
    pub fn with_backend(backend: B) -> Self {
        Self {
            backend,
            service: SERVICE.to_owned(),
        }
    }

    /// Overrides the service name (for example to keep a development build's
    /// credentials apart from the installed app's).
    pub fn with_service(mut self, service: impl Into<String>) -> Self {
        self.service = service.into();
        self
    }

    /// The service name entries are stored under.
    pub fn service(&self) -> &str {
        &self.service
    }
}

impl<B: KeyringBackend> CredentialReader for KeyringCredentialStore<B> {
    fn read(&self, alias: &ConnectionAlias) -> Result<Option<Secret>, CredentialError> {
        match self.backend.get(&self.service, alias.as_str()) {
            Ok(password) => Ok(Some(Secret::new(password))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(err) => Err(map_error(Operation::Read, alias, &err, None)),
        }
    }
}

impl<B: KeyringBackend> CredentialWriter for KeyringCredentialStore<B> {
    fn write(&self, alias: &ConnectionAlias, secret: &Secret) -> Result<(), CredentialError> {
        self.backend
            .set(&self.service, alias.as_str(), secret.expose())
            .map_err(|err| map_error(Operation::Write, alias, &err, Some(secret)))
    }

    fn delete(&self, alias: &ConnectionAlias) -> Result<(), CredentialError> {
        match self.backend.delete(&self.service, alias.as_str()) {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(err) => Err(map_error(Operation::Delete, alias, &err, None)),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Operation {
    Read,
    Write,
    Delete,
}

impl Operation {
    fn verb(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "save",
            Self::Delete => "remove",
        }
    }
}

/// Builds a user-facing error from a keyring error. The text is assembled
/// from the error *kind*; payloads that can carry secret bytes
/// (`BadEncoding`, `BadDataFormat`) are dropped, and free-text platform
/// details are scrubbed of `secret`.
fn map_error(
    op: Operation,
    alias: &ConnectionAlias,
    err: &keyring::Error,
    secret: Option<&Secret>,
) -> CredentialError {
    let detail = match err {
        keyring::Error::NoStorageAccess(inner) => format!(
            "the OS credential store is locked or refused access ({})",
            scrub(&inner.to_string(), secret)
        ),
        keyring::Error::NoDefaultStore => {
            "no OS credential store is available (on Linux, is a Secret Service such as GNOME Keyring or KWallet running?)"
                .to_owned()
        }
        keyring::Error::PlatformFailure(inner) => format!(
            "the OS credential store failed ({})",
            scrub(&inner.to_string(), secret)
        ),
        keyring::Error::NoEntry => "no credential is stored".to_owned(),
        keyring::Error::BadEncoding(_) | keyring::Error::BadDataFormat(_, _) => {
            "the stored credential is not readable text; remove and reconnect".to_owned()
        }
        keyring::Error::BadStoreFormat(_) => "the OS credential store is corrupted".to_owned(),
        keyring::Error::TooLong(field, limit) => format!(
            "the {} is longer than the OS limit of {limit}",
            describe_field(field)
        ),
        keyring::Error::Invalid(field, _) => {
            format!("the {} is not accepted by the OS credential store", describe_field(field))
        }
        keyring::Error::Ambiguous(entries) => format!(
            "{} matching entries exist; remove the duplicates with the OS credential manager",
            entries.len()
        ),
        keyring::Error::NotSupportedByStore(what) => {
            format!("operation not supported by the OS credential store ({})", scrub(what, secret))
        }
        _ => "unexpected credential store error".to_owned(),
    };
    CredentialError(format!(
        "could not {} the credential for connection \"{alias}\": {detail}",
        op.verb()
    ))
}

/// Keyring field names are internal (`service`, `user`, `secret`…); say
/// something a user understands instead, and never echo a value.
fn describe_field(field: &str) -> &'static str {
    match field {
        "user" | "username" | "account" => "connection name",
        "service" | "target" => "service name",
        "password" | "secret" => "token",
        _ => "value",
    }
}

fn scrub(text: &str, secret: Option<&Secret>) -> String {
    match secret.map(Secret::expose) {
        Some(value) if !value.is_empty() => text.replace(value, "***"),
        _ => text.to_owned(),
    }
}

/// A credential store that lives in memory only. For tests and for the UI's
/// tests; never persists anything.
#[derive(Debug, Default)]
pub struct InMemoryCredentialStore {
    entries: Mutex<HashMap<ConnectionAlias, Secret>>,
}

impl InMemoryCredentialStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// A store pre-filled with `entries`.
    pub fn with_entries(entries: impl IntoIterator<Item = (ConnectionAlias, Secret)>) -> Self {
        Self {
            entries: Mutex::new(entries.into_iter().collect()),
        }
    }

    /// Aliases that currently have a credential, sorted.
    pub fn aliases(&self) -> Vec<ConnectionAlias> {
        let mut aliases: Vec<_> = self.lock().keys().cloned().collect();
        aliases.sort();
        aliases
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<ConnectionAlias, Secret>> {
        // A panic while holding the lock cannot leave a HashMap half-updated
        // in a way that matters here, so recover from poisoning.
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl CredentialReader for InMemoryCredentialStore {
    fn read(&self, alias: &ConnectionAlias) -> Result<Option<Secret>, CredentialError> {
        Ok(self.lock().get(alias).cloned())
    }
}

impl CredentialWriter for InMemoryCredentialStore {
    fn write(&self, alias: &ConnectionAlias, secret: &Secret) -> Result<(), CredentialError> {
        self.lock().insert(alias.clone(), secret.clone());
        Ok(())
    }

    fn delete(&self, alias: &ConnectionAlias) -> Result<(), CredentialError> {
        self.lock().remove(alias);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    const TOKEN: &str = "ghp_SUPERSECRET123";

    fn alias() -> ConnectionAlias {
        ConnectionAlias::new("work-github")
    }

    type Recorded = Vec<(String, String, String, Option<String>)>;

    /// Records calls and answers with a scripted error factory (keyring
    /// errors are not `Clone`).
    struct ScriptedBackend {
        error: Option<fn() -> keyring::Error>,
        stored: Mutex<HashMap<(String, String), String>>,
        calls: Mutex<Recorded>,
    }

    impl ScriptedBackend {
        fn ok() -> Self {
            Self {
                error: None,
                stored: Mutex::default(),
                calls: Mutex::default(),
            }
        }

        fn failing(error: fn() -> keyring::Error) -> Self {
            Self {
                error: Some(error),
                ..Self::ok()
            }
        }

        fn record(&self, op: &str, service: &str, account: &str, password: Option<&str>) {
            self.calls.lock().unwrap().push((
                op.to_owned(),
                service.to_owned(),
                account.to_owned(),
                password.map(str::to_owned),
            ));
        }

        fn fail(&self) -> keyring::Result<()> {
            match self.error {
                Some(make) => Err(make()),
                None => Ok(()),
            }
        }
    }

    impl KeyringBackend for ScriptedBackend {
        fn get(&self, service: &str, account: &str) -> keyring::Result<String> {
            self.record("get", service, account, None);
            self.fail()?;
            self.stored
                .lock()
                .unwrap()
                .get(&(service.to_owned(), account.to_owned()))
                .cloned()
                .ok_or(keyring::Error::NoEntry)
        }

        fn set(&self, service: &str, account: &str, password: &str) -> keyring::Result<()> {
            self.record("set", service, account, Some(password));
            self.fail()?;
            self.stored.lock().unwrap().insert(
                (service.to_owned(), account.to_owned()),
                password.to_owned(),
            );
            Ok(())
        }

        fn delete(&self, service: &str, account: &str) -> keyring::Result<()> {
            self.record("delete", service, account, None);
            self.fail()?;
            self.stored
                .lock()
                .unwrap()
                .remove(&(service.to_owned(), account.to_owned()))
                .map(|_| ())
                .ok_or(keyring::Error::NoEntry)
        }
    }

    fn platform(msg: &str) -> Box<dyn std::error::Error + Send + Sync> {
        Box::new(io::Error::other(msg.to_owned()))
    }

    #[test]
    fn round_trip_uses_service_and_alias() {
        let store = KeyringCredentialStore::with_backend(ScriptedBackend::ok());
        assert_eq!(store.service(), "buildrail");
        assert_eq!(store.read(&alias()), Ok(None));
        store.write(&alias(), &Secret::new(TOKEN)).unwrap();
        assert_eq!(store.read(&alias()), Ok(Some(Secret::new(TOKEN))));
        store.delete(&alias()).unwrap();
        assert_eq!(store.read(&alias()), Ok(None));
        let calls = store.backend.calls.lock().unwrap();
        assert!(calls
            .iter()
            .all(|(_, service, account, _)| service == "buildrail" && account == "work-github"));
        assert_eq!(calls.len(), 5);
    }

    #[test]
    fn deleting_a_missing_entry_is_ok() {
        let store = KeyringCredentialStore::with_backend(ScriptedBackend::ok());
        assert_eq!(store.delete(&alias()), Ok(()));
        let store = KeyringCredentialStore::with_backend(ScriptedBackend::failing(|| {
            keyring::Error::NoEntry
        }));
        assert_eq!(store.delete(&alias()), Ok(()));
        assert_eq!(store.read(&alias()), Ok(None));
    }

    #[test]
    fn custom_service_name() {
        let store = KeyringCredentialStore::with_backend(ScriptedBackend::ok())
            .with_service("buildrail-dev");
        store.write(&alias(), &Secret::new("x")).unwrap();
        assert_eq!(store.backend.calls.lock().unwrap()[0].1, "buildrail-dev");
    }

    fn write_error(make: fn() -> keyring::Error) -> String {
        let store = KeyringCredentialStore::with_backend(ScriptedBackend::failing(make));
        let err = store.write(&alias(), &Secret::new(TOKEN)).unwrap_err();
        let text = err.to_string();
        assert!(!text.contains(TOKEN), "secret leaked: {text}");
        assert!(
            !format!("{err:?}").contains(TOKEN),
            "secret leaked in Debug"
        );
        assert!(text.contains("work-github"), "{text}");
        text
    }

    #[test]
    fn errors_never_contain_the_secret_even_if_the_platform_echoes_it() {
        let text = write_error(|| {
            keyring::Error::PlatformFailure(platform("dbus said ghp_SUPERSECRET123 is bad"))
        });
        assert!(text.contains("could not save"));
        assert!(text.contains("dbus said *** is bad"), "{text}");
        let text =
            write_error(|| keyring::Error::NoStorageAccess(platform("locked: ghp_SUPERSECRET123")));
        assert!(text.contains("locked"), "{text}");
        let text = write_error(|| keyring::Error::NotSupportedByStore("ghp_SUPERSECRET123".into()));
        assert!(text.contains("not supported"), "{text}");
        let text = write_error(|| keyring::Error::BadEncoding(TOKEN.as_bytes().to_vec()));
        assert!(text.contains("not readable"), "{text}");
        let text = write_error(|| {
            keyring::Error::BadDataFormat(TOKEN.as_bytes().to_vec(), platform(TOKEN))
        });
        assert!(text.contains("not readable"), "{text}");
        let text = write_error(|| keyring::Error::Invalid("password".into(), TOKEN.into()));
        assert!(text.contains("token is not accepted"), "{text}");
    }

    #[test]
    fn every_error_kind_has_a_readable_message() {
        assert!(write_error(|| keyring::Error::NoDefaultStore).contains("Secret Service"));
        assert!(write_error(|| keyring::Error::BadStoreFormat("x".into())).contains("corrupted"));
        assert!(write_error(|| keyring::Error::TooLong("user".into(), 10))
            .contains("connection name is longer than the OS limit of 10"));
        assert!(
            write_error(|| keyring::Error::TooLong("service".into(), 5)).contains("service name")
        );
        assert!(
            write_error(|| keyring::Error::Invalid("weird".into(), "r".into()))
                .contains("value is not accepted")
        );
        assert!(
            write_error(|| keyring::Error::Ambiguous(Vec::new())).contains("0 matching entries")
        );
        assert!(write_error(|| keyring::Error::NoEntry).contains("no credential is stored"));
    }

    #[test]
    fn read_and_delete_errors_say_which_operation_failed() {
        let store = KeyringCredentialStore::with_backend(ScriptedBackend::failing(|| {
            keyring::Error::NoStorageAccess(platform("locked"))
        }));
        let err = store.read(&alias()).unwrap_err();
        assert!(
            err.0
                .starts_with("could not read the credential for connection \"work-github\""),
            "{err}"
        );
        let err = store.delete(&alias()).unwrap_err();
        assert!(err.0.starts_with("could not remove"), "{err}");
    }

    #[test]
    fn scrub_ignores_empty_secret() {
        assert_eq!(scrub("abc", Some(&Secret::new(""))), "abc");
        assert_eq!(scrub("abc", None), "abc");
    }

    #[test]
    fn in_memory_store_round_trip() {
        let store =
            InMemoryCredentialStore::with_entries([(ConnectionAlias::new("b"), Secret::new("1"))]);
        store
            .write(&ConnectionAlias::new("a"), &Secret::new("2"))
            .unwrap();
        assert_eq!(
            store.aliases(),
            vec![ConnectionAlias::new("a"), ConnectionAlias::new("b")]
        );
        assert_eq!(
            store.read(&ConnectionAlias::new("a")),
            Ok(Some(Secret::new("2")))
        );
        store.delete(&ConnectionAlias::new("a")).unwrap();
        store.delete(&ConnectionAlias::new("a")).unwrap();
        assert_eq!(store.read(&ConnectionAlias::new("a")), Ok(None));
        assert_eq!(
            InMemoryCredentialStore::new().aliases(),
            Vec::<ConnectionAlias>::new()
        );
    }

    #[test]
    fn stores_are_usable_as_trait_objects() {
        let reader: Box<dyn CredentialReader> = Box::new(KeyringCredentialStore::new());
        let _writer: Box<dyn CredentialWriter> = Box::new(InMemoryCredentialStore::new());
        drop(reader);
    }

    /// Talks to the real OS store; needs an unlocked Secret Service / Keychain
    /// / Credential Manager. Run manually with `--ignored`.
    #[test]
    #[ignore = "needs a real, unlocked OS credential store"]
    fn os_keyring_round_trip() {
        let store = KeyringCredentialStore::new().with_service("buildrail-test");
        let alias = ConnectionAlias::new("buildrail-platform-selftest");
        store.write(&alias, &Secret::new("value")).unwrap();
        assert_eq!(store.read(&alias).unwrap(), Some(Secret::new("value")));
        store.delete(&alias).unwrap();
        assert_eq!(store.read(&alias).unwrap(), None);
    }
}
