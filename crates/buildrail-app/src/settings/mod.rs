//! Settings and "Add build" as explicit state machines, with no egui.
//!
//! Every flow here follows the same pattern so it can be driven by tests
//! exactly like the UI drives it:
//!
//! ```text
//!   user input ──▶ machine.input(..) ──▶ Vec<Command> ──▶ controller.execute(..)
//!                                                           │ (async job / file write)
//!   machine.on_event(&Event) ◀── controller.poll_events() ◀─┘
//! ```
//!
//! * A [`Command`] is work the machine cannot do itself (network, OS
//!   credential store, disk). It carries a [`Ticket`].
//! * An [`Event`] is the result, carrying the same ticket. A machine only
//!   accepts events for the ticket it is waiting for, so a slow answer to a
//!   question the user has since abandoned (they went back and picked
//!   another repository) is ignored instead of clobbering newer state.
//! * Machines receive the data they read (current profile, local
//!   connections) through a [`SettingsContext`] borrowed per call, so they
//!   never hold stale copies of controller state.
//!
//! Modules:
//!
//! * [`connect`] – obtaining a credential (today: pasting a read-only token)
//!   behind [`connect::ConnectFlow`], so a device-flow sign-in can be added
//!   without touching the wizard or the import flow.
//! * [`wizard`] – Add build: provider → connection → target → preview →
//!   label → pinned.
//! * [`manage`] – rename, reorder, remove, reconnect, remove connection.
//! * [`import`] – import a teammate's profile (preview, alias mapping,
//!   atomic replace) and export.

pub mod connect;
pub mod import;
pub mod manage;
pub mod wizard;

use std::sync::atomic::{AtomicU64, Ordering};

use buildrail_core::domain::{BuildRecord, ConnectionAlias, ProviderKind, WatchId, WatchTarget};
use buildrail_core::ports::ProviderError;
use buildrail_profile::{LocalConnections, Profile, WatchDraft};
use buildrail_providers::catalog::{
    CloudflareAccount, CloudflareWorker, GitHubBranch, GitHubRepository, GitHubWorkflow,
};

pub use connect::{ConnectError, ConnectFlow, ConnectInput, ConnectRequest, NewConnection};

/// Correlates a [`Command`] with its [`Event`]. Unique per process.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Ticket(u64);

impl Ticket {
    /// A fresh ticket.
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// Read-only controller state a machine may consult while handling input.
#[derive(Debug, Clone, Copy)]
pub struct SettingsContext<'a> {
    /// The profile as currently saved.
    pub profile: &'a Profile,
    /// This device's connections (alias → provider + display name).
    pub connections: &'a LocalConnections,
}

/// A catalogue question for the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogQuery {
    /// GitHub repositories the token can see.
    Repositories,
    /// Workflows of a repository.
    Workflows {
        /// Repository owner.
        owner: String,
        /// Repository name.
        repo: String,
    },
    /// Branches of a repository.
    Branches {
        /// Repository owner.
        owner: String,
        /// Repository name.
        repo: String,
    },
    /// Cloudflare accounts the token can access.
    Accounts,
    /// Workers in an account.
    Workers {
        /// Account ID.
        account_id: String,
    },
}

/// A catalogue answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CatalogItems {
    /// Answer to [`CatalogQuery::Repositories`].
    Repositories(Vec<GitHubRepository>),
    /// Answer to [`CatalogQuery::Workflows`].
    Workflows(Vec<GitHubWorkflow>),
    /// Answer to [`CatalogQuery::Branches`].
    Branches(Vec<GitHubBranch>),
    /// Answer to [`CatalogQuery::Accounts`].
    Accounts(Vec<CloudflareAccount>),
    /// Answer to [`CatalogQuery::Workers`].
    Workers(Vec<CloudflareWorker>),
}

/// A change to the saved profile requested from Settings. The controller
/// applies it to a copy, saves atomically, and only then adopts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileEdit {
    /// Rename a watch.
    Rename {
        /// Watch to rename.
        id: WatchId,
        /// New label (validated by the profile).
        label: String,
    },
    /// Move the watch at `from` to position `to`.
    Move {
        /// Current position.
        from: usize,
        /// New position.
        to: usize,
    },
    /// Remove a watch.
    Remove {
        /// Watch to remove.
        id: WatchId,
    },
    /// Point a watch at another local connection.
    SetConnection {
        /// Watch to change.
        id: WatchId,
        /// Its new connection.
        connection: ConnectionAlias,
    },
}

