//! The "Pinned builds" list in Settings: rename, reorder, remove,
//! reconnect, and removing a connection.
//!
//! Edits are checked against the profile's own rules *before* a command is
//! sent (the same pure `Profile::with_*` methods the controller applies),
//! so the user sees "label must not be empty" immediately; the controller
//! then saves atomically and reports back with [`Event::Saved`].
//!
//! Removing a connection that watches still use is refused with the names
//! of those watches, so a profile can never point at a connection that no
//! longer exists on this device.

use buildrail_core::domain::{ConnectionAlias, WatchId};

use super::connect::{ConnectFlow, ConnectInput};
use super::{watches_using, Command, Event, ProfileEdit, SettingsContext, Ticket};

/// User input for the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManageInput {
    /// Start renaming a watch.
    StartRename(WatchId),
    /// The rename field changed.
    SetRename(String),
    /// Save the new name.
    CommitRename,
    /// Abandon the rename.
    CancelRename,
    /// Move a watch one place left (earlier on the strip).
    MoveUp(WatchId),
    /// Move a watch one place right.
    MoveDown(WatchId),
    /// Ask to remove a watch (needs confirmation).
    Remove(WatchId),
    /// Confirm the pending removal.
    ConfirmRemove,
    /// Keep the watch.
    CancelRemove,
    /// Start reconnecting a watch's connection with a new token.
    StartReconnect(WatchId),
    /// Input for the reconnect form.
    Reconnect(ConnectInput),
    /// Point the watch at another existing connection instead.
    UseConnection(WatchId, ConnectionAlias),
    /// Close the reconnect form.
    CancelReconnect,
    /// Delete a connection from this device.
    RemoveConnection(ConnectionAlias),
}

/// State of the list's inline editors. See the module docs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ManageList {
    renaming: Option<(WatchId, String)>,
    confirm_remove: Option<WatchId>,
    reconnecting: Option<(WatchId, ConnectFlow)>,
    message: Option<String>,
    waiting: Option<Ticket>,
}

impl ManageList {
    /// An idle list.
    pub fn new() -> Self {
        Self::default()
    }

    /// The watch being renamed and the text typed so far.
    pub fn renaming(&self) -> Option<(&WatchId, &str)> {
        self.renaming.as_ref().map(|(id, text)| (id, text.as_str()))
    }

    /// The watch awaiting removal confirmation.
    pub fn confirm_remove(&self) -> Option<&WatchId> {
        self.confirm_remove.as_ref()
    }

    /// The watch being reconnected and its connect form.
    pub fn reconnecting(&self) -> Option<(&WatchId, &ConnectFlow)> {
        self.reconnecting.as_ref().map(|(id, flow)| (id, flow))
    }

    /// The last error or confirmation message.
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// `true` while a save is in progress.
    pub fn is_busy(&self) -> bool {
        self.waiting.is_some()
    }

    /// Handles input; returns the commands to execute.
    pub fn input(&mut self, input: ManageInput, ctx: SettingsContext<'_>) -> Vec<Command> {
        match input {
            ManageInput::StartRename(id) => {
                if let Some(watch) = ctx.profile.get(&id) {
                    self.renaming = Some((id, watch.label.clone()));
                    self.message = None;
                }
                Vec::new()
            }
            ManageInput::SetRename(text) => {
                if let Some((_, current)) = self.renaming.as_mut() {
                    *current = text;
                }
                Vec::new()
            }
            ManageInput::CommitRename => {
                let Some((id, label)) = self.renaming.clone() else {
                    return Vec::new();
                };
                match ctx.profile.with_label(&id, &label) {
                    Ok(_) => {
                        self.renaming = None;
                        self.edit(ProfileEdit::Rename { id, label })
                    }
                    Err(error) => {
                        self.message = Some(capitalise(&error.to_string()));
                        Vec::new()
                    }
                }
            }
            ManageInput::CancelRename => {
                self.renaming = None;
                Vec::new()
            }
            ManageInput::MoveUp(ref id) | ManageInput::MoveDown(ref id) => {
                let up = matches!(input, ManageInput::MoveUp(_));
                let Some(from) = ctx.profile.position(id) else {
                    return Vec::new();
                };
                let to = if up {
                    from.checked_sub(1)
                } else {
                    Some(from + 1).filter(|to| *to < ctx.profile.len())
                };
                match to {
                    Some(to) => self.edit(ProfileEdit::Move { from, to }),
                    None => Vec::new(),
                }
            }
            ManageInput::Remove(id) => {
                if ctx.profile.get(&id).is_some() {
                    self.confirm_remove = Some(id);
                }
                Vec::new()
            }
            ManageInput::ConfirmRemove => match self.confirm_remove.take() {
                Some(id) => self.edit(ProfileEdit::Remove { id }),
                None => Vec::new(),
            },
            ManageInput::CancelRemove => {
                self.confirm_remove = None;
                Vec::new()
            }
            ManageInput::StartReconnect(id) => {
                if let Some(watch) = ctx.profile.get(&id) {
                    let flow = ConnectFlow::reconnect(watch.target.provider(), &watch.connection);
                    self.reconnecting = Some((id, flow));
                    self.message = None;
                }
                Vec::new()
            }
            ManageInput::Reconnect(input) => match self.reconnecting.as_mut() {
                Some((_, flow)) => flow.input(input, ctx.connections),
                None => Vec::new(),
            },
            ManageInput::UseConnection(id, connection) => {
                let fits = match (ctx.profile.get(&id), ctx.connections.get(&connection)) {
                    (Some(watch), Some(local)) => local.provider == watch.target.provider(),
                    _ => false,
                };
                if !fits {
                    self.message = Some(format!(
                        "\"{connection}\" is not a matching connection on this device."
                    ));
                    return Vec::new();
                }
                self.reconnecting = None;
                self.edit(ProfileEdit::SetConnection { id, connection })
            }
            ManageInput::CancelReconnect => {
                self.reconnecting = None;
                Vec::new()
            }
            ManageInput::RemoveConnection(alias) => {
                let users = watches_using(ctx.profile, &alias);
                if !users.is_empty() {
                    self.message = Some(format!(
                        "\"{alias}\" is still used by {}. Remove those builds or switch them to another connection first.",
                        users.join(", ")
                    ));
                    return Vec::new();
                }
                let ticket = Ticket::next();
                self.waiting = Some(ticket);
                self.message = None;
                vec![Command::RemoveConnection { ticket, alias }]
            }
        }
    }

