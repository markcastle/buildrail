//! Importing a teammate's profile: preview, map aliases, apply.
//!
//! ```text
//!  TOML text ──parse+validate──▶ ImportPreview ──apply(mapping)──▶ Profile ──ProfileStore::save──▶ disk
//!                                 │  diff vs current
//!                                 │  required (alias, provider) pairs
//!                                 └─ suggested_mapping(local connections)
//! ```
//!
//! Nothing in this module touches the filesystem. The caller saves the
//! applied profile with [`crate::ProfileStore::save`], which is atomic, so
//! a malformed import or an incomplete mapping leaves the working
//! configuration exactly as it was.

use std::collections::{BTreeMap, HashMap};
use std::fmt;

use buildrail_core::domain::{ConnectionAlias, ProviderKind, Watch, WatchId};
use thiserror::Error;

use crate::device::LocalConnection;
use crate::format::{parse_profile, DocumentError};
use crate::profile::Profile;
use crate::validate::ValidationErrors;

/// Local connections by alias, as kept in [`crate::DevicePrefs::connections`].
pub type LocalConnections = BTreeMap<ConnectionAlias, LocalConnection>;

/// What differs between two profiles, watch by watch (matched by ID).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileDiff {
    /// One entry per watch in either profile: first the watches of the new
    /// profile in their new order, then removed watches in their old order.
    pub entries: Vec<DiffEntry>,
    /// `true` when watches present in both profiles appear in a different
    /// relative order. (Insertions and removals alone do not count.)
    pub reordered: bool,
}

/// One watch in a [`ProfileDiff`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffEntry {
    /// The watch's stable ID.
    pub id: WatchId,
    /// Position in the old profile, if it was there.
    pub before_index: Option<usize>,
    /// Position in the new profile, if it is there.
    pub after_index: Option<usize>,
    /// What happened to it.
    pub kind: DiffKind,
}

/// What happened to one watch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffKind {
    /// Only in the new profile.
    Added(Watch),
    /// Only in the old profile.
    Removed(Watch),
    /// In both, with different contents.
    Changed {
        /// Old version.
        before: Watch,
        /// New version.
        after: Watch,
        /// Which parts differ (never empty).
        changes: Vec<WatchChange>,
    },
    /// In both with identical contents (its position may still differ; see
    /// [`ProfileDiff::reordered`]).
    Unchanged(Watch),
}

/// A part of a watch that changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WatchChange {
    /// The label.
    Label,
    /// The connection alias.
    Connection,
    /// Provider or provider identifiers (repository, workflow, account,
    /// worker).
    Target,
    /// Only the branch filter.
    Branch,
}

impl ProfileDiff {
    /// Compares `before` with `after`.
    pub fn between(before: &Profile, after: &Profile) -> Self {
        let old_index: HashMap<&WatchId, usize> = before
            .watches()
            .iter()
            .enumerate()
            .map(|(index, watch)| (&watch.id, index))
            .collect();
        let mut entries = Vec::with_capacity(before.len().max(after.len()));
        let mut common_old_positions = Vec::new();
        for (after_index, watch) in after.watches().iter().enumerate() {
            let before_index = old_index.get(&watch.id).copied();
            let kind = match before_index.and_then(|i| before.watches().get(i)) {
                None => DiffKind::Added(watch.clone()),
                Some(old) => {
                    common_old_positions.extend(before_index);
                    let changes = changes_between(old, watch);
                    if changes.is_empty() {
                        DiffKind::Unchanged(watch.clone())
                    } else {
                        DiffKind::Changed {
                            before: old.clone(),
                            after: watch.clone(),
                            changes,
                        }
                    }
                }
            };
            entries.push(DiffEntry {
                id: watch.id.clone(),
                before_index,
                after_index: Some(after_index),
                kind,
            });
        }
        for (index, watch) in before.watches().iter().enumerate() {
            if after.get(&watch.id).is_none() {
                entries.push(DiffEntry {
                    id: watch.id.clone(),
                    before_index: Some(index),
                    after_index: None,
                    kind: DiffKind::Removed(watch.clone()),
                });
            }
        }
        let reordered = common_old_positions.windows(2).any(|w| w[0] > w[1]);
        Self { entries, reordered }
    }

