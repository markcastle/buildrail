//! The single validation path shared by the watch picker and the TOML
//! importer.
//!
//! Both front ends produce a [`WatchDraft`] – raw, untrusted strings exactly
//! as typed or read – and hand it to [`validate_draft`] (one watch) or
//! [`validate_drafts`] (a whole ordered list, which also checks that IDs are
//! unique). Validation never stops at the first problem: every issue is
//! collected into [`ValidationErrors`] so a form can highlight every bad
//! field at once and an import can list every broken line.

use std::collections::HashMap;
use std::fmt;

use buildrail_core::domain::{ConnectionAlias, ProviderKind, Watch, WatchId, WatchTarget};
use thiserror::Error;

/// Longest label accepted, in characters.
pub const MAX_LABEL_CHARS: usize = 64;
/// Longest watch ID accepted, in characters.
pub const MAX_ID_CHARS: usize = 64;
/// Longest connection alias accepted, in characters.
pub const MAX_ALIAS_CHARS: usize = 64;

/// Creates a fresh, globally unique watch ID (a UUID v4).
///
/// Use this for watches created in the picker. Hand-written TOML may also
/// use any short ID such as `"api-ci"`; see the crate docs for the rules.
pub fn new_watch_id() -> WatchId {
    WatchId::new(uuid::Uuid::new_v4().to_string())
}

/// A watch as entered by a person or read from a file, before validation.
///
/// Every field is optional so the importer can report a *missing* key
/// separately from an *empty* one. Surrounding whitespace is trimmed during
/// validation. Fields that do not belong to the chosen provider (for example
/// `account_id` on a GitHub watch) are rejected rather than silently
/// dropped.
///
/// A draft with `id: None` receives a newly generated ID when validated.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WatchDraft {
    /// Stable ID; `None` for a new watch.
    pub id: Option<String>,
    /// Text shown on the strip.
    pub label: Option<String>,
    /// `"github"` or `"cloudflare"`.
    pub provider: Option<String>,
    /// Local connection alias supplying the credential.
    pub connection: Option<String>,
    /// GitHub repository owner.
    pub owner: Option<String>,
    /// GitHub repository name.
    pub repo: Option<String>,
    /// GitHub workflow file name (`ci.yml`) or numeric workflow ID.
    pub workflow: Option<String>,
    /// Cloudflare account ID (32 hex characters).
    pub account_id: Option<String>,
    /// Cloudflare Worker name.
    pub worker: Option<String>,
    /// Optional branch filter. Leave `None` (not empty) for "any branch".
    pub branch: Option<String>,
}

impl WatchDraft {
    /// A draft for a new GitHub Actions watch, as the picker would build it.
    pub fn github(
        label: impl Into<String>,
        connection: impl Into<String>,
        owner: impl Into<String>,
        repo: impl Into<String>,
        workflow: impl Into<String>,
        branch: Option<String>,
    ) -> Self {
        Self {
            label: Some(label.into()),
            provider: Some("github".into()),
            connection: Some(connection.into()),
            owner: Some(owner.into()),
            repo: Some(repo.into()),
            workflow: Some(workflow.into()),
            branch,
            ..Self::default()
        }
    }

    /// A draft for a new Cloudflare Workers Builds watch.
    pub fn cloudflare(
        label: impl Into<String>,
        connection: impl Into<String>,
        account_id: impl Into<String>,
        worker: impl Into<String>,
        branch: Option<String>,
    ) -> Self {
        Self {
            label: Some(label.into()),
            provider: Some("cloudflare".into()),
            connection: Some(connection.into()),
            account_id: Some(account_id.into()),
            worker: Some(worker.into()),
            branch,
            ..Self::default()
        }
    }

    /// Sets the ID (builder style), e.g. to keep an existing watch's identity.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }
}

