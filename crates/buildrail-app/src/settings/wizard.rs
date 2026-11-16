//! The "Add build" wizard.
//!
//! ```text
//! ChooseProvider ─▶ Connect ─┬─ GitHub ─────▶ Repository ─▶ Workflow ─▶ Branch ─┐
//!                            └─ Cloudflare ─▶ Account ────▶ Worker ───▶ Branch ─┤
//!        Pinned ◀─ Label ◀─ Preview ◀────────────────────────────────────────────┘
//! ```
//!
//! * **Connect** offers this device's existing connections for the provider
//!   or a [`ConnectFlow`] for a new one.
//! * Each target step requests its list from a catalogue as a
//!   [`Command::Catalog`]; the list arrives as an [`Event::Catalog`].
//! * **Preview** asks for the latest run matching the exact target
//!   ([`Command::Preview`]; the controller uses the provider's `RunSource`
//!   and core's `selection::select_latest`). An empty history
//!   (`Ready(None)`, "No runs yet") may be pinned; an access error may not,
//!   and offers Reconnect instead. The two are never conflated.
//! * **Label** validates through `buildrail_profile::validate_draft`, the
//!   same validator the profile file uses, before [`Command::Pin`].
//!
//! [`WizardInput::Back`] always returns to the previous step; answers to
//! requests made on a step the user has left are ignored (tickets).

use buildrail_core::domain::{BuildRecord, ConnectionAlias, ProviderKind, WatchId, WatchTarget};
use buildrail_profile::{validate_draft, WatchDraft};
use buildrail_providers::catalog::{
    CloudflareAccount, CloudflareWorker, GitHubBranch, GitHubRepository, GitHubWorkflow,
};

use super::connect::{ConnectFlow, ConnectInput};
use super::{CatalogItems, CatalogQuery, Command, Event, Loadable, SettingsContext, Ticket};

/// Where the wizard is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// GitHub or Cloudflare.
    ChooseProvider,
    /// Pick an existing connection or connect a new one.
    Connect,
    /// GitHub: pick a repository.
    Repository,
    /// GitHub: pick a workflow.
    Workflow,
    /// Pick a branch filter (GitHub: from the list; Cloudflare: free text).
    Branch,
    /// Cloudflare: pick an account.
    Account,
    /// Cloudflare: pick a Worker.
    Worker,
    /// Confirm with the latest matching run.
    Preview,
    /// Choose the short label.
    Label,
    /// Done.
    Pinned,
}

/// User input for the wizard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WizardInput {
    /// Provider chosen.
    ChooseProvider(ProviderKind),
    /// Use an existing connection.
    UseConnection(ConnectionAlias),
    /// Connect a new account instead of using an existing connection.
    NewConnection,
    /// Input for the connect form.
    Connect(ConnectInput),
    /// Replace the credential of the connection in use (after an access
    /// error).
    Reconnect,
    /// GitHub repository chosen.
    SelectRepository(GitHubRepository),
    /// GitHub workflow chosen.
    SelectWorkflow(GitHubWorkflow),
    /// Cloudflare account chosen.
    SelectAccount(CloudflareAccount),
    /// Cloudflare Worker chosen.
    SelectWorker(CloudflareWorker),
    /// Branch filter changed; `None` (or blank) means every branch.
    SetBranch(Option<String>),
    /// Branch confirmed: go to the preview.
    ConfirmBranch,
    /// Repeat the failed request of the current step.
    Retry,
    /// Selection confirmed by the preview.
    ConfirmPreview,
    /// Label field changed.
    SetLabel(String),
    /// Pin the watch.
    Pin,
    /// Previous step.
    Back,
}

/// The Add-build wizard. See the module docs.
#[derive(Debug, Clone, PartialEq)]
pub struct AddBuildWizard {
    step: Step,
    provider: Option<ProviderKind>,
    connection: Option<ConnectionAlias>,
    connect: Option<ConnectFlow>,
    repositories: Loadable<Vec<GitHubRepository>>,
    repository: Option<GitHubRepository>,
    workflows: Loadable<Vec<GitHubWorkflow>>,
    workflow: Option<GitHubWorkflow>,
    branches: Loadable<Vec<GitHubBranch>>,
    branch: Option<String>,
    accounts: Loadable<Vec<CloudflareAccount>>,
    account: Option<CloudflareAccount>,
    workers: Loadable<Vec<CloudflareWorker>>,
    worker: Option<CloudflareWorker>,
    preview: Loadable<Option<BuildRecord>>,
    label: String,
    label_error: Option<String>,
    pin_ticket: Option<Ticket>,
    pinned: Option<WatchId>,
}