    /// `true` when applying would change nothing (same watches, same order).
    pub fn is_identity(&self) -> bool {
        !self.reordered
            && self
                .entries
                .iter()
                .all(|entry| matches!(entry.kind, DiffKind::Unchanged(_)))
    }

    /// Number of added, removed, changed and unchanged watches, for a
    /// summary line ("3 added, 1 removed, …").
    pub fn counts(&self) -> DiffCounts {
        let mut counts = DiffCounts::default();
        for entry in &self.entries {
            match entry.kind {
                DiffKind::Added(_) => counts.added += 1,
                DiffKind::Removed(_) => counts.removed += 1,
                DiffKind::Changed { .. } => counts.changed += 1,
                DiffKind::Unchanged(_) => counts.unchanged += 1,
            }
        }
        counts
    }
}

/// Totals of a [`ProfileDiff`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiffCounts {
    /// Watches only in the new profile.
    pub added: usize,
    /// Watches only in the old profile.
    pub removed: usize,
    /// Watches in both with different contents.
    pub changed: usize,
    /// Watches in both with identical contents.
    pub unchanged: usize,
}

fn changes_between(old: &Watch, new: &Watch) -> Vec<WatchChange> {
    let mut changes = Vec::new();
    if old.label != new.label {
        changes.push(WatchChange::Label);
    }
    if old.connection != new.connection {
        changes.push(WatchChange::Connection);
    }
    if old.target != new.target {
        let mut old_target = old.target.clone();
        clear_branch(&mut old_target);
        let mut new_target = new.target.clone();
        clear_branch(&mut new_target);
        changes.push(if old_target == new_target {
            WatchChange::Branch
        } else {
            WatchChange::Target
        });
    }
    changes
}

fn clear_branch(target: &mut buildrail_core::domain::WatchTarget) {
    use buildrail_core::domain::WatchTarget;
    match target {
        WatchTarget::GitHub { branch, .. } | WatchTarget::Cloudflare { branch, .. } => {
            *branch = None;
        }
    }
}

/// A connection the imported profile refers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequiredConnection {
    /// The alias as written by the sender.
    pub alias: ConnectionAlias,
    /// The provider the watches using it need.
    pub provider: ProviderKind,
    /// How many imported watches use this pair.
    pub watch_count: usize,
}

/// The recipient's choice: for each imported `(alias, provider)` pair, the
/// local alias to use instead.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AliasMapping {
    map: BTreeMap<(ConnectionAlias, ProviderKind), ConnectionAlias>,
}

impl AliasMapping {
    /// An empty mapping.
    pub fn new() -> Self {
        Self::default()
    }

    /// Maps the imported `alias` (for `provider`) to `local`.
    pub fn insert(
        &mut self,
        alias: ConnectionAlias,
        provider: ProviderKind,
        local: ConnectionAlias,
    ) {
        self.map.insert((alias, provider), local);
    }

    /// Removes a choice.
    pub fn remove(&mut self, alias: &ConnectionAlias, provider: ProviderKind) {
        self.map.remove(&(alias.clone(), provider));
    }

    /// The local alias chosen for an imported pair.
    pub fn get(&self, alias: &ConnectionAlias, provider: ProviderKind) -> Option<&ConnectionAlias> {
        self.map.get(&(alias.clone(), provider))
    }
}

/// One reason an alias mapping cannot be applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MappingProblem {
    /// No local alias was chosen for this imported pair. The UI should ask
    /// the user to pick a connection or create one (and its credential).
    Unmapped(RequiredConnection),
    /// The chosen local alias does not exist on this device.
    UnknownLocal {
        /// The imported pair.
        required: RequiredConnection,
        /// The alias that was chosen.
        local: ConnectionAlias,
    },
    /// The chosen local connection is for another provider.
    ProviderMismatch {
        /// The imported pair.
        required: RequiredConnection,
        /// The alias that was chosen.
        local: ConnectionAlias,
        /// That connection's provider.
        local_provider: ProviderKind,
    },
}