impl From<&Watch> for WatchDraft {
    /// The draft that validates back to exactly this watch. Used to edit an
    /// existing watch and to export it.
    fn from(watch: &Watch) -> Self {
        let mut draft = Self {
            id: Some(watch.id.as_str().to_owned()),
            label: Some(watch.label.clone()),
            connection: Some(watch.connection.as_str().to_owned()),
            ..Self::default()
        };
        match &watch.target {
            WatchTarget::GitHub {
                owner,
                repo,
                workflow,
                branch,
            } => {
                draft.provider = Some("github".into());
                draft.owner = Some(owner.clone());
                draft.repo = Some(repo.clone());
                draft.workflow = Some(workflow.clone());
                draft.branch = branch.clone();
            }
            WatchTarget::Cloudflare {
                account_id,
                worker,
                branch,
            } => {
                draft.provider = Some("cloudflare".into());
                draft.account_id = Some(account_id.clone());
                draft.worker = Some(worker.clone());
                draft.branch = branch.clone();
            }
        }
        draft
    }
}

/// Which input a [`ValidationIssue`] is about. For watch fields the
/// [`Field::key`] equals the TOML key and is a sensible form-field name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Field {
    /// Watch `id`.
    Id,
    /// Watch `label`.
    Label,
    /// Watch `provider`.
    Provider,
    /// Watch `connection`.
    Connection,
    /// GitHub `owner`.
    Owner,
    /// GitHub `repo`.
    Repo,
    /// GitHub `workflow`.
    Workflow,
    /// Cloudflare `account_id`.
    AccountId,
    /// Cloudflare `worker`.
    Worker,
    /// Optional `branch`.
    Branch,
    /// A connection's `display_name` (device preferences).
    DisplayName,
    /// The `scale` override (device preferences).
    Scale,
    /// A key the format does not know (likely a typo), or a key with the
    /// wrong type. Holds the key as written.
    Key(String),
}

impl Field {
    /// The TOML key this field is stored under.
    pub fn key(&self) -> &str {
        match self {
            Self::Id => "id",
            Self::Label => "label",
            Self::Provider => "provider",
            Self::Connection => "connection",
            Self::Owner => "owner",
            Self::Repo => "repo",
            Self::Workflow => "workflow",
            Self::AccountId => "account_id",
            Self::Worker => "worker",
            Self::Branch => "branch",
            Self::DisplayName => "display_name",
            Self::Scale => "scale",
            Self::Key(key) => key,
        }
    }
}

impl fmt::Display for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.key())
    }
}

/// Where a [`ValidationIssue`] was found.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IssueLocation {
    /// The single draft passed to [`validate_draft`] (the picker form).
    Draft,
    /// The top level of a TOML document.
    Document,
    /// The watch at `index` (zero-based) in an ordered list. `label` is the
    /// label as written, when there was one, to help people find it.
    Watch {
        /// Zero-based position.
        index: usize,
        /// The (trimmed) label, if any.
        label: Option<String>,
    },
    /// A local connection in the device preferences.
    Connection {
        /// The alias as written.
        alias: String,
    },
}

impl fmt::Display for IssueLocation {
    /// Human prefix; watches are numbered from 1.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Draft | Self::Document => Ok(()),
            Self::Watch {
                index,
                label: Some(label),
            } => write!(f, "watch {} ({label:?})", index + 1),
            Self::Watch { index, label: None } => write!(f, "watch {}", index + 1),
            Self::Connection { alias } => write!(f, "connection {alias:?}"),
        }
    }
}

/// One problem found during validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    /// Which entry the problem is in.
    pub location: IssueLocation,
    /// Which field of that entry.
    pub field: Field,
    /// Human-readable sentence, already naming the field
    /// (`"repo must not be empty"`).
    pub message: String,
}

impl fmt::Display for ValidationIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.location {
            IssueLocation::Draft | IssueLocation::Document => f.write_str(&self.message),
            _ => write!(f, "{}: {}", self.location, self.message),
        }
    }
}

