//! Importing a teammate's profile, and exporting ours.
//!
//! ```text
//! Input (paste / load file) ──Load──▶ Mapping ──Continue──▶ Review ──Replace──▶ Done
//!     ▲ parse error shown here           │ map each (alias, provider)
//!     │                                  │ to a local connection, or
//!     └──────────── Back ◀───────────────┘ create one (ConnectFlow)
//! ```
//!
//! The pure work is done by `buildrail_profile`: [`ImportPreview`] parses
//! and validates, suggests a mapping and applies it. This machine only
//! sequences the steps. The profile file is touched once, by the
//! controller's atomic save after the final confirmation
//! ([`Command::ReplaceProfile`]); a malformed file or an incomplete mapping
//! never gets that far, so the working configuration stays intact.
//!
//! Export is [`export_text`]: the portable TOML (aliases, never secrets).

use buildrail_core::domain::{ConnectionAlias, ProviderKind};
use buildrail_profile::{
    profile_to_toml, AliasMapping, ImportError, ImportPreview, Profile, ProfileDiff,
    RequiredConnection,
};

use super::connect::{ConnectFlow, ConnectInput};
use super::{Command, Event, SettingsContext, Ticket};

/// The portable profile text to give a teammate. Contains connection
/// aliases only; credentials live in the OS store and cannot appear here.
pub fn export_text(profile: &Profile) -> String {
    profile_to_toml(profile)
}

/// Where the import is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportStep {
    /// Paste or load the file.
    Input,
    /// Map the file's connections to this device's.
    Mapping,
    /// Final diff; confirm to replace.
    Review,
    /// Replaced.
    Done,
}

/// User input for the import flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportInput {
    /// The pasted text changed.
    SetText(String),
    /// Parse and validate the text.
    Load,
    /// Map the required connection at `index` to a local alias (`None`
    /// clears the choice).
    Map {
        /// Index into [`ImportFlow::required`].
        index: usize,
        /// Chosen local alias.
        local: Option<ConnectionAlias>,
    },
    /// Create a new local connection for the required connection at
    /// `index`.
    CreateConnection(usize),
    /// Input for that connection's form.
    Connect(ConnectInput),
    /// Close the connection form.
    CancelCreate,
    /// Apply the mapping and show the final diff.
    Continue,
    /// Replace the profile.
    Confirm,
    /// Previous step.
    Back,
}

/// The import flow. See the module docs.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportFlow {
    step: ImportStep,
    text: String,
    error: Option<String>,
    preview: Option<ImportPreview>,
    mapping: AliasMapping,
    creating: Option<(usize, ConnectFlow)>,
    applied: Option<(Profile, ProfileDiff)>,
    waiting: Option<Ticket>,
}

impl Default for ImportFlow {
    fn default() -> Self {
        Self::new()
    }
}

impl ImportFlow {
    /// An empty import.
    pub fn new() -> Self {
        Self {
            step: ImportStep::Input,
            text: String::new(),
            error: None,
            preview: None,
            mapping: AliasMapping::new(),
            creating: None,
            applied: None,
            waiting: None,
        }
    }

    /// The current step.
    pub fn step(&self) -> ImportStep {
        self.step
    }
    /// The pasted text.
    pub fn text(&self) -> &str {
        &self.text
    }
    /// The last problem (parse errors, mapping problems, save failure).
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    /// The validated incoming profile and its diff (from `Mapping` on).
    pub fn preview(&self) -> Option<&ImportPreview> {
        self.preview.as_ref()
    }
    /// The connections the file needs, in order of first use.
    pub fn required(&self) -> &[RequiredConnection] {
        self.preview
            .as_ref()
            .map_or(&[], ImportPreview::required_connections)
    }
    /// The local alias chosen for a required connection.
    pub fn mapped(&self, required: &RequiredConnection) -> Option<&ConnectionAlias> {
        self.mapping.get(&required.alias, required.provider)
    }
    /// The connection being created, and for which required index.
    pub fn creating(&self) -> Option<(usize, &ConnectFlow)> {
        self.creating.as_ref().map(|(i, flow)| (*i, flow))
    }
    /// The profile that will be saved and its exact diff, at `Review`.
    pub fn applied(&self) -> Option<(&Profile, &ProfileDiff)> {
        self.applied.as_ref().map(|(p, d)| (p, d))
    }
    /// `true` while the replacement is being saved.
    pub fn is_saving(&self) -> bool {
        self.waiting.is_some()
    }