    /// Handles a controller event.
    pub fn on_event(&mut self, event: &Event) {
        if let Some((_, flow)) = self.reconnecting.as_mut() {
            if let Some(connection) = flow.on_event(event) {
                self.reconnecting = None;
                self.message = Some(format!(
                    "Reconnected \"{}\". Refreshing now.",
                    connection.alias
                ));
                return;
            }
        }
        if let Event::Saved { ticket, result } = event {
            if self.waiting == Some(*ticket) {
                self.waiting = None;
                self.message = result.as_ref().err().cloned();
            }
        }
    }

    fn edit(&mut self, edit: ProfileEdit) -> Vec<Command> {
        let ticket = Ticket::next();
        self.waiting = Some(ticket);
        self.message = None;
        vec![Command::Edit { ticket, edit }]
    }
}

fn capitalise(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use buildrail_core::domain::ProviderKind;
    use buildrail_profile::{LocalConnections, Profile};

    use super::*;
    use crate::settings::connect::NewConnection;
    use crate::settings::test_support::{connections, profile};

    struct F {
        profile: Profile,
        conns: LocalConnections,
        list: ManageList,
    }

    impl F {
        fn new() -> Self {
            Self {
                profile: profile(),
                conns: connections(&[
                    ("work", ProviderKind::GitHub),
                    ("home", ProviderKind::GitHub),
                    ("cf", ProviderKind::Cloudflare),
                    ("spare", ProviderKind::Cloudflare),
                ]),
                list: ManageList::new(),
            }
        }
        fn input(&mut self, input: ManageInput) -> Vec<Command> {
            let ctx = SettingsContext {
                profile: &self.profile,
                connections: &self.conns,
            };
            self.list.input(input, ctx)
        }
    }

    fn id(s: &str) -> WatchId {
        WatchId::new(s)
    }

    fn edit_of(commands: &[Command]) -> (Ticket, ProfileEdit) {
        match commands {
            [Command::Edit { ticket, edit }] => (*ticket, edit.clone()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rename_validates_then_saves() {
        let mut f = F::new();
        f.input(ManageInput::StartRename(id("zz")));
        assert!(f.list.renaming().is_none());
        f.input(ManageInput::SetRename("ignored".into()));
        assert!(f.input(ManageInput::CommitRename).is_empty());

        f.input(ManageInput::StartRename(id("k")));
        assert_eq!(f.list.renaming(), Some((&id("k"), "Keevoro")));
        f.input(ManageInput::SetRename("".into()));
        assert!(f.input(ManageInput::CommitRename).is_empty());
        assert!(f.list.message().unwrap().contains("abel must not be empty"));
        f.input(ManageInput::SetRename("Keevoro API".into()));
        let (ticket, edit) = edit_of(&f.input(ManageInput::CommitRename));
        assert_eq!(
            edit,
            ProfileEdit::Rename {
                id: id("k"),
                label: "Keevoro API".into()
            }
        );
        assert!(f.list.is_busy());
        f.list.on_event(&Event::Saved {
            ticket,
            result: Err("disk full".into()),
        });
        assert_eq!(f.list.message(), Some("disk full"));
        assert!(!f.list.is_busy());

        f.input(ManageInput::StartRename(id("k")));
        f.input(ManageInput::CancelRename);
        assert!(f.list.renaming().is_none());
    }

    #[test]
    fn reorder_within_bounds() {
        let mut f = F::new();
        assert!(f.input(ManageInput::MoveUp(id("k"))).is_empty());
        assert!(f.input(ManageInput::MoveDown(id("s"))).is_empty());
        assert!(f.input(ManageInput::MoveDown(id("zz"))).is_empty());
        let (_, edit) = edit_of(&f.input(ManageInput::MoveDown(id("k"))));
        assert_eq!(edit, ProfileEdit::Move { from: 0, to: 1 });
        let (ticket, edit) = edit_of(&f.input(ManageInput::MoveUp(id("s"))));
        assert_eq!(edit, ProfileEdit::Move { from: 1, to: 0 });
        f.list.on_event(&Event::Saved {
            ticket: Ticket::next(),
            result: Err("other".into()),
        });
        assert!(f.list.is_busy(), "other tickets ignored");
        f.list.on_event(&Event::Saved {
            ticket,
            result: Ok(()),
        });
        assert_eq!(f.list.message(), None);
    }

    #[test]
    fn remove_needs_confirmation() {
        let mut f = F::new();
        assert!(f.input(ManageInput::ConfirmRemove).is_empty());
        f.input(ManageInput::Remove(id("zz")));
        assert!(f.list.confirm_remove().is_none());
        f.input(ManageInput::Remove(id("s")));
        assert_eq!(f.list.confirm_remove(), Some(&id("s")));
        f.input(ManageInput::CancelRemove);
        assert!(f.list.confirm_remove().is_none());
        f.input(ManageInput::Remove(id("s")));
        let (_, edit) = edit_of(&f.input(ManageInput::ConfirmRemove));
        assert_eq!(edit, ProfileEdit::Remove { id: id("s") });
    }

    #[test]
    fn reconnect_with_token_or_other_connection() {
        let mut f = F::new();
        assert!(f
            .input(ManageInput::Reconnect(ConnectInput::Submit))
            .is_empty());
        f.input(ManageInput::StartReconnect(id("zz")));
        assert!(f.list.reconnecting().is_none());
        f.input(ManageInput::StartReconnect(id("k")));
        let (_, flow) = f.list.reconnecting().unwrap();
        assert_eq!(flow.provider(), ProviderKind::GitHub);
        f.input(ManageInput::Reconnect(ConnectInput::SetToken("t".into())));
        let commands = f.input(ManageInput::Reconnect(ConnectInput::Submit));
        let [Command::Connect { ticket, request }] = commands.as_slice() else {
            panic!()
        };
        assert!(request.replace);
        assert_eq!(request.alias.as_str(), "work");
        f.list.on_event(&Event::Connected {
            ticket: *ticket,
            result: Ok(NewConnection {
                alias: request.alias.clone(),
                provider: ProviderKind::GitHub,
                display_name: "octocat".into(),
            }),
        });
        assert!(f.list.reconnecting().is_none());
        assert!(f
            .list
            .message()
            .unwrap()
            .starts_with("Reconnected \"work\""));

        // Switch to another connection: provider must match.
        f.input(ManageInput::StartReconnect(id("k")));
        assert!(f
            .input(ManageInput::UseConnection(
                id("k"),
                ConnectionAlias::new("cf")
            ))
            .is_empty());
        assert!(f.list.message().unwrap().contains("not a matching"));
        let (_, edit) = edit_of(&f.input(ManageInput::UseConnection(
            id("k"),
            ConnectionAlias::new("home"),
        )));
        assert_eq!(
            edit,
            ProfileEdit::SetConnection {
                id: id("k"),
                connection: ConnectionAlias::new("home")
            }
        );
        assert!(f.list.reconnecting().is_none());
        f.input(ManageInput::StartReconnect(id("s")));
        f.input(ManageInput::CancelReconnect);
        assert!(f.list.reconnecting().is_none());
    }

    #[test]
    fn connection_in_use_cannot_be_removed() {
        let mut f = F::new();
        assert!(f
            .input(ManageInput::RemoveConnection(ConnectionAlias::new("work")))
            .is_empty());
        assert!(f.list.message().unwrap().contains("still used by Keevoro"));
        let commands = f.input(ManageInput::RemoveConnection(ConnectionAlias::new("spare")));
        let [Command::RemoveConnection { ticket, alias }] = commands.as_slice() else {
            panic!()
        };
        assert_eq!(alias.as_str(), "spare");
        f.list.on_event(&Event::Saved {
            ticket: *ticket,
            result: Ok(()),
        });
        assert!(!f.list.is_busy());
        assert_eq!(capitalise(""), "");
    }
}
