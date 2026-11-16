//! The ordered, always-valid list of watches, and the edits Settings makes
//! to it.

use buildrail_core::domain::{ConnectionAlias, Watch, WatchId};
use thiserror::Error;

use crate::validate::{validate_drafts, ValidationErrors, WatchDraft};

/// The user's pinned watches, in strip order.
///
/// A `Profile` can only be built through validation, so holding one means
/// every watch is valid and every ID is unique. Edits are pure: each returns
/// a *new* profile (or an error) and leaves `self` untouched, which makes
/// "preview, then commit" and undo trivial for the UI.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Profile {
    watches: Vec<Watch>,
}

/// Why an edit was refused.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum EditError {
    /// No watch has this ID.
    #[error("no watch with id {0:?}")]
    UnknownWatch(WatchId),
    /// A position outside `0..len`.
    #[error("position {index} is out of range (the profile has {len} watches)")]
    OutOfRange {
        /// The position asked for.
        index: usize,
        /// Number of watches.
        len: usize,
    },
    /// The edited profile would not be valid.
    #[error("{0}")]
    Invalid(#[from] ValidationErrors),
}

impl Profile {
    /// A profile with no watches (first launch).
    pub fn empty() -> Self {
        Self::default()
    }

    /// Validates drafts (from TOML or the picker) into a profile.
    pub fn from_drafts(drafts: &[WatchDraft]) -> Result<Self, ValidationErrors> {
        validate_drafts(drafts).map(|watches| Self { watches })
    }

    /// Re-validates already-typed watches into a profile, e.g. after alias
    /// mapping during import.
    pub fn from_watches(watches: &[Watch]) -> Result<Self, ValidationErrors> {
        Self::from_drafts(&watches.iter().map(WatchDraft::from).collect::<Vec<_>>())
    }

    /// Watches in strip order.
    pub fn watches(&self) -> &[Watch] {
        &self.watches
    }

    /// Number of watches.
    pub fn len(&self) -> usize {
        self.watches.len()
    }

    /// `true` when nothing is pinned.
    pub fn is_empty(&self) -> bool {
        self.watches.is_empty()
    }

    /// The watch with this ID.
    pub fn get(&self, id: &WatchId) -> Option<&Watch> {
        self.watches.iter().find(|watch| &watch.id == id)
    }

    /// Position of the watch with this ID.
    pub fn position(&self, id: &WatchId) -> Option<usize> {
        self.watches.iter().position(|watch| &watch.id == id)
    }

    /// Appends a new watch (from the picker) at the end. A draft without an
    /// ID gets a fresh one. Returns the new profile and the added watch's ID.
    pub fn with_added(&self, draft: WatchDraft) -> Result<(Self, WatchId), EditError> {
        let mut drafts = self.drafts();
        drafts.push(draft);
        let profile = Self::from_drafts(&drafts)?;
        let id = profile
            .watches
            .last()
            .map(|watch| watch.id.clone())
            .ok_or(EditError::OutOfRange { index: 0, len: 0 })?;
        Ok((profile, id))
    }

    /// Changes a watch's label.
    pub fn with_label(&self, id: &WatchId, label: &str) -> Result<Self, EditError> {
        self.edit(id, |draft| draft.label = Some(label.to_owned()))
    }

    /// Points a watch at a different local connection ("Reconnect").
    pub fn with_connection(
        &self,
        id: &WatchId,
        connection: &ConnectionAlias,
    ) -> Result<Self, EditError> {
        self.edit(id, |draft| {
            draft.connection = Some(connection.as_str().to_owned());
        })
    }

    /// Replaces a watch's contents with `draft` while keeping its ID and
    /// position (editing a watch in the picker). Any ID in `draft` is
    /// ignored.
    pub fn with_replaced(&self, id: &WatchId, draft: WatchDraft) -> Result<Self, EditError> {
        self.edit(id, |slot| *slot = draft.with_id(id.as_str()))
    }

    /// Moves the watch at `from` so that it ends up at position `to`; the
    /// others keep their relative order.
    pub fn with_moved(&self, from: usize, to: usize) -> Result<Self, EditError> {
        let len = self.len();
        for index in [from, to] {
            if index >= len {
                return Err(EditError::OutOfRange { index, len });
            }
        }
        let mut watches = self.watches.clone();
        let watch = watches.remove(from);
        watches.insert(to, watch);
        Ok(Self::from_watches(&watches)?)
    }

    /// Removes a watch.
    pub fn without(&self, id: &WatchId) -> Result<Self, EditError> {
        let index = self.index_of(id)?;
        let mut watches = self.watches.clone();
        watches.remove(index);
        Ok(Self::from_watches(&watches)?)
    }

    fn index_of(&self, id: &WatchId) -> Result<usize, EditError> {
        self.position(id)
            .ok_or_else(|| EditError::UnknownWatch(id.clone()))
    }