impl fmt::Display for MappingProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unmapped(required) => write!(
                f,
                "choose a local {} connection for {:?}",
                required.provider,
                required.alias.as_str()
            ),
            Self::UnknownLocal { required, local } => write!(
                f,
                "{:?} (for {:?}) is not a connection on this device",
                local.as_str(),
                required.alias.as_str()
            ),
            Self::ProviderMismatch {
                required,
                local,
                local_provider,
            } => write!(
                f,
                "{:?} is a {local_provider} connection, but {:?} needs {}",
                local.as_str(),
                required.alias.as_str(),
                required.provider
            ),
        }
    }
}

/// Why an import could not be applied. The current profile is unaffected.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ImportError {
    /// The alias mapping is incomplete or wrong. Lists every problem.
    #[error("{}", list(.0))]
    Mapping(Vec<MappingProblem>),
    /// The mapped profile failed validation.
    #[error("{0}")]
    Invalid(#[from] ValidationErrors),
}

fn list(problems: &[MappingProblem]) -> String {
    problems
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n")
}

/// A validated incoming profile, compared with the current one, waiting for
/// the recipient to map aliases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportPreview {
    incoming: Profile,
    diff: ProfileDiff,
    required: Vec<RequiredConnection>,
}

impl ImportPreview {
    /// Parses and validates `text` and compares it with `current`. Fails
    /// with every problem in the file; `current` is never modified.
    pub fn from_toml(current: &Profile, text: &str) -> Result<Self, DocumentError> {
        Ok(Self::new(current, parse_profile(text)?))
    }

    /// Previews replacing `current` with an already validated profile.
    pub fn new(current: &Profile, incoming: Profile) -> Self {
        let diff = ProfileDiff::between(current, &incoming);
        let mut required: Vec<RequiredConnection> = Vec::new();
        for watch in incoming.watches() {
            let provider = watch.target.provider();
            match required
                .iter_mut()
                .find(|r| r.alias == watch.connection && r.provider == provider)
            {
                Some(existing) => existing.watch_count += 1,
                None => required.push(RequiredConnection {
                    alias: watch.connection.clone(),
                    provider,
                    watch_count: 1,
                }),
            }
        }
        Self {
            incoming,
            diff,
            required,
        }
    }

    /// The imported profile exactly as written (aliases not yet mapped).
    pub fn incoming(&self) -> &Profile {
        &self.incoming
    }

    /// Differences from the current profile, with aliases as written by the
    /// sender. After mapping, `ProfileDiff::between(current, &applied)`
    /// gives the exact final change.
    pub fn diff(&self) -> &ProfileDiff {
        &self.diff
    }

    /// Distinct `(alias, provider)` pairs the import uses, in order of first
    /// use.
    pub fn required_connections(&self) -> &[RequiredConnection] {
        &self.required
    }

    /// A starting mapping: an imported alias maps to the local connection of
    /// the same name and provider, or else to the only local connection for
    /// that provider. Anything ambiguous or missing is left unmapped for the
    /// user to decide.
    pub fn suggested_mapping(&self, local: &LocalConnections) -> AliasMapping {
        let mut mapping = AliasMapping::new();
        for required in &self.required {
            let same_name = local
                .get(&required.alias)
                .filter(|connection| connection.provider == required.provider)
                .map(|_| required.alias.clone());
            let only_one = || {
                let mut candidates = local
                    .iter()
                    .filter(|(_, connection)| connection.provider == required.provider);
                match (candidates.next(), candidates.next()) {
                    (Some((alias, _)), None) => Some(alias.clone()),
                    _ => None,
                }
            };
            if let Some(chosen) = same_name.or_else(only_one) {
                mapping.insert(required.alias.clone(), required.provider, chosen);
            }
        }
        mapping
    }