/// Every problem found in one validation run. Never empty when returned as
/// an error.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub struct ValidationErrors {
    issues: Vec<ValidationIssue>,
}

impl ValidationErrors {
    /// Wraps a list of issues.
    pub fn new(issues: Vec<ValidationIssue>) -> Self {
        Self { issues }
    }

    /// The individual issues, in document order.
    pub fn issues(&self) -> &[ValidationIssue] {
        &self.issues
    }

    /// Issues about one field, for highlighting a single form input.
    pub fn for_field<'a>(&'a self, field: &'a Field) -> impl Iterator<Item = &'a ValidationIssue> {
        self.issues
            .iter()
            .filter(move |issue| &issue.field == field)
    }

    /// `Err(self)` when there is at least one issue.
    pub(crate) fn into_result(self) -> Result<(), Self> {
        if self.issues.is_empty() {
            Ok(())
        } else {
            Err(self)
        }
    }
}

impl fmt::Display for ValidationErrors {
    /// One issue per line.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (n, issue) in self.issues.iter().enumerate() {
            if n > 0 {
                f.write_str("\n")?;
            }
            write!(f, "{issue}")?;
        }
        Ok(())
    }
}

/// Validates one draft from the picker. Issues carry
/// [`IssueLocation::Draft`].
pub fn validate_draft(draft: &WatchDraft) -> Result<Watch, ValidationErrors> {
    let mut sink = IssueSink::new(IssueLocation::Draft);
    let watch = check_watch(draft, &mut sink);
    sink.finish(watch)
}

/// Validates an ordered list of drafts (a whole profile): every watch on its
/// own, plus ID uniqueness across the list. Order is preserved.
pub fn validate_drafts(drafts: &[WatchDraft]) -> Result<Vec<Watch>, ValidationErrors> {
    let mut issues = Vec::new();
    let mut watches = Vec::with_capacity(drafts.len());
    let mut seen: HashMap<WatchId, usize> = HashMap::new();
    for (index, draft) in drafts.iter().enumerate() {
        let location = IssueLocation::Watch {
            index,
            label: draft
                .label
                .as_deref()
                .map(str::trim)
                .filter(|label| !label.is_empty())
                .map(str::to_owned),
        };
        let mut sink = IssueSink::new(location.clone());
        if let Some(watch) = check_watch(draft, &mut sink) {
            if let Some(first) = seen.get(&watch.id) {
                sink.push(
                    Field::Id,
                    format!(
                        "id {:?} is already used by watch {}",
                        watch.id.as_str(),
                        first + 1
                    ),
                );
            } else {
                seen.insert(watch.id.clone(), index);
            }
            watches.push(watch);
        }
        issues.append(&mut sink.issues);
    }
    ValidationErrors::new(issues)
        .into_result()
        .map(|()| watches)
}

/// Validates a connection alias on its own (used by device preferences and
/// alias mapping).
pub fn validate_alias(alias: &str) -> Result<ConnectionAlias, String> {
    check_alias(alias.trim()).map(|()| ConnectionAlias::new(alias.trim()))
}

/// Collects issues for one location.
pub(crate) struct IssueSink {
    location: IssueLocation,
    pub(crate) issues: Vec<ValidationIssue>,
}

impl IssueSink {
    pub(crate) fn new(location: IssueLocation) -> Self {
        Self {
            location,
            issues: Vec::new(),
        }
    }

    pub(crate) fn push(&mut self, field: Field, message: impl Into<String>) {
        self.issues.push(ValidationIssue {
            location: self.location.clone(),
            field,
            message: message.into(),
        });
    }

    /// Records the result of a rule check under `field`.
    fn check(&mut self, field: Field, result: Result<(), String>) -> bool {
        match result {
            Ok(()) => true,
            Err(message) => {
                self.push(field, message);
                false
            }
        }
    }