    /// Handles input; returns the commands to execute.
    pub fn input(&mut self, input: ImportInput, ctx: SettingsContext<'_>) -> Vec<Command> {
        match (self.step, input) {
            (ImportStep::Input, ImportInput::SetText(text)) => {
                self.text = text;
                Vec::new()
            }
            (ImportStep::Input, ImportInput::Load) => {
                self.load(ctx);
                Vec::new()
            }
            (ImportStep::Mapping, ImportInput::Map { index, local }) => {
                if let Some(required) = self.required().get(index).cloned() {
                    match local {
                        Some(local) => {
                            self.mapping
                                .insert(required.alias, required.provider, local);
                        }
                        None => self.mapping.remove(&required.alias, required.provider),
                    }
                }
                Vec::new()
            }
            (ImportStep::Mapping, ImportInput::CreateConnection(index)) => {
                if let Some(required) = self.required().get(index) {
                    let flow = new_connection_for(required, ctx);
                    self.creating = Some((index, flow));
                }
                Vec::new()
            }
            (ImportStep::Mapping, ImportInput::Connect(input)) => match self.creating.as_mut() {
                Some((_, flow)) => flow.input(input, ctx.connections),
                None => Vec::new(),
            },
            (ImportStep::Mapping, ImportInput::CancelCreate) => {
                self.creating = None;
                Vec::new()
            }
            (ImportStep::Mapping, ImportInput::Continue) => {
                self.apply(ctx);
                Vec::new()
            }
            (ImportStep::Review, ImportInput::Confirm) if self.waiting.is_none() => {
                let Some((profile, _)) = &self.applied else {
                    return Vec::new();
                };
                let ticket = Ticket::next();
                self.waiting = Some(ticket);
                self.error = None;
                vec![Command::ReplaceProfile {
                    ticket,
                    profile: profile.clone(),
                }]
            }
            (_, ImportInput::Back) => {
                self.back();
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// Handles a controller event.
    pub fn on_event(&mut self, event: &Event) {
        if let Some((index, flow)) = self.creating.as_mut() {
            if let Some(connection) = flow.on_event(event) {
                let index = *index;
                self.creating = None;
                if let Some(required) = self.required().get(index).cloned() {
                    self.mapping
                        .insert(required.alias, required.provider, connection.alias);
                }
                return;
            }
        }
        if let Event::Saved { ticket, result } = event {
            if self.waiting == Some(*ticket) {
                self.waiting = None;
                match result {
                    Ok(()) => {
                        self.step = ImportStep::Done;
                        self.error = None;
                    }
                    Err(message) => self.error = Some(message.clone()),
                }
            }
        }
    }

    fn load(&mut self, ctx: SettingsContext<'_>) {
        match ImportPreview::from_toml(ctx.profile, &self.text) {
            Ok(preview) => {
                self.mapping = preview.suggested_mapping(ctx.connections);
                self.preview = Some(preview);
                self.error = None;
                self.step = ImportStep::Mapping;
            }
            Err(error) => {
                self.error = Some(format!(
                    "This profile cannot be imported. Nothing was changed.\n{error}"
                ));
            }
        }
    }

    fn apply(&mut self, ctx: SettingsContext<'_>) {
        let Some(preview) = &self.preview else {
            return;
        };
        match preview.apply(&self.mapping, ctx.connections) {
            Ok(profile) => {
                let diff = ProfileDiff::between(ctx.profile, &profile);
                self.applied = Some((profile, diff));
                self.error = None;
                self.step = ImportStep::Review;
            }
            Err(ImportError::Mapping(problems)) => {
                let lines: Vec<String> = problems.iter().map(ToString::to_string).collect();
                self.error = Some(capitalise(&lines.join("\n")));
            }
            Err(ImportError::Invalid(errors)) => self.error = Some(errors.to_string()),
        }
    }

    fn back(&mut self) {
        self.error = None;
        self.creating = None;
        self.step = match self.step {
            ImportStep::Input | ImportStep::Done => return,
            ImportStep::Mapping => {
                self.preview = None;
                ImportStep::Input
            }
            ImportStep::Review => {
                self.applied = None;
                self.waiting = None;
                ImportStep::Mapping
            }
        };
    }
}

/// A connect form for a connection the import needs, defaulting to the
/// sender's alias when it is free here.
fn new_connection_for(required: &RequiredConnection, ctx: SettingsContext<'_>) -> ConnectFlow {
    if ctx.connections.contains_key(&required.alias) {
        ConnectFlow::new_connection(required.provider, ctx.connections)
    } else {
        ConnectFlow::with_alias(required.provider, required.alias.as_str(), false)
    }
}

/// Local connections usable for `provider`, for the mapping drop-downs.
pub fn choices_for(
    provider: ProviderKind,
    ctx: SettingsContext<'_>,
) -> Vec<(ConnectionAlias, String)> {
    ctx.connections
        .iter()
        .filter(|(_, c)| c.provider == provider)
        .map(|(alias, c)| (alias.clone(), c.display_name.clone()))
        .collect()
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

    use buildrail_profile::{DiffKind, LocalConnections, WatchDraft};

    use super::*;
    use crate::settings::connect::NewConnection;
    use crate::settings::test_support::{connections, profile};

    struct F {
        profile: Profile,
        conns: LocalConnections,
        flow: ImportFlow,
    }

    impl F {
        fn new(conns: &[(&str, ProviderKind)]) -> Self {
            Self {
                profile: profile(),
                conns: connections(conns),
                flow: ImportFlow::new(),
            }
        }
        fn input(&mut self, input: ImportInput) -> Vec<Command> {
            let ctx = SettingsContext {
                profile: &self.profile,
                connections: &self.conns,
            };
            self.flow.input(input, ctx)
        }
    }

    fn teammate_file() -> String {
        let incoming = Profile::from_drafts(&[
            WatchDraft::cloudflare(
                "Site",
                "acme-cf",
                "0123456789abcdef0123456789abcdef",
                "site",
                None,
            )
            .with_id("s"),
            WatchDraft::github("Billing", "team-gh", "acme", "billing", "ci.yml", None)
                .with_id("b"),
        ])
        .unwrap();
        export_text(&incoming)
    }

    #[test]
    fn malformed_text_stays_on_input_with_error() {
        let mut f = F::new(&[]);
        f.input(ImportInput::SetText(
            "version = 1\n[[watch]]\nlable = 'x'\n".into(),
        ));
        assert_eq!(f.flow.text(), "version = 1\n[[watch]]\nlable = 'x'\n");
        assert!(f.input(ImportInput::Load).is_empty());
        assert_eq!(f.flow.step(), ImportStep::Input);
        let error = f.flow.error().unwrap();
        assert!(
            error.starts_with("This profile cannot be imported"),
            "{error}"
        );
        assert!(error.contains("lable"), "{error}");
        assert!(f.flow.required().is_empty());
        // Inputs for later steps do nothing here.
        assert!(f.input(ImportInput::Confirm).is_empty());
        f.input(ImportInput::Back);
        assert_eq!(f.flow.step(), ImportStep::Input);
    }

    #[test]
    fn full_import_with_mapping_and_created_connection() {
        let mut f = F::new(&[
            ("work", ProviderKind::GitHub),
            ("home", ProviderKind::GitHub),
            ("cf", ProviderKind::Cloudflare),
        ]);
        f.input(ImportInput::SetText(teammate_file()));
        f.input(ImportInput::Load);
        assert_eq!(f.flow.step(), ImportStep::Mapping);
        let required = f.flow.required().to_vec();
        assert_eq!(required.len(), 2);
        // Cloudflare: only one local connection → suggested.
        assert_eq!(f.flow.mapped(&required[0]).unwrap().as_str(), "cf");
        // GitHub: two candidates → left for the user.
        assert_eq!(f.flow.mapped(&required[1]), None);
        let ctx = SettingsContext {
            profile: &f.profile,
            connections: &f.conns,
        };
        assert_eq!(choices_for(ProviderKind::GitHub, ctx).len(), 2);

        // Continuing with a gap lists the problem and stays.
        f.input(ImportInput::Continue);
        assert_eq!(f.flow.step(), ImportStep::Mapping);
        assert!(f.flow.error().unwrap().contains("team-gh"));

        // Create a new GitHub connection for "team-gh".
        f.input(ImportInput::CreateConnection(1));
        let (index, flow) = f.flow.creating().unwrap();
        assert_eq!(index, 1);
        let ConnectFlow::Token(form) = flow;
        assert_eq!(form.alias, "team-gh", "sender's alias is free here");
        f.input(ImportInput::Connect(ConnectInput::SetToken("t".into())));
        let commands = f.input(ImportInput::Connect(ConnectInput::Submit));
        let [Command::Connect { ticket, request }] = commands.as_slice() else {
            panic!()
        };
        f.conns.insert(
            request.alias.clone(),
            buildrail_profile::LocalConnection {
                provider: ProviderKind::GitHub,
                display_name: "octocat".into(),
            },
        );
        f.flow.on_event(&Event::Connected {
            ticket: *ticket,
            result: Ok(NewConnection {
                alias: request.alias.clone(),
                provider: ProviderKind::GitHub,
                display_name: "octocat".into(),
            }),
        });
        assert!(f.flow.creating().is_none());
        assert_eq!(f.flow.mapped(&required[1]).unwrap().as_str(), "team-gh");

        f.input(ImportInput::Continue);
        assert_eq!(f.flow.step(), ImportStep::Review);
        let (applied, diff) = f.flow.applied().unwrap();
        assert_eq!(applied.watches()[0].connection.as_str(), "cf");
        assert_eq!(applied.watches()[1].connection.as_str(), "team-gh");
        let counts = diff.counts();
        assert_eq!(
            (counts.added, counts.removed, counts.unchanged),
            (1, 1, 1),
            "{diff:?}"
        );
        assert!(diff
            .entries
            .iter()
            .any(|e| matches!(&e.kind, DiffKind::Removed(w) if w.label == "Keevoro")));

        let commands = f.input(ImportInput::Confirm);
        let [Command::ReplaceProfile { ticket, profile }] = commands.as_slice() else {
            panic!()
        };
        assert_eq!(profile.len(), 2);
        assert!(f.flow.is_saving());
        assert!(f.input(ImportInput::Confirm).is_empty(), "no double submit");
        f.flow.on_event(&Event::Saved {
            ticket: *ticket,
            result: Err("disk full".into()),
        });
        assert_eq!(f.flow.error(), Some("disk full"));
        assert_eq!(f.flow.step(), ImportStep::Review);
        let commands = f.input(ImportInput::Confirm);
        let [Command::ReplaceProfile { ticket, .. }] = commands.as_slice() else {
            panic!()
        };
        f.flow.on_event(&Event::Saved {
            ticket: *ticket,
            result: Ok(()),
        });
        assert_eq!(f.flow.step(), ImportStep::Done);
        f.input(ImportInput::Back);
        assert_eq!(f.flow.step(), ImportStep::Done);
    }

    #[test]
    fn mapping_edits_and_back_navigation() {
        let mut f = F::new(&[
            ("cf", ProviderKind::Cloudflare),
            ("gh", ProviderKind::GitHub),
        ]);
        f.input(ImportInput::SetText(teammate_file()));
        f.input(ImportInput::Load);
        let required = f.flow.required().to_vec();
        f.input(ImportInput::Map {
            index: 0,
            local: None,
        });
        assert_eq!(f.flow.mapped(&required[0]), None);
        // Wrong provider is reported on Continue.
        f.input(ImportInput::Map {
            index: 0,
            local: Some(ConnectionAlias::new("gh")),
        });
        f.input(ImportInput::Continue);
        assert!(f.flow.error().unwrap().contains("is a GitHub connection"));
        f.input(ImportInput::Map {
            index: 0,
            local: Some(ConnectionAlias::new("cf")),
        });
        f.input(ImportInput::Map {
            index: 9,
            local: None,
        });
        // Creating a connection whose alias is taken suggests a free one.
        f.input(ImportInput::Map {
            index: 1,
            local: Some(ConnectionAlias::new("gh")),
        });
        f.input(ImportInput::CreateConnection(9));
        assert!(f.flow.creating().is_none());
        f.input(ImportInput::CreateConnection(0));
        assert!(f.flow.creating().is_some());
        f.input(ImportInput::CancelCreate);
        assert!(f.flow.creating().is_none());
        assert!(f
            .input(ImportInput::Connect(ConnectInput::Submit))
            .is_empty());
        f.input(ImportInput::Continue);
        assert_eq!(f.flow.step(), ImportStep::Review);
        f.input(ImportInput::Back);
        assert_eq!(f.flow.step(), ImportStep::Mapping);
        assert!(f.flow.applied().is_none());
        f.input(ImportInput::Back);
        assert_eq!(f.flow.step(), ImportStep::Input);
        assert!(f.flow.preview().is_none());
        assert_eq!(ImportFlow::default().step(), ImportStep::Input);
        assert_eq!(capitalise(""), "");
    }

    #[test]
    fn taken_alias_gets_fresh_suggestion() {
        let ctx_conns = connections(&[("acme-cf", ProviderKind::GitHub)]);
        let p = Profile::empty();
        let ctx = SettingsContext {
            profile: &p,
            connections: &ctx_conns,
        };
        let required = RequiredConnection {
            alias: ConnectionAlias::new("acme-cf"),
            provider: ProviderKind::Cloudflare,
            watch_count: 1,
        };
        let ConnectFlow::Token(form) = new_connection_for(&required, ctx);
        assert_eq!(form.alias, "cloudflare");
    }

    #[test]
    fn export_has_no_secret_fields() {
        let text = export_text(&profile());
        assert!(text.contains("connection = \"work\""));
        assert!(!text.to_lowercase().contains("token"));
    }
}