    /// Checks `mapping` against the required pairs and the local
    /// connections, and returns the profile to save. Every problem is
    /// reported at once.
    pub fn apply(
        &self,
        mapping: &AliasMapping,
        local: &LocalConnections,
    ) -> Result<Profile, ImportError> {
        let mut problems = Vec::new();
        for required in &self.required {
            match mapping.get(&required.alias, required.provider) {
                None => problems.push(MappingProblem::Unmapped(required.clone())),
                Some(chosen) => match local.get(chosen) {
                    None => problems.push(MappingProblem::UnknownLocal {
                        required: required.clone(),
                        local: chosen.clone(),
                    }),
                    Some(connection) if connection.provider != required.provider => {
                        problems.push(MappingProblem::ProviderMismatch {
                            required: required.clone(),
                            local: chosen.clone(),
                            local_provider: connection.provider,
                        });
                    }
                    Some(_) => {}
                },
            }
        }
        if !problems.is_empty() {
            return Err(ImportError::Mapping(problems));
        }
        let watches: Vec<Watch> = self
            .incoming
            .watches()
            .iter()
            .map(|watch| {
                let mut watch = watch.clone();
                if let Some(local) = mapping.get(&watch.connection, watch.target.provider()) {
                    watch.connection = local.clone();
                }
                watch
            })
            .collect();
        Ok(Profile::from_watches(&watches)?)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::format::profile_to_toml;
    use crate::validate::WatchDraft;

    const ACCOUNT: &str = "0123456789abcdef0123456789abcdef";

    fn gh(id: &str, label: &str, conn: &str) -> WatchDraft {
        WatchDraft::github(label, conn, "acme", id, "ci.yml", None).with_id(id)
    }

    fn cf(id: &str, conn: &str) -> WatchDraft {
        WatchDraft::cloudflare(id, conn, ACCOUNT, id, None).with_id(id)
    }

    fn profile(drafts: &[WatchDraft]) -> Profile {
        Profile::from_drafts(drafts).unwrap()
    }

    fn local(entries: &[(&str, ProviderKind)]) -> LocalConnections {
        entries
            .iter()
            .map(|(alias, provider)| {
                (
                    ConnectionAlias::new(*alias),
                    LocalConnection {
                        provider: *provider,
                        display_name: "me".into(),
                    },
                )
            })
            .collect()
    }

    fn alias(s: &str) -> ConnectionAlias {
        ConnectionAlias::new(s)
    }

    type Row = (String, Option<usize>, Option<usize>, Vec<WatchChange>);

    #[test]
    fn diff_classifies_every_watch() {
        let before = profile(&[
            gh("a", "A", "w"),
            gh("b", "B", "w"),
            gh("c", "C", "w"),
            cf("d", "c"),
        ]);
        let mut renamed = gh("b", "Bee", "home");
        renamed.branch = Some("main".into());
        let mut branch_only = gh("a", "A", "w");
        branch_only.branch = Some("dev".into());
        let mut retargeted = gh("c", "C", "w");
        retargeted.repo = Some("other".into());
        let after = profile(&[gh("new", "N", "w"), retargeted, renamed, branch_only]);
        let diff = ProfileDiff::between(&before, &after);

        let summary: Vec<Row> = diff
            .entries
            .iter()
            .map(|e| {
                let changes = match &e.kind {
                    DiffKind::Changed { changes, .. } => changes.clone(),
                    _ => vec![],
                };
                (e.id.to_string(), e.before_index, e.after_index, changes)
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                ("new".into(), None, Some(0), vec![]),
                ("c".into(), Some(2), Some(1), vec![WatchChange::Target]),
                (
                    "b".into(),
                    Some(1),
                    Some(2),
                    vec![
                        WatchChange::Label,
                        WatchChange::Connection,
                        WatchChange::Branch
                    ]
                ),
                ("a".into(), Some(0), Some(3), vec![WatchChange::Branch]),
                ("d".into(), Some(3), None, vec![]),
            ]
        );
        assert!(matches!(diff.entries[0].kind, DiffKind::Added(_)));
        assert!(matches!(diff.entries[4].kind, DiffKind::Removed(_)));
        assert!(diff.reordered);
        assert!(!diff.is_identity());
        assert_eq!(
            diff.counts(),
            DiffCounts {
                added: 1,
                removed: 1,
                changed: 3,
                unchanged: 0
            }
        );
    }

    #[test]
    fn insertion_alone_is_not_reorder() {
        let before = profile(&[gh("a", "A", "w"), gh("b", "B", "w")]);
        let after = profile(&[gh("x", "X", "w"), gh("a", "A", "w"), gh("b", "B", "w")]);
        let diff = ProfileDiff::between(&before, &after);
        assert!(!diff.reordered);
        assert_eq!(diff.counts().unchanged, 2);
        assert!(ProfileDiff::between(&before, &before).is_identity());
        let swapped = profile(&[gh("b", "B", "w"), gh("a", "A", "w")]);
        let diff = ProfileDiff::between(&before, &swapped);
        assert!(diff.reordered && !diff.is_identity());
        assert_eq!(diff.counts().unchanged, 2);
    }

    #[test]
    fn required_connections_are_distinct_pairs_in_order() {
        let incoming = profile(&[
            gh("a", "A", "team-gh"),
            cf("b", "team"),
            gh("c", "C", "team"),
            gh("d", "D", "team-gh"),
        ]);
        let preview = ImportPreview::new(&Profile::empty(), incoming.clone());
        let pairs: Vec<(&str, ProviderKind, usize)> = preview
            .required_connections()
            .iter()
            .map(|r| (r.alias.as_str(), r.provider, r.watch_count))
            .collect();
        assert_eq!(
            pairs,
            vec![
                ("team-gh", ProviderKind::GitHub, 2),
                ("team", ProviderKind::Cloudflare, 1),
                ("team", ProviderKind::GitHub, 1),
            ]
        );
        assert_eq!(preview.incoming(), &incoming);
        assert_eq!(preview.diff().counts().added, 4);
    }

    #[test]
    fn suggested_mapping_prefers_same_name_then_single_candidate() {
        let incoming = profile(&[
            gh("a", "A", "work"),
            cf("b", "cf"),
            gh("c", "C", "personal"),
        ]);
        let preview = ImportPreview::new(&Profile::empty(), incoming);
        let only_one_gh = local(&[
            ("work", ProviderKind::GitHub),
            ("my-cf", ProviderKind::Cloudflare),
        ]);
        let mapping = preview.suggested_mapping(&only_one_gh);
        assert_eq!(
            mapping.get(&alias("work"), ProviderKind::GitHub),
            Some(&alias("work"))
        );
        assert_eq!(
            mapping.get(&alias("personal"), ProviderKind::GitHub),
            Some(&alias("work"))
        );
        assert_eq!(
            mapping.get(&alias("cf"), ProviderKind::Cloudflare),
            Some(&alias("my-cf"))
        );

        let two_gh = local(&[("x", ProviderKind::GitHub), ("y", ProviderKind::GitHub)]);
        let mapping = preview.suggested_mapping(&two_gh);
        assert_eq!(mapping, AliasMapping::new(), "ambiguous: nothing suggested");
        let err = preview.apply(&mapping, &two_gh).unwrap_err();
        assert_eq!(
            err.to_string(),
            "choose a local GitHub connection for \"work\"\n\
             choose a local Cloudflare connection for \"cf\"\n\
             choose a local GitHub connection for \"personal\""
        );
    }

    #[test]
    fn apply_reports_every_mapping_problem() {
        let incoming = profile(&[
            gh("a", "A", "work"),
            cf("b", "cf"),
            gh("c", "C", "personal"),
        ]);
        let preview = ImportPreview::new(&Profile::empty(), incoming);
        let locals = local(&[
            ("mine", ProviderKind::GitHub),
            ("mine-cf", ProviderKind::Cloudflare),
        ]);
        let mut mapping = AliasMapping::new();
        mapping.insert(alias("work"), ProviderKind::GitHub, alias("mine-cf"));
        mapping.insert(alias("cf"), ProviderKind::Cloudflare, alias("ghost"));
        mapping.insert(alias("personal"), ProviderKind::GitHub, alias("mine"));
        mapping.remove(&alias("personal"), ProviderKind::GitHub);
        let ImportError::Mapping(problems) = preview.apply(&mapping, &locals).unwrap_err() else {
            panic!()
        };
        assert_eq!(problems.len(), 3);
        assert!(matches!(
            problems[0],
            MappingProblem::ProviderMismatch { .. }
        ));
        assert!(matches!(problems[1], MappingProblem::UnknownLocal { .. }));
        assert!(matches!(problems[2], MappingProblem::Unmapped(_)));
        assert_eq!(
            problems[0].to_string(),
            "\"mine-cf\" is a Cloudflare connection, but \"work\" needs GitHub"
        );
        assert_eq!(
            problems[1].to_string(),
            "\"ghost\" (for \"cf\") is not a connection on this device"
        );
    }

    #[test]
    fn export_import_round_trip_preserves_targets_and_order() {
        // Sender's machine.
        let sender = profile(&[
            cf("site", "acme-cf"),
            gh("api", "API", "work-github"),
            gh("web", "Web", "work-github"),
        ]);
        let exported = profile_to_toml(&sender);

        // Recipient's machine, with an unrelated existing profile.
        let current = profile(&[gh("old", "Old", "me")]);
        let preview = ImportPreview::from_toml(&current, &exported).unwrap();
        assert_eq!(
            preview.diff().counts(),
            DiffCounts {
                added: 3,
                removed: 1,
                changed: 0,
                unchanged: 0
            }
        );
        let locals = local(&[
            ("me", ProviderKind::GitHub),
            ("cf-me", ProviderKind::Cloudflare),
        ]);
        let mapping = preview.suggested_mapping(&locals);
        let applied = preview.apply(&mapping, &locals).unwrap();

        let ids = |p: &Profile| p.watches().iter().map(|w| w.id.clone()).collect::<Vec<_>>();
        let targets = |p: &Profile| {
            p.watches()
                .iter()
                .map(|w| w.target.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&applied), ids(&sender));
        assert_eq!(targets(&applied), targets(&sender));
        let aliases: Vec<&str> = applied
            .watches()
            .iter()
            .map(|w| w.connection.as_str())
            .collect();
        assert_eq!(aliases, ["cf-me", "me", "me"]);

        // Re-importing the same file over the result only changes aliases back.
        let again = ImportPreview::from_toml(&applied, &exported).unwrap();
        assert!(!again.diff().reordered);
        assert_eq!(again.diff().counts().changed, 3);
        let final_diff = ProfileDiff::between(&applied, &again.apply(&mapping, &locals).unwrap());
        assert!(final_diff.is_identity());
    }

    #[test]
    fn malformed_import_is_an_error_before_anything_else() {
        let current = profile(&[gh("a", "A", "w")]);
        assert!(ImportPreview::from_toml(&current, "version = 1\n[[watch]]\nid = 'x'").is_err());
        assert!(ImportPreview::from_toml(&current, "not toml at all").is_err());
        assert_eq!(current, profile(&[gh("a", "A", "w")]));
    }

    #[test]
    fn mapping_to_an_invalid_local_alias_is_rejected() {
        let preview = ImportPreview::new(&Profile::empty(), profile(&[gh("a", "A", "w")]));
        let locals = local(&[("bad alias", ProviderKind::GitHub)]);
        let mapping = preview.suggested_mapping(&locals);
        let err = preview.apply(&mapping, &locals).unwrap_err();
        assert!(matches!(err, ImportError::Invalid(_)), "{err}");
    }
}