/// Work a settings machine asks the controller to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Verify a credential, store it in the OS credential store and
    /// register the connection on this device.
    Connect {
        /// Correlation.
        ticket: Ticket,
        /// What to connect.
        request: ConnectRequest,
    },
    /// Ask a catalogue, with the stored credential of `connection`.
    Catalog {
        /// Correlation.
        ticket: Ticket,
        /// Connection whose credential to use.
        connection: ConnectionAlias,
        /// The question.
        query: CatalogQuery,
    },
    /// Fetch the latest run matching `target`, to confirm a selection.
    Preview {
        /// Correlation.
        ticket: Ticket,
        /// Connection whose credential to use.
        connection: ConnectionAlias,
        /// The target being previewed.
        target: WatchTarget,
    },
    /// Append a (validated) watch to the profile and save.
    Pin {
        /// Correlation.
        ticket: Ticket,
        /// The watch to pin.
        draft: WatchDraft,
    },
    /// Apply an edit to the profile and save.
    Edit {
        /// Correlation.
        ticket: Ticket,
        /// The edit.
        edit: ProfileEdit,
    },
    /// Replace the whole profile (import) and save.
    ReplaceProfile {
        /// Correlation.
        ticket: Ticket,
        /// The new profile.
        profile: Profile,
    },
    /// Delete a connection's credential and unregister it.
    RemoveConnection {
        /// Correlation.
        ticket: Ticket,
        /// The connection.
        alias: ConnectionAlias,
    },
}

/// The result of a [`Command`].
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Result of [`Command::Connect`].
    Connected {
        /// Correlation.
        ticket: Ticket,
        /// The registered connection, or why not.
        result: Result<NewConnection, ConnectError>,
    },
    /// Result of [`Command::Catalog`].
    Catalog {
        /// Correlation.
        ticket: Ticket,
        /// The items, or the provider error.
        result: Result<CatalogItems, LoadError>,
    },
    /// Result of [`Command::Preview`]. `Ok(None)` is an empty run history.
    Preview {
        /// Correlation.
        ticket: Ticket,
        /// The latest matching run, or the error.
        result: Result<Option<BuildRecord>, LoadError>,
    },
    /// Result of [`Command::Pin`].
    Pinned {
        /// Correlation.
        ticket: Ticket,
        /// The new watch's ID, or why it was not saved.
        result: Result<WatchId, String>,
    },
    /// Result of [`Command::Edit`], [`Command::ReplaceProfile`] and
    /// [`Command::RemoveConnection`].
    Saved {
        /// Correlation.
        ticket: Ticket,
        /// `Err` with a user-facing message when nothing changed.
        result: Result<(), String>,
    },
}

impl Event {
    /// The ticket this event answers.
    pub fn ticket(&self) -> Ticket {
        match self {
            Self::Connected { ticket, .. }
            | Self::Catalog { ticket, .. }
            | Self::Preview { ticket, .. }
            | Self::Pinned { ticket, .. }
            | Self::Saved { ticket, .. } => *ticket,
        }
    }
}

/// Why a catalogue or preview request failed, split the way the UI must
/// distinguish them: an access problem needs a reconnect, anything else a
/// retry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    /// The credential is missing, invalid or lacks a read permission.
    Access(String),
    /// The target does not exist or is hidden from this credential.
    NotFound(String),
    /// Network, rate limit or provider failure: try again.
    Other(String),
}

impl LoadError {
    /// Classifies a provider error and words it for `provider`.
    pub fn from_provider(provider: ProviderKind, error: &ProviderError) -> Self {
        let name = provider.display_name();
        match error {
            ProviderError::AccessDenied { .. } => Self::Access(format!(
                "{name} refused access. The token may be expired or missing a read permission; reconnect with a new token."
            )),
            ProviderError::NotFound { .. } => Self::NotFound(format!(
                "{name} could not find this, or the token cannot see it."
            )),
            ProviderError::RateLimited { .. } => {
                Self::Other(format!("{name} asked us to slow down. Try again in a minute."))
            }
            ProviderError::Network(_) => Self::Other(format!(
                "Could not reach {name}. Check your network connection and try again."
            )),
            ProviderError::Server { status, .. } => {
                Self::Other(format!("{name} had a problem (HTTP {status}). Try again later."))
            }
            ProviderError::Malformed(_) => {
                Self::Other(format!("{name} sent a response BuildRail could not read."))
            }
        }
    }

    /// No credential is stored for the connection.
    pub fn missing_credential(alias: &ConnectionAlias) -> Self {
        Self::Access(format!(
            "No token is stored for connection \"{alias}\" on this device; reconnect it."
        ))
    }

    /// The user-facing message.
    pub fn message(&self) -> &str {
        match self {
            Self::Access(message) | Self::NotFound(message) | Self::Other(message) => message,
        }
    }

    /// `true` for access problems (reconnect rather than retry).
    pub fn is_access(&self) -> bool {
        matches!(self, Self::Access(_))
    }
}

/// Something being loaded on behalf of a machine.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Loadable<T> {
    /// Not requested.
    #[default]
    Idle,
    /// Waiting for the answer to this ticket.
    Loading(Ticket),
    /// Loaded.
    Ready(T),
    /// Failed.
    Failed(LoadError),
}

impl<T> Loadable<T> {
    /// `true` while waiting for `ticket`.
    pub fn awaits(&self, ticket: Ticket) -> bool {
        matches!(self, Self::Loading(t) if *t == ticket)
    }

    /// `true` while waiting for anything.
    pub fn is_loading(&self) -> bool {
        matches!(self, Self::Loading(_))
    }