    fn drafts(&self) -> Vec<WatchDraft> {
        self.watches.iter().map(WatchDraft::from).collect()
    }

    fn edit(&self, id: &WatchId, change: impl FnOnce(&mut WatchDraft)) -> Result<Self, EditError> {
        let index = self.index_of(id)?;
        let mut drafts = self.drafts();
        if let Some(draft) = drafts.get_mut(index) {
            change(draft);
        }
        Ok(Self::from_drafts(&drafts)?)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::validate::Field;

    fn draft(label: &str) -> WatchDraft {
        WatchDraft::github(label, "work", "acme", "api", "ci.yml", None)
    }

    fn three() -> Profile {
        Profile::from_drafts(&[
            draft("a").with_id("a"),
            draft("b").with_id("b"),
            draft("c").with_id("c"),
        ])
        .unwrap()
    }

    fn ids(profile: &Profile) -> Vec<&str> {
        profile.watches().iter().map(|w| w.id.as_str()).collect()
    }

    fn id(s: &str) -> WatchId {
        WatchId::new(s)
    }

    #[test]
    fn empty_profile() {
        let profile = Profile::empty();
        assert!(profile.is_empty());
        assert_eq!(profile.len(), 0);
        assert_eq!(profile, Profile::from_drafts(&[]).unwrap());
    }

    #[test]
    fn add_appends_with_new_id() {
        let (profile, new_id) = three().with_added(draft("d")).unwrap();
        assert_eq!(profile.len(), 4);
        assert_eq!(profile.position(&new_id), Some(3));
        assert_eq!(profile.get(&new_id).unwrap().label, "d");
    }

    #[test]
    fn add_rejects_duplicate_or_invalid() {
        let err = three().with_added(draft("x").with_id("b")).unwrap_err();
        assert!(matches!(err, EditError::Invalid(_)));
        assert!(err.to_string().contains("already used by watch 2"));
        let err = three().with_added(draft("")).unwrap_err();
        assert!(err.to_string().contains("label must not be empty"));
    }

    #[test]
    fn rename_keeps_everything_else() {
        let before = three();
        let after = before.with_label(&id("b"), "  Bee ").unwrap();
        assert_eq!(after.get(&id("b")).unwrap().label, "Bee");
        assert_eq!(ids(&after), ids(&before));
        assert_eq!(
            before.get(&id("b")).unwrap().label,
            "b",
            "original untouched"
        );
        let err = before.with_label(&id("b"), "").unwrap_err();
        let EditError::Invalid(errors) = err else {
            panic!()
        };
        assert_eq!(errors.issues()[0].field, Field::Label);
        assert!(matches!(
            before.with_label(&id("zz"), "x"),
            Err(EditError::UnknownWatch(_))
        ));
    }

    #[test]
    fn reorder() {
        let profile = three();
        assert_eq!(ids(&profile.with_moved(0, 2).unwrap()), ["b", "c", "a"]);
        assert_eq!(ids(&profile.with_moved(2, 0).unwrap()), ["c", "a", "b"]);
        assert_eq!(ids(&profile.with_moved(1, 1).unwrap()), ["a", "b", "c"]);
        assert_eq!(
            profile.with_moved(3, 0),
            Err(EditError::OutOfRange { index: 3, len: 3 })
        );
        assert_eq!(
            profile.with_moved(0, 5).unwrap_err().to_string(),
            "position 5 is out of range (the profile has 3 watches)"
        );
    }

    #[test]
    fn remove() {
        let profile = three().without(&id("b")).unwrap();
        assert_eq!(ids(&profile), ["a", "c"]);
        assert_eq!(
            profile.without(&id("b")).unwrap_err().to_string(),
            "no watch with id WatchId(\"b\")"
        );
    }

    #[test]
    fn reconnect_and_replace() {
        let profile = three()
            .with_connection(&id("a"), &ConnectionAlias::new("home"))
            .unwrap();
        assert_eq!(profile.get(&id("a")).unwrap().connection.as_str(), "home");
        assert!(three()
            .with_connection(&id("a"), &ConnectionAlias::new("bad alias"))
            .is_err());

        let replacement = WatchDraft::cloudflare(
            "Site",
            "cf",
            "0123456789abcdef0123456789abcdef",
            "site",
            None,
        )
        .with_id("ignored");
        let profile = three().with_replaced(&id("c"), replacement).unwrap();
        assert_eq!(ids(&profile), ["a", "b", "c"]);
        assert_eq!(profile.get(&id("c")).unwrap().label, "Site");
    }

    #[test]
    fn from_watches_revalidates() {
        let mut watches = three().watches().to_vec();
        watches[1].id = WatchId::new("a");
        assert!(Profile::from_watches(&watches).is_err());
    }
}