    /// A required, trimmed, non-empty value.
    fn required<'a>(&mut self, field: Field, value: Option<&'a str>) -> Option<&'a str> {
        match value.map(str::trim) {
            None => {
                self.push(field.clone(), format!("{field} is required"));
                None
            }
            Some("") => {
                self.push(field.clone(), format!("{field} must not be empty"));
                None
            }
            Some(value) => Some(value),
        }
    }

    /// A required value that must also satisfy `rule`.
    fn required_with(
        &mut self,
        field: Field,
        value: Option<&str>,
        rule: fn(&str) -> Result<(), String>,
    ) -> Option<String> {
        let value = self.required(field.clone(), value)?;
        self.check(field, rule(value)).then(|| value.to_owned())
    }

    fn unexpected(&mut self, field: Field, value: Option<&str>, provider: ProviderKind) {
        if value.is_some() {
            let other = match provider {
                ProviderKind::GitHub => "cloudflare",
                ProviderKind::Cloudflare => "github",
            };
            self.push(
                field.clone(),
                format!("{field} is only used by {other} watches, not {provider} ones"),
            );
        }
    }

    pub(crate) fn finish<T>(self, value: Option<T>) -> Result<T, ValidationErrors> {
        match value {
            Some(value) if self.issues.is_empty() => Ok(value),
            _ => Err(ValidationErrors::new(self.issues)),
        }
    }
}

fn check_watch(draft: &WatchDraft, sink: &mut IssueSink) -> Option<Watch> {
    let id = match draft.id.as_deref() {
        None => Some(new_watch_id()),
        Some(raw) => sink
            .required_with(Field::Id, Some(raw), check_id)
            .map(WatchId::new),
    };
    let label = sink.required_with(Field::Label, draft.label.as_deref(), check_label);
    let connection = sink
        .required_with(Field::Connection, draft.connection.as_deref(), check_alias)
        .map(ConnectionAlias::new);
    let branch = match draft.branch.as_deref().map(str::trim) {
        None => Some(None),
        Some("") => {
            sink.push(
                Field::Branch,
                "branch must not be empty (leave it out to watch every branch)",
            );
            None
        }
        Some(branch) => sink
            .check(Field::Branch, check_branch(branch))
            .then(|| Some(branch.to_owned())),
    };
    let target = match parse_provider(draft.provider.as_deref(), sink) {
        Some(ProviderKind::GitHub) => check_github(draft, sink, branch),
        Some(ProviderKind::Cloudflare) => check_cloudflare(draft, sink, branch),
        None => None,
    };
    Some(Watch {
        id: id?,
        label: label?,
        connection: connection?,
        target: target?,
    })
}

fn parse_provider(raw: Option<&str>, sink: &mut IssueSink) -> Option<ProviderKind> {
    match sink.required(Field::Provider, raw)? {
        "github" => Some(ProviderKind::GitHub),
        "cloudflare" => Some(ProviderKind::Cloudflare),
        other => {
            sink.push(
                Field::Provider,
                format!("provider {other:?} is not supported (use \"github\" or \"cloudflare\")"),
            );
            None
        }
    }
}

fn check_github(
    draft: &WatchDraft,
    sink: &mut IssueSink,
    branch: Option<Option<String>>,
) -> Option<WatchTarget> {
    let owner = sink.required_with(Field::Owner, draft.owner.as_deref(), check_owner);
    let repo = sink.required_with(Field::Repo, draft.repo.as_deref(), check_repo);
    let workflow = sink.required_with(Field::Workflow, draft.workflow.as_deref(), check_workflow);
    sink.unexpected(
        Field::AccountId,
        draft.account_id.as_deref(),
        ProviderKind::GitHub,
    );
    sink.unexpected(Field::Worker, draft.worker.as_deref(), ProviderKind::GitHub);
    Some(WatchTarget::GitHub {
        owner: owner?,
        repo: repo?,
        workflow: workflow?,
        branch: branch?,
    })
}