    /// The loaded value.
    pub fn ready(&self) -> Option<&T> {
        match self {
            Self::Ready(value) => Some(value),
            _ => None,
        }
    }
}

/// Labels of watches that use `alias`, for the "connection still in use"
/// guard.
pub fn watches_using(profile: &Profile, alias: &ConnectionAlias) -> Vec<String> {
    profile
        .watches()
        .iter()
        .filter(|w| &w.connection == alias)
        .map(|w| w.label.clone())
        .collect()
}

/// A free alias for a new connection: `github`, `github-2`, …
pub fn suggest_alias(provider: ProviderKind, connections: &LocalConnections) -> String {
    let base = match provider {
        ProviderKind::GitHub => "github",
        ProviderKind::Cloudflare => "cloudflare",
    };
    (1..)
        .map(|n| {
            if n == 1 {
                base.to_owned()
            } else {
                format!("{base}-{n}")
            }
        })
        .find(|candidate| !connections.contains_key(&ConnectionAlias::new(candidate.as_str())))
        .unwrap_or_else(|| base.to_owned())
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Shared fixtures for the settings machine tests.
    #![allow(clippy::unwrap_used)]

    use buildrail_core::domain::{ConnectionAlias, ProviderKind};
    use buildrail_profile::{LocalConnection, LocalConnections, Profile, WatchDraft};

    pub fn connections(entries: &[(&str, ProviderKind)]) -> LocalConnections {
        entries
            .iter()
            .map(|(alias, provider)| {
                (
                    ConnectionAlias::new(*alias),
                    LocalConnection {
                        provider: *provider,
                        display_name: format!("{alias}-user"),
                    },
                )
            })
            .collect()
    }

    pub fn profile() -> Profile {
        Profile::from_drafts(&[
            WatchDraft::github("Keevoro", "work", "acme", "keevoro", "ci.yml", None).with_id("k"),
            WatchDraft::cloudflare(
                "Site",
                "cf",
                "0123456789abcdef0123456789abcdef",
                "site",
                None,
            )
            .with_id("s"),
        ])
        .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::*;
    use super::*;

    #[test]
    fn tickets_are_unique_and_events_report_them() {
        let a = Ticket::next();
        let b = Ticket::next();
        assert_ne!(a, b);
        let events = [
            Event::Saved {
                ticket: a,
                result: Ok(()),
            },
            Event::Pinned {
                ticket: a,
                result: Err("x".into()),
            },
            Event::Preview {
                ticket: a,
                result: Ok(None),
            },
            Event::Catalog {
                ticket: a,
                result: Ok(CatalogItems::Accounts(vec![])),
            },
            Event::Connected {
                ticket: a,
                result: Err(ConnectError::Store("x".into())),
            },
        ];
        assert!(events.iter().all(|e| e.ticket() == a));
    }

    #[test]
    fn load_errors_are_classified() {
        let gh = ProviderKind::GitHub;
        let access = LoadError::from_provider(gh, &ProviderError::AccessDenied { provider: gh });
        assert!(access.is_access());
        assert!(access.message().contains("GitHub refused access"));
        let nf = LoadError::from_provider(gh, &ProviderError::NotFound { provider: gh });
        assert!(matches!(nf, LoadError::NotFound(_)));
        for error in [
            ProviderError::RateLimited {
                provider: gh,
                retry_after: None,
            },
            ProviderError::Network("x".into()),
            ProviderError::Server {
                provider: gh,
                status: 502,
            },
            ProviderError::Malformed("x".into()),
        ] {
            let e = LoadError::from_provider(gh, &error);
            assert!(!e.is_access());
            assert!(matches!(e, LoadError::Other(_)), "{error}");
        }
        let missing = LoadError::missing_credential(&ConnectionAlias::new("work"));
        assert!(missing.is_access());
        assert!(missing.message().contains("\"work\""));
    }

    #[test]
    fn loadable_helpers() {
        let t = Ticket::next();
        let l: Loadable<u8> = Loadable::Loading(t);
        assert!(l.awaits(t));
        assert!(!l.awaits(Ticket::next()));
        assert!(l.is_loading());
        assert_eq!(l.ready(), None);
        assert_eq!(Loadable::Ready(3).ready(), Some(&3));
        assert!(!Loadable::<u8>::default().is_loading());
    }

    #[test]
    fn aliases_and_usage() {
        let none = connections(&[]);
        assert_eq!(suggest_alias(ProviderKind::GitHub, &none), "github");
        let some = connections(&[
            ("github", ProviderKind::GitHub),
            ("github-2", ProviderKind::GitHub),
        ]);
        assert_eq!(suggest_alias(ProviderKind::GitHub, &some), "github-3");
        assert_eq!(suggest_alias(ProviderKind::Cloudflare, &some), "cloudflare");
        let p = profile();
        assert_eq!(
            watches_using(&p, &ConnectionAlias::new("work")),
            vec!["Keevoro"]
        );
        assert!(watches_using(&p, &ConnectionAlias::new("zzz")).is_empty());
    }
}