impl Default for AddBuildWizard {
    fn default() -> Self {
        Self::new()
    }
}

impl AddBuildWizard {
    /// A wizard at its first step.
    pub fn new() -> Self {
        Self {
            step: Step::ChooseProvider,
            provider: None,
            connection: None,
            connect: None,
            repositories: Loadable::Idle,
            repository: None,
            workflows: Loadable::Idle,
            workflow: None,
            branches: Loadable::Idle,
            branch: None,
            accounts: Loadable::Idle,
            account: None,
            workers: Loadable::Idle,
            worker: None,
            preview: Loadable::Idle,
            label: String::new(),
            label_error: None,
            pin_ticket: None,
            pinned: None,
        }
    }

    // ------------------------------------------------------------ views

    /// The current step.
    pub fn step(&self) -> Step {
        self.step
    }
    /// The chosen provider.
    pub fn provider(&self) -> Option<ProviderKind> {
        self.provider
    }
    /// The connection in use.
    pub fn connection(&self) -> Option<&ConnectionAlias> {
        self.connection.as_ref()
    }
    /// The connect form, when connecting a new account (or reconnecting).
    pub fn connect_flow(&self) -> Option<&ConnectFlow> {
        self.connect.as_ref()
    }
    /// GitHub repositories.
    pub fn repositories(&self) -> &Loadable<Vec<GitHubRepository>> {
        &self.repositories
    }
    /// GitHub workflows of the chosen repository.
    pub fn workflows(&self) -> &Loadable<Vec<GitHubWorkflow>> {
        &self.workflows
    }
    /// GitHub branches of the chosen repository.
    pub fn branches(&self) -> &Loadable<Vec<GitHubBranch>> {
        &self.branches
    }
    /// Cloudflare accounts.
    pub fn accounts(&self) -> &Loadable<Vec<CloudflareAccount>> {
        &self.accounts
    }
    /// Workers of the chosen account.
    pub fn workers(&self) -> &Loadable<Vec<CloudflareWorker>> {
        &self.workers
    }
    /// The branch filter (`None` = every branch).
    pub fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }
    /// The preview result.
    pub fn preview(&self) -> &Loadable<Option<BuildRecord>> {
        &self.preview
    }
    /// The label being entered.
    pub fn label(&self) -> &str {
        &self.label
    }
    /// Why the label (or pinning) was refused.
    pub fn label_error(&self) -> Option<&str> {
        self.label_error.as_deref()
    }
    /// `true` while the pin is being saved.
    pub fn is_pinning(&self) -> bool {
        self.pin_ticket.is_some()
    }
    /// The pinned watch, at [`Step::Pinned`].
    pub fn pinned(&self) -> Option<&WatchId> {
        self.pinned.as_ref()
    }

    /// One line describing the selection so far, e.g.
    /// `acme/keevoro · ci.yml · branch main`.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(repo) = &self.repository {
            parts.push(repo.full_name.clone());
        }
        if let Some(workflow) = &self.workflow {
            parts.push(workflow.watch_key());
        }
        if let Some(account) = &self.account {
            parts.push(account.name.clone());
        }
        if let Some(worker) = &self.worker {
            parts.push(format!("Worker {}", worker.name));
        }
        if matches!(self.step, Step::Preview | Step::Label | Step::Pinned) {
            parts.push(match &self.branch {
                Some(branch) => format!("branch {branch}"),
                None => "any branch".to_owned(),
            });
        }
        parts.join(" · ")
    }

    /// The target selected so far, once complete.
    pub fn target(&self) -> Option<WatchTarget> {
        match self.provider? {
            ProviderKind::GitHub => {
                let repo = self.repository.as_ref()?;
                Some(WatchTarget::GitHub {
                    owner: repo.owner.clone(),
                    repo: repo.name.clone(),
                    workflow: self.workflow.as_ref()?.watch_key(),
                    branch: self.branch.clone(),
                })
            }
            ProviderKind::Cloudflare => Some(WatchTarget::Cloudflare {
                account_id: self.account.as_ref()?.id.clone(),
                worker: self.worker.as_ref()?.name.clone(),
                branch: self.branch.clone(),
            }),
        }
    }

    /// The draft that will be pinned, once the target is complete.
    pub fn draft(&self) -> Option<WatchDraft> {
        let connection = self.connection.as_ref()?.as_str();
        Some(match self.target()? {
            WatchTarget::GitHub {
                owner,
                repo,
                workflow,
                branch,
            } => WatchDraft::github(&self.label, connection, owner, repo, workflow, branch),
            WatchTarget::Cloudflare {
                account_id,
                worker,
                branch,
            } => WatchDraft::cloudflare(&self.label, connection, account_id, worker, branch),
        })
    }

    // ------------------------------------------------------------ input

    /// Handles user input; returns the commands to execute.
    pub fn input(&mut self, input: WizardInput, ctx: SettingsContext<'_>) -> Vec<Command> {
        match input {
            WizardInput::ChooseProvider(provider) if self.step == Step::ChooseProvider => {
                self.provider = Some(provider);
                self.enter_connect(ctx);
                Vec::new()
            }
            WizardInput::UseConnection(alias) if self.step == Step::Connect => {
                let usable = self
                    .provider
                    .is_some_and(|p| ctx.connections.get(&alias).is_some_and(|c| c.provider == p));
                if !usable {
                    return Vec::new();
                }
                self.connection = Some(alias);
                self.connect = None;
                self.enter_first_target()
            }
            WizardInput::NewConnection if self.step == Step::Connect => {
                if let Some(provider) = self.provider {
                    self.connect = Some(ConnectFlow::new_connection(provider, ctx.connections));
                }
                Vec::new()
            }
            WizardInput::Connect(input) if self.step == Step::Connect => self
                .connect
                .as_mut()
                .map(|flow| flow.input(input, ctx.connections))
                .unwrap_or_default(),
            WizardInput::Reconnect => match (self.provider, &self.connection) {
                (Some(provider), Some(alias)) if self.step != Step::Pinned => {
                    self.connect = Some(ConnectFlow::reconnect(provider, alias));
                    self.step = Step::Connect;
                    Vec::new()
                }
                _ => Vec::new(),
            },
            WizardInput::SelectRepository(repo) if self.step == Step::Repository => {
                self.branch = repo.default_branch.clone();
                self.label = suggested_label(&repo.name);
                self.repository = Some(repo);
                self.workflow = None;
                self.step = Step::Workflow;
                self.load_workflows()
            }
            WizardInput::SelectWorkflow(workflow) if self.step == Step::Workflow => {
                self.workflow = Some(workflow);
                self.step = Step::Branch;
                self.load_branches()
            }
            WizardInput::SelectAccount(account) if self.step == Step::Account => {
                self.account = Some(account);
                self.worker = None;
                self.step = Step::Worker;
                self.load_workers()
            }
            WizardInput::SelectWorker(worker) if self.step == Step::Worker => {
                self.label = suggested_label(&worker.name);
                self.worker = Some(worker);
                self.branch = None;
                self.step = Step::Branch;
                Vec::new()
            }
            WizardInput::SetBranch(branch) if self.step == Step::Branch => {
                self.branch = branch
                    .map(|b| b.trim().to_owned())
                    .filter(|b| !b.is_empty());
                Vec::new()
            }
            WizardInput::ConfirmBranch if self.step == Step::Branch => {
                self.step = Step::Preview;
                self.load_preview()
            }
            WizardInput::Retry => self.retry(),
            WizardInput::ConfirmPreview
                if self.step == Step::Preview && self.preview.ready().is_some() =>
            {
                self.label_error = None;
                self.step = Step::Label;
                Vec::new()
            }
            WizardInput::SetLabel(label) if self.step == Step::Label => {
                self.label = label;
                self.label_error = None;
                Vec::new()
            }
            WizardInput::Pin if self.step == Step::Label && self.pin_ticket.is_none() => self.pin(),
            WizardInput::Back => {
                self.back();
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// Handles a controller event; returns follow-up commands.
    pub fn on_event(&mut self, event: &Event) -> Vec<Command> {
        if let Some(flow) = self.connect.as_mut() {
            if let Some(connection) = flow.on_event(event) {
                self.connection = Some(connection.alias);
                self.connect = None;
                return if self.step == Step::Connect {
                    self.enter_first_target()
                } else {
                    Vec::new()
                };
            }
        }
        match event {
            Event::Catalog { ticket, result } => {
                self.on_catalog(*ticket, result);
                Vec::new()
            }
            Event::Preview { ticket, result } if self.preview.awaits(*ticket) => {
                self.preview = match result {
                    Ok(record) => Loadable::Ready(record.clone()),
                    Err(error) => Loadable::Failed(error.clone()),
                };
                Vec::new()
            }
            Event::Pinned { ticket, result } if self.pin_ticket == Some(*ticket) => {
                self.pin_ticket = None;
                match result {
                    Ok(id) => {
                        self.pinned = Some(id.clone());
                        self.step = Step::Pinned;
                    }
                    Err(message) => self.label_error = Some(message.clone()),
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    // ---------------------------------------------------------- helpers

    fn enter_connect(&mut self, ctx: SettingsContext<'_>) {
        self.step = Step::Connect;
        self.connection = None;
        self.connect = match self.provider {
            Some(provider) if ctx.connections.values().all(|c| c.provider != provider) => {
                Some(ConnectFlow::new_connection(provider, ctx.connections))
            }
            _ => None,
        };
    }

    fn enter_first_target(&mut self) -> Vec<Command> {
        match self.provider {
            Some(ProviderKind::GitHub) => {
                self.step = Step::Repository;
                self.load_repositories()
            }
            Some(ProviderKind::Cloudflare) => {
                self.step = Step::Account;
                self.load_accounts()
            }
            None => Vec::new(),
        }
    }

    fn catalog(&self, query: CatalogQuery) -> Option<(Ticket, Command)> {
        let connection = self.connection.clone()?;
        let ticket = Ticket::next();
        Some((
            ticket,
            Command::Catalog {
                ticket,
                connection,
                query,
            },
        ))
    }

    fn load_repositories(&mut self) -> Vec<Command> {
        let Some((ticket, command)) = self.catalog(CatalogQuery::Repositories) else {
            return Vec::new();
        };
        self.repositories = Loadable::Loading(ticket);
        vec![command]
    }

    fn repo_query(&self, branches: bool) -> Option<CatalogQuery> {
        let repo = self.repository.as_ref()?;
        let (owner, repo) = (repo.owner.clone(), repo.name.clone());
        Some(if branches {
            CatalogQuery::Branches { owner, repo }
        } else {
            CatalogQuery::Workflows { owner, repo }
        })
    }

    fn load_workflows(&mut self) -> Vec<Command> {
        let Some((ticket, command)) = self.repo_query(false).and_then(|query| self.catalog(query))
        else {
            return Vec::new();
        };
        self.workflows = Loadable::Loading(ticket);
        vec![command]
    }

    fn load_branches(&mut self) -> Vec<Command> {
        if self.provider != Some(ProviderKind::GitHub) {
            return Vec::new();
        }
        let Some((ticket, command)) = self.repo_query(true).and_then(|query| self.catalog(query))
        else {
            return Vec::new();
        };
        self.branches = Loadable::Loading(ticket);
        vec![command]
    }

    fn load_accounts(&mut self) -> Vec<Command> {
        let Some((ticket, command)) = self.catalog(CatalogQuery::Accounts) else {
            return Vec::new();
        };
        self.accounts = Loadable::Loading(ticket);
        vec![command]
    }

    fn load_workers(&mut self) -> Vec<Command> {
        let Some(account_id) = self.account.as_ref().map(|a| a.id.clone()) else {
            return Vec::new();
        };
        let Some((ticket, command)) = self.catalog(CatalogQuery::Workers { account_id }) else {
            return Vec::new();
        };
        self.workers = Loadable::Loading(ticket);
        vec![command]
    }

    fn load_preview(&mut self) -> Vec<Command> {
        let (Some(connection), Some(target)) = (self.connection.clone(), self.target()) else {
            return Vec::new();
        };
        let ticket = Ticket::next();
        self.preview = Loadable::Loading(ticket);
        vec![Command::Preview {
            ticket,
            connection,
            target,
        }]
    }

    fn retry(&mut self) -> Vec<Command> {
        match self.step {
            Step::Repository => self.load_repositories(),
            Step::Workflow => self.load_workflows(),
            Step::Branch => self.load_branches(),
            Step::Account => self.load_accounts(),
            Step::Worker => self.load_workers(),
            Step::Preview => self.load_preview(),
            _ => Vec::new(),
        }
    }

    fn on_catalog(&mut self, ticket: Ticket, result: &Result<CatalogItems, super::LoadError>) {
        fn settle<T: Clone>(
            slot: &mut Loadable<Vec<T>>,
            result: &Result<CatalogItems, super::LoadError>,
            pick: impl Fn(&CatalogItems) -> Option<&Vec<T>>,
        ) {
            *slot = match result {
                Ok(items) => match pick(items) {
                    Some(items) => Loadable::Ready(items.clone()),
                    None => return,
                },
                Err(error) => Loadable::Failed(error.clone()),
            };
        }
        if self.repositories.awaits(ticket) {
            settle(&mut self.repositories, result, |i| match i {
                CatalogItems::Repositories(v) => Some(v),
                _ => None,
            });
        } else if self.workflows.awaits(ticket) {
            settle(&mut self.workflows, result, |i| match i {
                CatalogItems::Workflows(v) => Some(v),
                _ => None,
            });
        } else if self.branches.awaits(ticket) {
            settle(&mut self.branches, result, |i| match i {
                CatalogItems::Branches(v) => Some(v),
                _ => None,
            });
        } else if self.accounts.awaits(ticket) {
            settle(&mut self.accounts, result, |i| match i {
                CatalogItems::Accounts(v) => Some(v),
                _ => None,
            });
        } else if self.workers.awaits(ticket) {
            settle(&mut self.workers, result, |i| match i {
                CatalogItems::Workers(v) => Some(v),
                _ => None,
            });
        }
    }

    fn pin(&mut self) -> Vec<Command> {
        let Some(draft) = self.draft() else {
            return Vec::new();
        };
        if let Err(errors) = validate_draft(&draft) {
            let messages: Vec<String> = errors
                .issues()
                .iter()
                .map(|issue| capitalise(&issue.message))
                .collect();
            self.label_error = Some(messages.join("\n"));
            return Vec::new();
        }
        let ticket = Ticket::next();
        self.pin_ticket = Some(ticket);
        self.label_error = None;
        vec![Command::Pin { ticket, draft }]
    }

    fn back(&mut self) {
        // Leaving a step abandons whatever it was waiting for.
        let previous = match (self.step, self.provider) {
            (Step::ChooseProvider | Step::Pinned, _) => return,
            (Step::Connect, _) => Step::ChooseProvider,
            (Step::Repository | Step::Account, _) => Step::Connect,
            (Step::Workflow, _) => Step::Repository,
            (Step::Branch, Some(ProviderKind::GitHub)) => Step::Workflow,
            (Step::Branch, _) => Step::Worker,
            (Step::Worker, _) => Step::Account,
            (Step::Preview, _) => Step::Branch,
            (Step::Label, _) => Step::Preview,
        };
        self.pin_ticket = None;
        match self.step {
            Step::Connect => {
                self.connect = None;
                self.provider = None;
            }
            Step::Repository | Step::Account => {
                self.connect = None;
                self.connection = None;
                self.repositories = Loadable::Idle;
                self.accounts = Loadable::Idle;
            }
            Step::Workflow => self.workflows = Loadable::Idle,
            Step::Branch => self.branches = Loadable::Idle,
            Step::Worker => self.workers = Loadable::Idle,
            Step::Preview => self.preview = Loadable::Idle,
            _ => {}
        }
        self.step = previous;
    }
}

/// A label suggestion from a repository or Worker name: first letter
/// upper-cased (`keevoro` → `Keevoro`).
pub fn suggested_label(name: &str) -> String {
    capitalise(name)
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

    use buildrail_core::domain::{BuildStatus, RunId};
    use buildrail_profile::{LocalConnections, Profile};
    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::settings::connect::{ConnectError, NewConnection};
    use crate::settings::test_support::connections;
    use crate::settings::LoadError;

    struct Fixture {
        profile: Profile,
        connections: LocalConnections,
        wizard: AddBuildWizard,
    }

    impl Fixture {
        fn new(conns: &[(&str, ProviderKind)]) -> Self {
            Self {
                profile: Profile::empty(),
                connections: connections(conns),
                wizard: AddBuildWizard::new(),
            }
        }

        fn input(&mut self, input: WizardInput) -> Vec<Command> {
            let ctx = SettingsContext {
                profile: &self.profile,
                connections: &self.connections,
            };
            self.wizard.input(input, ctx)
        }

        fn only(&mut self, input: WizardInput) -> Command {
            let mut commands = self.input(input);
            assert_eq!(commands.len(), 1, "{commands:?}");
            commands.remove(0)
        }
    }

    fn ticket_of(command: &Command) -> Ticket {
        match command {
            Command::Catalog { ticket, .. }
            | Command::Preview { ticket, .. }
            | Command::Pin { ticket, .. }
            | Command::Connect { ticket, .. } => *ticket,
            other => panic!("unexpected {other:?}"),
        }
    }

    fn catalog(command: &Command, items: CatalogItems) -> Event {
        Event::Catalog {
            ticket: ticket_of(command),
            result: Ok(items),
        }
    }

    fn repo() -> GitHubRepository {
        GitHubRepository {
            owner: "acme".into(),
            name: "keevoro".into(),
            full_name: "acme/keevoro".into(),
            private: true,
            archived: false,
            default_branch: Some("main".into()),
        }
    }

    fn workflow() -> GitHubWorkflow {
        GitHubWorkflow {
            id: 7,
            name: "CI".into(),
            path: ".github/workflows/ci.yml".into(),
            state: "active".into(),
        }
    }

    fn run() -> BuildRecord {
        let at = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        BuildRecord {
            run_id: RunId::new("1"),
            attempt: 1,
            status: BuildStatus::Passed,
            url: "https://github.com/acme/keevoro/actions/runs/1".into(),
            branch: Some("main".into()),
            commit: Some("abc".into()),
            failing_job: None,
            created_at: at,
            updated_at: at,
            observed_at: at,
        }
    }

    #[test]
    fn github_happy_path_with_existing_connection() {
        let mut f = Fixture::new(&[("work", ProviderKind::GitHub)]);
        assert_eq!(f.wizard.step(), Step::ChooseProvider);
        assert!(f
            .input(WizardInput::ChooseProvider(ProviderKind::GitHub))
            .is_empty());
        assert_eq!(f.wizard.step(), Step::Connect);
        assert!(
            f.wizard.connect_flow().is_none(),
            "existing connection offered"
        );
        // A connection of another provider or unknown alias is refused.
        assert!(f
            .input(WizardInput::UseConnection(ConnectionAlias::new("nope")))
            .is_empty());

        let load = f.only(WizardInput::UseConnection(ConnectionAlias::new("work")));
        assert!(matches!(
            &load,
            Command::Catalog { query: CatalogQuery::Repositories, connection, .. } if connection.as_str() == "work"
        ));
        assert_eq!(f.wizard.step(), Step::Repository);
        assert!(f.wizard.repositories().is_loading());
        f.wizard
            .on_event(&catalog(&load, CatalogItems::Repositories(vec![repo()])));
        assert_eq!(f.wizard.repositories().ready().unwrap().len(), 1);

        let load = f.only(WizardInput::SelectRepository(repo()));
        assert!(
            matches!(&load, Command::Catalog { query: CatalogQuery::Workflows { owner, repo }, .. } if owner == "acme" && repo == "keevoro")
        );
        // A mismatched answer kind leaves the list loading.
        f.wizard
            .on_event(&catalog(&load, CatalogItems::Accounts(vec![])));
        assert!(f.wizard.workflows().is_loading());
        f.wizard
            .on_event(&catalog(&load, CatalogItems::Workflows(vec![workflow()])));

        let load = f.only(WizardInput::SelectWorkflow(workflow()));
        assert_eq!(f.wizard.step(), Step::Branch);
        assert_eq!(
            f.wizard.branch(),
            Some("main"),
            "default branch preselected"
        );
        f.wizard.on_event(&catalog(
            &load,
            CatalogItems::Branches(vec![GitHubBranch {
                name: "main".into(),
                protected: true,
            }]),
        ));
        assert_eq!(f.wizard.branches().ready().unwrap().len(), 1);
        f.input(WizardInput::SetBranch(Some("  ".into())));
        assert_eq!(f.wizard.branch(), None, "blank means any branch");
        f.input(WizardInput::SetBranch(Some("main".into())));

        let preview = f.only(WizardInput::ConfirmBranch);
        let Command::Preview { target, .. } = &preview else {
            panic!()
        };
        assert_eq!(
            target,
            &WatchTarget::GitHub {
                owner: "acme".into(),
                repo: "keevoro".into(),
                workflow: "ci.yml".into(),
                branch: Some("main".into()),
            }
        );
        assert_eq!(f.wizard.summary(), "acme/keevoro · ci.yml · branch main");
        // Cannot confirm before the preview answered.
        f.input(WizardInput::ConfirmPreview);
        assert_eq!(f.wizard.step(), Step::Preview);
        f.wizard.on_event(&Event::Preview {
            ticket: ticket_of(&preview),
            result: Ok(Some(run())),
        });
        assert_eq!(f.wizard.preview().ready(), Some(&Some(run())));
        f.input(WizardInput::ConfirmPreview);
        assert_eq!(f.wizard.step(), Step::Label);
        assert_eq!(f.wizard.label(), "Keevoro");

        // Invalid label: refused by the shared validator, no command.
        f.input(WizardInput::SetLabel("   ".into()));
        assert!(f.input(WizardInput::Pin).is_empty());
        assert_eq!(f.wizard.label_error(), Some("Label must not be empty"));

        f.input(WizardInput::SetLabel("Keevoro CI".into()));
        let pin = f.only(WizardInput::Pin);
        let Command::Pin { draft, .. } = &pin else {
            panic!()
        };
        assert_eq!(draft.label.as_deref(), Some("Keevoro CI"));
        assert_eq!(draft.connection.as_deref(), Some("work"));
        assert!(f.wizard.is_pinning());
        assert!(f.input(WizardInput::Pin).is_empty(), "no double pin");

        // A save failure is shown and the wizard stays on Label.
        f.wizard.on_event(&Event::Pinned {
            ticket: ticket_of(&pin),
            result: Err("disk full".into()),
        });
        assert_eq!(f.wizard.label_error(), Some("disk full"));
        let pin = f.only(WizardInput::Pin);
        f.wizard.on_event(&Event::Pinned {
            ticket: ticket_of(&pin),
            result: Ok(WatchId::new("new")),
        });
        assert_eq!(f.wizard.step(), Step::Pinned);
        assert_eq!(f.wizard.pinned().unwrap().as_str(), "new");
        f.input(WizardInput::Back);
        assert_eq!(f.wizard.step(), Step::Pinned, "no way back from pinned");
    }

    #[test]
    fn cloudflare_with_new_connection_and_empty_history() {
        let mut f = Fixture::new(&[]);
        f.input(WizardInput::ChooseProvider(ProviderKind::Cloudflare));
        assert!(
            f.wizard.connect_flow().is_some(),
            "no connection yet → form shown"
        );
        f.input(WizardInput::Connect(ConnectInput::SetToken(
            "cf-token".into(),
        )));
        let connect = f.only(WizardInput::Connect(ConnectInput::Submit));
        assert!(f.wizard.connect_flow().unwrap().is_busy());
        let follow = f.wizard.on_event(&Event::Connected {
            ticket: ticket_of(&connect),
            result: Ok(NewConnection {
                alias: ConnectionAlias::new("cloudflare"),
                provider: ProviderKind::Cloudflare,
                display_name: "token".into(),
            }),
        });
        assert_eq!(follow.len(), 1);
        assert_eq!(f.wizard.step(), Step::Account);
        assert_eq!(f.wizard.connection().unwrap().as_str(), "cloudflare");
        let account = CloudflareAccount {
            id: "0123456789abcdef0123456789abcdef".into(),
            name: "Acme".into(),
        };
        f.wizard.on_event(&catalog(
            &follow[0],
            CatalogItems::Accounts(vec![account.clone()]),
        ));
        assert_eq!(f.wizard.accounts().ready().unwrap().len(), 1);
        let load = f.only(WizardInput::SelectAccount(account));
        let worker = CloudflareWorker {
            name: "site".into(),
            tag: None,
            modified_on: None,
        };
        f.wizard
            .on_event(&catalog(&load, CatalogItems::Workers(vec![worker.clone()])));
        assert_eq!(f.wizard.workers().ready().unwrap().len(), 1);
        assert!(f.input(WizardInput::SelectWorker(worker)).is_empty());
        assert_eq!(f.wizard.step(), Step::Branch);
        let preview = f.only(WizardInput::ConfirmBranch);
        f.wizard.on_event(&Event::Preview {
            ticket: ticket_of(&preview),
            result: Ok(None),
        });
        assert_eq!(
            f.wizard.preview(),
            &Loadable::Ready(None),
            "empty history is Ready(None)"
        );
        assert_eq!(f.wizard.summary(), "Acme · Worker site · any branch");
        f.input(WizardInput::ConfirmPreview);
        assert_eq!(f.wizard.step(), Step::Label, "empty history can be pinned");
        assert_eq!(f.wizard.label(), "Site");
        let Command::Pin { draft, .. } = f.only(WizardInput::Pin) else {
            panic!()
        };
        assert_eq!(draft.worker.as_deref(), Some("site"));
        assert_eq!(draft.branch, None);
    }

    #[test]
    fn access_error_blocks_and_offers_reconnect() {
        let mut f = Fixture::new(&[("work", ProviderKind::GitHub)]);
        f.input(WizardInput::ChooseProvider(ProviderKind::GitHub));
        let load = f.only(WizardInput::UseConnection(ConnectionAlias::new("work")));
        f.wizard.on_event(&Event::Catalog {
            ticket: ticket_of(&load),
            result: Err(LoadError::Access("denied".into())),
        });
        assert!(matches!(f.wizard.repositories(), Loadable::Failed(e) if e.is_access()));
        // Retry re-requests.
        let retry = f.only(WizardInput::Retry);
        assert!(matches!(
            retry,
            Command::Catalog {
                query: CatalogQuery::Repositories,
                ..
            }
        ));
        // Reconnect replaces the credential of the same alias.
        assert!(f.input(WizardInput::Reconnect).is_empty());
        assert_eq!(f.wizard.step(), Step::Connect);
        let flow = f.wizard.connect_flow().unwrap();
        let ConnectFlow::Token(form) = flow;
        assert!(form.alias_locked);
        assert_eq!(form.alias, "work");
        // A failed reconnect stays on the form with a message.
        f.input(WizardInput::Connect(ConnectInput::SetToken("t".into())));
        let c = f.only(WizardInput::Connect(ConnectInput::Submit));
        assert!(f
            .wizard
            .on_event(&Event::Connected {
                ticket: ticket_of(&c),
                result: Err(ConnectError::Store("locked".into())),
            })
            .is_empty());
        assert!(f.wizard.connect_flow().unwrap().error().is_some());
    }

    #[test]
    fn preview_access_error_is_not_pinnable_and_stale_answers_are_ignored() {
        let mut f = Fixture::new(&[("work", ProviderKind::GitHub)]);
        f.input(WizardInput::ChooseProvider(ProviderKind::GitHub));
        let first = f.only(WizardInput::UseConnection(ConnectionAlias::new("work")));
        f.wizard
            .on_event(&catalog(&first, CatalogItems::Repositories(vec![repo()])));
        let wf = f.only(WizardInput::SelectRepository(repo()));
        f.wizard
            .on_event(&catalog(&wf, CatalogItems::Workflows(vec![workflow()])));
        let branches = f.only(WizardInput::SelectWorkflow(workflow()));
        // Leave the branch step before its answer arrives.
        f.input(WizardInput::Back);
        assert_eq!(f.wizard.step(), Step::Workflow);
        f.wizard
            .on_event(&catalog(&branches, CatalogItems::Branches(vec![])));
        assert_eq!(f.wizard.branches(), &Loadable::Idle, "stale answer ignored");
        f.input(WizardInput::SelectWorkflow(workflow()));
        let preview = f.only(WizardInput::ConfirmBranch);
        f.wizard.on_event(&Event::Preview {
            ticket: ticket_of(&preview),
            result: Err(LoadError::Access("denied".into())),
        });
        f.input(WizardInput::ConfirmPreview);
        assert_eq!(
            f.wizard.step(),
            Step::Preview,
            "access error blocks pinning"
        );
        // Back walks all the way to the start, clearing state.
        for expected in [
            Step::Branch,
            Step::Workflow,
            Step::Repository,
            Step::Connect,
            Step::ChooseProvider,
        ] {
            f.input(WizardInput::Back);
            assert_eq!(f.wizard.step(), expected);
        }
        assert_eq!(f.wizard.provider(), None);
        f.input(WizardInput::Back);
        assert_eq!(f.wizard.step(), Step::ChooseProvider);
    }

    #[test]
    fn cloudflare_back_path_and_ignored_inputs() {
        let mut f = Fixture::new(&[("cf", ProviderKind::Cloudflare)]);
        // Inputs for the wrong step do nothing.
        assert!(f.input(WizardInput::ConfirmBranch).is_empty());
        assert!(f.input(WizardInput::Retry).is_empty());
        assert!(f.input(WizardInput::Reconnect).is_empty());
        assert!(f.input(WizardInput::NewConnection).is_empty());
        assert_eq!(f.wizard.step(), Step::ChooseProvider);
        assert!(f.wizard.target().is_none());
        assert!(f.wizard.draft().is_none());

        f.input(WizardInput::ChooseProvider(ProviderKind::Cloudflare));
        f.input(WizardInput::NewConnection);
        assert!(f.wizard.connect_flow().is_some());
        f.only(WizardInput::UseConnection(ConnectionAlias::new("cf")));
        let account = CloudflareAccount {
            id: "0123456789abcdef0123456789abcdef".into(),
            name: "Acme".into(),
        };
        f.only(WizardInput::SelectAccount(account));
        f.input(WizardInput::SelectWorker(CloudflareWorker {
            name: "api".into(),
            tag: None,
            modified_on: None,
        }));
        f.input(WizardInput::SetBranch(Some("prod".into())));
        assert_eq!(f.wizard.branch(), Some("prod"));
        assert!(
            f.input(WizardInput::Retry).is_empty(),
            "no branch list for Cloudflare"
        );
        f.only(WizardInput::ConfirmBranch);
        let retry = f.only(WizardInput::Retry);
        assert!(matches!(retry, Command::Preview { .. }));
        for expected in [
            Step::Branch,
            Step::Worker,
            Step::Account,
            Step::Connect,
            Step::ChooseProvider,
        ] {
            f.input(WizardInput::Back);
            assert_eq!(f.wizard.step(), expected);
        }
        assert_eq!(AddBuildWizard::default().step(), Step::ChooseProvider);
        assert_eq!(suggested_label(""), "");
    }
}