fn check_cloudflare(
    draft: &WatchDraft,
    sink: &mut IssueSink,
    branch: Option<Option<String>>,
) -> Option<WatchTarget> {
    let account_id = sink
        .required_with(
            Field::AccountId,
            draft.account_id.as_deref(),
            check_account_id,
        )
        .map(|id| id.to_ascii_lowercase());
    let worker = sink.required_with(Field::Worker, draft.worker.as_deref(), check_worker);
    sink.unexpected(
        Field::Owner,
        draft.owner.as_deref(),
        ProviderKind::Cloudflare,
    );
    sink.unexpected(Field::Repo, draft.repo.as_deref(), ProviderKind::Cloudflare);
    sink.unexpected(
        Field::Workflow,
        draft.workflow.as_deref(),
        ProviderKind::Cloudflare,
    );
    Some(WatchTarget::Cloudflare {
        account_id: account_id?,
        worker: worker?,
        branch: branch?,
    })
}

// ----- individual rules: input is already trimmed and non-empty -----------

fn max_chars(field: &str, value: &str, max: usize) -> Result<(), String> {
    let count = value.chars().count();
    if count > max {
        Err(format!(
            "{field} must be at most {max} characters (it has {count})"
        ))
    } else {
        Ok(())
    }
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

fn check_id(id: &str) -> Result<(), String> {
    max_chars("id", id, MAX_ID_CHARS)?;
    if id.chars().all(is_word_char) {
        Ok(())
    } else {
        Err("id may only contain letters, digits, '-' and '_'".into())
    }
}

/// Label rule: at most [`MAX_LABEL_CHARS`] characters, no control characters.
pub(crate) fn check_label(label: &str) -> Result<(), String> {
    max_chars("label", label, MAX_LABEL_CHARS)?;
    if label.chars().any(char::is_control) {
        Err("label must not contain control characters".into())
    } else {
        Ok(())
    }
}

fn check_alias(alias: &str) -> Result<(), String> {
    if alias.is_empty() {
        return Err("connection must not be empty".into());
    }
    max_chars("connection", alias, MAX_ALIAS_CHARS)?;
    let starts_alnum = alias.starts_with(|c: char| c.is_ascii_alphanumeric());
    if starts_alnum && alias.chars().all(|c| is_word_char(c) || c == '.') {
        Ok(())
    } else {
        Err(format!(
            "connection {alias:?} must start with a letter or digit and contain only letters, digits, '.', '-' and '_'"
        ))
    }
}

fn check_owner(owner: &str) -> Result<(), String> {
    max_chars("owner", owner, 39)?;
    let charset = owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if charset && !owner.starts_with('-') && !owner.ends_with('-') {
        Ok(())
    } else {
        Err(
            "owner may only contain letters, digits and '-', and cannot start or end with '-'"
                .into(),
        )
    }
}

fn check_repo(repo: &str) -> Result<(), String> {
    if repo.contains('/') {
        return Err(
            "repo must be the repository name only, without the owner (\"api\", not \"acme/api\")"
                .into(),
        );
    }
    max_chars("repo", repo, 100)?;
    let charset = repo
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if charset && repo != "." && repo != ".." {
        Ok(())
    } else {
        Err("repo may only contain letters, digits, '.', '-' and '_'".into())
    }
}

fn check_workflow(workflow: &str) -> Result<(), String> {
    if workflow.chars().all(|c| c.is_ascii_digit()) {
        return if workflow.len() <= 20 {
            Ok(())
        } else {
            Err("workflow ID is too long".into())
        };
    }
    if workflow.contains(['/', '\\']) {
        return Err("workflow must be a file name such as \"ci.yml\", not a path".into());
    }
    max_chars("workflow", workflow, 255)?;
    let has_extension = [".yml", ".yaml"]
        .iter()
        .any(|ext| workflow.len() > ext.len() && workflow.ends_with(ext));
    if !has_extension {
        Err("workflow must be a file name ending in .yml or .yaml, or a numeric workflow ID".into())
    } else if workflow.chars().any(char::is_control) {
        Err("workflow must not contain control characters".into())
    } else {
        Ok(())
    }
}

fn check_account_id(account_id: &str) -> Result<(), String> {
    if account_id.len() == 32 && account_id.chars().all(|c| c.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("account_id must be exactly 32 hexadecimal characters".into())
    }
}

fn check_worker(worker: &str) -> Result<(), String> {
    max_chars("worker", worker, 63)?;
    let charset = worker
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if charset && !worker.starts_with('-') && !worker.ends_with('-') {
        Ok(())
    } else {
        Err("worker may only contain lowercase letters, digits, '-' and '_', and cannot start or end with '-'".into())
    }
}

fn check_branch(branch: &str) -> Result<(), String> {
    max_chars("branch", branch, 255)?;
    let bad_char = branch
        .chars()
        .any(|c| c.is_control() || c.is_whitespace() || "~^:?*[\\".contains(c));
    let bad_shape = branch.contains("..")
        || branch.contains("//")
        || branch.contains("@{")
        || branch.starts_with(['-', '/', '.'])
        || branch.ends_with(['/', '.'])
        || branch.ends_with(".lock")
        || branch == "@";
    if bad_char || bad_shape {
        Err(format!("branch {branch:?} is not a valid git branch name"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    const ACCOUNT: &str = "0123456789abcdef0123456789ABCDEF";

    fn gh() -> WatchDraft {
        WatchDraft::github("API CI", "work-github", "acme", "api", "ci.yml", None)
    }

    fn cf() -> WatchDraft {
        WatchDraft::cloudflare(
            "Site",
            "acme-cf",
            ACCOUNT,
            "marketing-site",
            Some("main".into()),
        )
    }

    fn issues_of(draft: &WatchDraft) -> Vec<ValidationIssue> {
        validate_draft(draft).unwrap_err().issues().to_vec()
    }

    fn fields_of(draft: &WatchDraft) -> Vec<Field> {
        issues_of(draft).into_iter().map(|i| i.field).collect()
    }

    #[test]
    fn valid_github_draft_gets_generated_uuid() {
        let watch = validate_draft(&gh()).unwrap();
        assert_eq!(watch.label, "API CI");
        assert_eq!(watch.connection.as_str(), "work-github");
        assert!(uuid::Uuid::parse_str(watch.id.as_str()).is_ok());
        assert_eq!(
            watch.target,
            WatchTarget::GitHub {
                owner: "acme".into(),
                repo: "api".into(),
                workflow: "ci.yml".into(),
                branch: None
            }
        );
    }

    #[test]
    fn valid_cloudflare_draft_normalises_account_id_and_trims() {
        let mut draft = cf();
        draft.label = Some("  Site  ".into());
        let watch = validate_draft(&draft).unwrap();
        assert_eq!(watch.label, "Site");
        assert_eq!(
            watch.target,
            WatchTarget::Cloudflare {
                account_id: ACCOUNT.to_ascii_lowercase(),
                worker: "marketing-site".into(),
                branch: Some("main".into())
            }
        );
    }

    #[test]
    fn draft_round_trips_through_watch() {
        for draft in [gh().with_id("a"), cf().with_id("b")] {
            let watch = validate_draft(&draft).unwrap();
            let again = validate_draft(&WatchDraft::from(&watch)).unwrap();
            assert_eq!(watch, again);
        }
    }

    #[test]
    fn collects_every_problem() {
        let draft = WatchDraft {
            id: Some("bad id!".into()),
            label: Some("   ".into()),
            provider: Some("github".into()),
            connection: None,
            owner: Some("-acme".into()),
            repo: Some("".into()),
            workflow: Some("build".into()),
            account_id: Some(ACCOUNT.into()),
            worker: None,
            branch: Some("".into()),
        };
        assert_eq!(
            fields_of(&draft),
            vec![
                Field::Id,
                Field::Label,
                Field::Connection,
                Field::Branch,
                Field::Owner,
                Field::Repo,
                Field::Workflow,
                Field::AccountId
            ]
        );
        let text = validate_draft(&draft).unwrap_err().to_string();
        assert!(text.contains("label must not be empty"), "{text}");
        assert!(text.contains("connection is required"), "{text}");
        assert!(
            text.contains("account_id is only used by cloudflare watches"),
            "{text}"
        );
        assert_eq!(text.lines().count(), 8);
    }

    #[test]
    fn cloudflare_rejects_github_fields() {
        let mut draft = cf();
        draft.owner = Some("x".into());
        draft.repo = Some("y".into());
        draft.workflow = Some("z.yml".into());
        assert_eq!(
            fields_of(&draft),
            vec![Field::Owner, Field::Repo, Field::Workflow]
        );
    }

    #[test]
    fn provider_rules() {
        let mut draft = gh();
        draft.provider = Some("gitlab".into());
        let issues = issues_of(&draft);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].message.contains("\"gitlab\" is not supported"));
        draft.provider = None;
        assert_eq!(issues_of(&draft)[0].message, "provider is required");
    }

    #[test]
    fn label_rules() {
        let mut draft = gh();
        draft.label = Some("x".repeat(MAX_LABEL_CHARS));
        assert!(validate_draft(&draft).is_ok());
        draft.label = Some("é".repeat(MAX_LABEL_CHARS + 1));
        assert!(issues_of(&draft)[0].message.contains("at most 64"));
        draft.label = Some("a\u{7}b".into());
        assert!(issues_of(&draft)[0].message.contains("control"));
    }

    fn rejects(field: Field, values: &[&str], base: fn() -> WatchDraft) {
        for value in values {
            let mut draft = base();
            let slot = match field {
                Field::Id => &mut draft.id,
                Field::Connection => &mut draft.connection,
                Field::Owner => &mut draft.owner,
                Field::Repo => &mut draft.repo,
                Field::Workflow => &mut draft.workflow,
                Field::AccountId => &mut draft.account_id,
                Field::Worker => &mut draft.worker,
                Field::Branch => &mut draft.branch,
                _ => unreachable!(),
            };
            *slot = Some((*value).to_owned());
            assert_eq!(
                fields_of(&draft),
                vec![field.clone()],
                "{field} = {value:?}"
            );
        }
    }

    fn accepts(field: Field, values: &[&str], base: fn() -> WatchDraft) {
        for value in values {
            let mut draft = base();
            match field {
                Field::Id => draft.id = Some((*value).into()),
                Field::Connection => draft.connection = Some((*value).into()),
                Field::Owner => draft.owner = Some((*value).into()),
                Field::Repo => draft.repo = Some((*value).into()),
                Field::Workflow => draft.workflow = Some((*value).into()),
                Field::AccountId => draft.account_id = Some((*value).into()),
                Field::Worker => draft.worker = Some((*value).into()),
                Field::Branch => draft.branch = Some((*value).into()),
                _ => unreachable!(),
            }
            assert!(validate_draft(&draft).is_ok(), "{field} = {value:?}");
        }
    }

    #[test]
    fn identifier_rules() {
        let long = "a".repeat(65);
        accepts(
            Field::Id,
            &["api-ci", "6f1c2a3b-0000-4000-8000-000000000000", "x_1"],
            gh,
        );
        rejects(Field::Id, &["", "has space", "a.b", &long], gh);
        accepts(Field::Connection, &["work", "work.github", "w-1_2"], gh);
        rejects(Field::Connection, &["-work", ".x", "a b", "ä", &long], gh);
    }

    #[test]
    fn github_target_rules() {
        accepts(Field::Owner, &["acme", "a-b", "A1"], gh);
        rejects(Field::Owner, &["-a", "a-", "a_b", &"a".repeat(40)], gh);
        accepts(Field::Repo, &["api", "my.repo", "a_b-c", ".github"], gh);
        rejects(
            Field::Repo,
            &["acme/api", ".", "..", "a b", &"a".repeat(101)],
            gh,
        );
        accepts(
            Field::Workflow,
            &["ci.yml", "deploy.yaml", "12345", "Build & Test.yml"],
            gh,
        );
        rejects(
            Field::Workflow,
            &[
                ".github/workflows/ci.yml",
                "a\\b.yml",
                "ci",
                ".yml",
                "123456789012345678901",
                "a\u{1}.yml",
                &format!("{}.yml", "a".repeat(252)),
            ],
            gh,
        );
    }

    #[test]
    fn cloudflare_target_rules() {
        accepts(Field::AccountId, &[ACCOUNT], cf);
        rejects(
            Field::AccountId,
            &["abc", &"g".repeat(32), &"a".repeat(33)],
            cf,
        );
        accepts(Field::Worker, &["site", "my_worker-2"], cf);
        rejects(
            Field::Worker,
            &["Site", "-a", "a-", "a.b", &"a".repeat(64)],
            cf,
        );
    }

    #[test]
    fn branch_rules() {
        accepts(
            Field::Branch,
            &["main", "release/1.2", "feature/x-y_z", "v1"],
            gh,
        );
        rejects(
            Field::Branch,
            &[
                "  ",
                "a b",
                "a..b",
                "a~1",
                "a^",
                "a:b",
                "a?",
                "a*",
                "a[",
                "a\\b",
                "-x",
                "/x",
                ".x",
                "x/",
                "x.",
                "x.lock",
                "@",
                "a@{1}",
                "a//b",
                &"b".repeat(256),
            ],
            gh,
        );
    }

    #[test]
    fn duplicate_ids_reported_on_second_occurrence() {
        let drafts = vec![
            gh().with_id("same"),
            cf().with_id("other"),
            cf().with_id("same"),
        ];
        let err = validate_drafts(&drafts).unwrap_err();
        assert_eq!(err.issues().len(), 1);
        assert_eq!(
            err.to_string(),
            "watch 3 (\"Site\"): id \"same\" is already used by watch 1"
        );
        assert_eq!(err.for_field(&Field::Id).count(), 1);
        assert_eq!(err.for_field(&Field::Label).count(), 0);
    }

    #[test]
    fn list_issues_name_watch_by_position_and_label() {
        let mut bad = gh();
        bad.repo = Some(String::new());
        bad.label = Some("Keevoro".into());
        let mut nameless = cf();
        nameless.label = None;
        let err = validate_drafts(&[gh(), gh(), bad, nameless]).unwrap_err();
        let lines: Vec<String> = err.issues().iter().map(ToString::to_string).collect();
        assert_eq!(
            lines,
            vec![
                "watch 3 (\"Keevoro\"): repo must not be empty",
                "watch 4: label is required"
            ]
        );
    }

    #[test]
    fn labels_need_not_be_unique_and_order_is_kept() {
        let watches = validate_drafts(&[gh().with_id("1"), gh().with_id("2")]).unwrap();
        let ids: Vec<&str> = watches.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["1", "2"]);
    }

    #[test]
    fn alias_validation_helper() {
        assert_eq!(validate_alias(" work ").unwrap().as_str(), "work");
        assert!(validate_alias("").is_err());
        assert!(validate_alias("no spaces").is_err());
    }

    #[test]
    fn location_display() {
        assert_eq!(
            IssueLocation::Connection { alias: "w".into() }.to_string(),
            "connection \"w\""
        );
        assert_eq!(IssueLocation::Document.to_string(), "");
        assert_eq!(Field::Key("lable".into()).to_string(), "lable");
        assert_eq!(Field::DisplayName.key(), "display_name");
        assert_eq!(Field::Scale.key(), "scale");
    }
}
