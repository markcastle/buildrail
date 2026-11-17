//! Obtaining a provider credential for a connection.
//!
//! The wizard, the import flow and "Reconnect" all need the same thing: a
//! verified credential stored under a connection alias. They hold a
//! [`ConnectFlow`], forward [`ConnectInput`]s and [`Event`]s to it, and get
//! a [`NewConnection`] back when it is done. They never look inside.
//!
//! Today there is one method, [`ConnectMethod::PasteToken`]: the user
//! creates a read-only token on the provider's site (we show exactly which
//! permissions) and pastes it into a masked field. An OAuth device flow
//! would be one more [`ConnectFlow`] variant with its own inputs and its own
//! UI panel; the callers stay unchanged because they only see
//! `input` / `on_event` / `NewConnection`.
//!
//! The token is moved into a [`Secret`] on submit and cleared from the form
//! immediately, so it is not kept in UI state; the controller verifies it
//! with the provider's identity call, writes it to the OS credential store
//! and registers the alias in the device preferences.

use buildrail_core::domain::{ConnectionAlias, ProviderKind};
use buildrail_core::ports::Secret;
use buildrail_profile::{validate_alias, LocalConnections};

use super::{suggest_alias, Command, Event, LoadError, Ticket};

/// How a credential is obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectMethod {
    /// Paste a read-only personal / API token.
    PasteToken,
}

/// What the controller is asked to do for a connect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectRequest {
    /// Provider the credential is for.
    pub provider: ProviderKind,
    /// Local alias to store it under.
    pub alias: ConnectionAlias,
    /// The credential. Never logged (its `Debug` prints `***`).
    pub secret: Secret,
    /// `true` when replacing the credential of an existing alias
    /// (Reconnect); `false` refuses to overwrite one.
    pub replace: bool,
}

/// A connection that is now registered and has a stored credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewConnection {
    /// Its alias.
    pub alias: ConnectionAlias,
    /// Its provider.
    pub provider: ProviderKind,
    /// Who the credential belongs to (GitHub login, Cloudflare token ID).
    pub display_name: String,
}

/// Why connecting failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectError {
    /// The provider refused the token or could not be asked.
    Verify(LoadError),
    /// The token exists but is not active (`disabled`, `expired`).
    Inactive(String),
    /// The OS credential store or the preferences file failed.
    Store(String),
    /// The alias is unusable (taken, or invalid).
    Alias(String),
}

impl ConnectError {
    /// User-facing message, with permission guidance for refused tokens.
    pub fn message(&self, provider: ProviderKind) -> String {
        match self {
            Self::Verify(LoadError::Access(_)) => format!(
                "{} refused this token. {}",
                provider.display_name(),
                guidance(provider).permissions
            ),
            Self::Verify(error) => error.message().to_owned(),
            Self::Inactive(status) => {
                format!("This token is {status}. Create a new one and paste it here.")
            }
            Self::Store(message) => format!("BuildRail could not save the connection: {message}"),
            Self::Alias(message) => message.clone(),
        }
    }
}

/// What to tell the user about creating a token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenGuidance {
    /// What kind of token ("GitHub fine-grained personal access token").
    pub title: &'static str,
    /// Where to create it.
    pub url: &'static str,
    /// Step-by-step instructions, each a sentence.
    pub steps: &'static [&'static str],
    /// The exact read permissions, in one sentence.
    pub permissions: &'static str,
    /// An example of what a token looks like, for the empty field.
    pub placeholder: &'static str,
}

/// Token instructions for `provider`. Read-only permissions only. Menu
/// paths are joined with `›`, as the design system joins steps.
pub fn guidance(provider: ProviderKind) -> TokenGuidance {
    match provider {
        ProviderKind::GitHub => TokenGuidance {
            title: "GitHub fine-grained personal access token",
            url: "https://github.com/settings/personal-access-tokens/new",
            steps: &[
                "On GitHub, open Settings › Developer settings › Fine-grained tokens › Generate new token.",
                "Pick the repositories you want to watch.",
                "Under Repository permissions, set Actions and Metadata to Read-only. Nothing else is needed.",
                "Copy the token and paste it below.",
            ],
            permissions: "It needs only the repository permissions Actions: Read and Metadata: Read.",
            placeholder: "github_pat_11ABCDEFG0…",
        },
        ProviderKind::Cloudflare => TokenGuidance {
            title: "Cloudflare user API token",
            url: "https://dash.cloudflare.com/profile/api-tokens",
            steps: &[
                "In the Cloudflare dashboard, open My Profile › API Tokens › Create Token › Custom token. Workers Builds accepts only user tokens, not account-owned ones.",
                "Add the permissions Account › Workers CI › Read and Account › Workers Scripts › Read.",
                "Include the accounts whose Workers you want to watch.",
                "Copy the token and paste it below.",
            ],
            permissions: "It must be a user API token with only Workers CI: Read and Workers Scripts: Read.",
            placeholder: "Xq7f2Lr9…",
        },
    }
}

/// Input for a [`ConnectFlow`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectInput {
    /// The alias field changed.
    SetAlias(String),
    /// The (masked) token field changed.
    SetToken(String),
    /// "Connect" pressed.
    Submit,
}

/// The paste-a-token form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenForm {
    /// Provider the token is for.
    pub provider: ProviderKind,
    /// Connection alias (editable unless `alias_locked`).
    pub alias: String,
    /// `true` for Reconnect: the alias is fixed and its credential replaced.
    pub alias_locked: bool,
    /// The token as typed. Cleared on submit.
    pub token: String,
    /// The last problem, shown under the form.
    pub error: Option<String>,
    waiting: Option<Ticket>,
}

/// A credential being obtained. See the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectFlow {
    /// Paste a token.
    Token(TokenForm),
}

impl ConnectFlow {
    /// A new connection for `provider` with a free suggested alias.
    pub fn new_connection(provider: ProviderKind, connections: &LocalConnections) -> Self {
        Self::with_alias(provider, suggest_alias(provider, connections), false)
    }

    /// A new connection whose alias defaults to `alias` (editable), e.g. the
    /// alias used in an imported profile.
    pub fn with_alias(provider: ProviderKind, alias: impl Into<String>, locked: bool) -> Self {
        Self::Token(TokenForm {
            provider,
            alias: alias.into(),
            alias_locked: locked,
            token: String::new(),
            error: None,
            waiting: None,
        })
    }

    /// Replaces the credential of an existing connection.
    pub fn reconnect(provider: ProviderKind, alias: &ConnectionAlias) -> Self {
        Self::with_alias(provider, alias.as_str(), true)
    }

    /// How this flow obtains the credential.
    pub fn method(&self) -> ConnectMethod {
        match self {
            Self::Token(_) => ConnectMethod::PasteToken,
        }
    }

    /// Provider being connected.
    pub fn provider(&self) -> ProviderKind {
        match self {
            Self::Token(form) => form.provider,
        }
    }

    /// `true` while the controller is verifying.
    pub fn is_busy(&self) -> bool {
        match self {
            Self::Token(form) => form.waiting.is_some(),
        }
    }

    /// The current error message, if any.
    pub fn error(&self) -> Option<&str> {
        match self {
            Self::Token(form) => form.error.as_deref(),
        }
    }

    /// Handles input; returns the commands to execute.
    pub fn input(&mut self, input: ConnectInput, connections: &LocalConnections) -> Vec<Command> {
        let Self::Token(form) = self;
        if form.waiting.is_some() {
            return Vec::new();
        }
        match input {
            ConnectInput::SetAlias(alias) => {
                if !form.alias_locked {
                    form.alias = alias;
                }
                Vec::new()
            }
            ConnectInput::SetToken(token) => {
                form.token = token;
                Vec::new()
            }
            ConnectInput::Submit => form.submit(connections),
        }
    }

    /// Handles a controller event; returns the connection once done.
    pub fn on_event(&mut self, event: &Event) -> Option<NewConnection> {
        let Self::Token(form) = self;
        let Event::Connected { ticket, result } = event else {
            return None;
        };
        if form.waiting != Some(*ticket) {
            return None;
        }
        form.waiting = None;
        match result {
            Ok(connection) => {
                form.error = None;
                Some(connection.clone())
            }
            Err(error) => {
                form.error = Some(error.message(form.provider));
                None
            }
        }
    }
}

impl TokenForm {
    fn submit(&mut self, connections: &LocalConnections) -> Vec<Command> {
        let alias = match validate_alias(&self.alias) {
            Ok(alias) => alias,
            Err(message) => {
                self.error = Some(format!("Connection name: {message}"));
                return Vec::new();
            }
        };
        if !self.alias_locked && connections.contains_key(&alias) {
            self.error = Some(format!(
                "A connection named \"{alias}\" already exists. Choose another name."
            ));
            return Vec::new();
        }
        let token = std::mem::take(&mut self.token);
        let token = token.trim();
        if token.is_empty() {
            self.error = Some("Paste a token first.".to_owned());
            return Vec::new();
        }
        let ticket = Ticket::next();
        self.waiting = Some(ticket);
        self.error = None;
        vec![Command::Connect {
            ticket,
            request: ConnectRequest {
                provider: self.provider,
                alias,
                secret: Secret::new(token),
                replace: self.alias_locked,
            },
        }]
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::panic)]

    use super::*;
    use crate::settings::test_support::connections;

    fn submit(flow: &mut ConnectFlow, conns: &LocalConnections) -> (Ticket, ConnectRequest) {
        match flow.input(ConnectInput::Submit, conns).pop() {
            Some(Command::Connect { ticket, request }) => (ticket, request),
            other => panic!("expected connect, got {other:?}"),
        }
    }

    #[test]
    fn happy_path_clears_token_and_returns_connection() {
        let conns = connections(&[("github", ProviderKind::GitHub)]);
        let mut flow = ConnectFlow::new_connection(ProviderKind::GitHub, &conns);
        assert_eq!(flow.method(), ConnectMethod::PasteToken);
        assert_eq!(flow.provider(), ProviderKind::GitHub);
        let ConnectFlow::Token(form) = &flow;
        assert_eq!(form.alias, "github-2");
        flow.input(ConnectInput::SetToken("  ghp_secret  ".into()), &conns);
        let (ticket, request) = submit(&mut flow, &conns);
        assert_eq!(request.secret.expose(), "ghp_secret");
        assert_eq!(request.alias.as_str(), "github-2");
        assert!(!request.replace);
        assert!(flow.is_busy());
        let ConnectFlow::Token(form) = &flow;
        assert!(form.token.is_empty(), "token must not linger in UI state");
        // Input while busy is ignored.
        assert!(flow.input(ConnectInput::Submit, &conns).is_empty());

        // Events for other tickets or of other kinds are ignored.
        let other = Event::Connected {
            ticket: Ticket::next(),
            result: Err(ConnectError::Store("x".into())),
        };
        assert_eq!(flow.on_event(&other), None);
        let saved = Event::Saved {
            ticket,
            result: Ok(()),
        };
        assert_eq!(flow.on_event(&saved), None);
        assert!(flow.is_busy());

        let done = NewConnection {
            alias: request.alias.clone(),
            provider: ProviderKind::GitHub,
            display_name: "octocat".into(),
        };
        let event = Event::Connected {
            ticket,
            result: Ok(done.clone()),
        };
        assert_eq!(flow.on_event(&event), Some(done));
        assert!(!flow.is_busy());
        assert_eq!(flow.error(), None);
    }

    #[test]
    fn validation_errors() {
        let conns = connections(&[("work", ProviderKind::GitHub)]);
        let mut flow = ConnectFlow::with_alias(ProviderKind::GitHub, "work", false);
        flow.input(ConnectInput::SetToken("t".into()), &conns);
        assert!(flow.input(ConnectInput::Submit, &conns).is_empty());
        assert!(flow.error().unwrap().contains("already exists"));

        flow.input(ConnectInput::SetAlias("bad alias!".into()), &conns);
        assert!(flow.input(ConnectInput::Submit, &conns).is_empty());
        assert!(flow.error().unwrap().starts_with("Connection name:"));

        flow.input(ConnectInput::SetAlias("fresh".into()), &conns);
        flow.input(ConnectInput::SetToken("   ".into()), &conns);
        assert!(flow.input(ConnectInput::Submit, &conns).is_empty());
        assert_eq!(flow.error(), Some("Paste a token first."));
    }

    #[test]
    fn reconnect_locks_alias_and_replaces() {
        let conns = connections(&[("work", ProviderKind::GitHub)]);
        let alias = ConnectionAlias::new("work");
        let mut flow = ConnectFlow::reconnect(ProviderKind::GitHub, &alias);
        flow.input(ConnectInput::SetAlias("other".into()), &conns);
        flow.input(ConnectInput::SetToken("t".into()), &conns);
        let (ticket, request) = submit(&mut flow, &conns);
        assert_eq!(request.alias, alias);
        assert!(request.replace);
        let event = Event::Connected {
            ticket,
            result: Err(ConnectError::Verify(LoadError::Access("x".into()))),
        };
        assert_eq!(flow.on_event(&event), None);
        let message = flow.error().unwrap();
        assert!(
            message.contains("Actions: Read and Metadata: Read"),
            "{message}"
        );
    }

    #[test]
    fn error_messages_and_guidance() {
        let cf = ProviderKind::Cloudflare;
        assert!(ConnectError::Verify(LoadError::Access("x".into()))
            .message(cf)
            .contains("user API token with only Workers CI: Read and Workers Scripts: Read"));
        assert_eq!(
            ConnectError::Verify(LoadError::Other("offline".into())).message(cf),
            "offline"
        );
        assert!(ConnectError::Inactive("expired".into())
            .message(cf)
            .contains("This token is expired"));
        assert!(ConnectError::Store("locked".into())
            .message(cf)
            .ends_with("locked"));
        assert_eq!(ConnectError::Alias("taken".into()).message(cf), "taken");
        for provider in [ProviderKind::GitHub, ProviderKind::Cloudflare] {
            let g = guidance(provider);
            assert!(g.url.starts_with("https://"));
            assert!(g.steps.len() >= 3);
            assert!(g.steps.iter().any(|s| s.contains("Read")));
        }
    }
}
