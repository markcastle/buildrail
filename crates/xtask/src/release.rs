//! Cutting a release: the one place it happens.
//!
//! The Release workflow runs this, and so can a person, so the two routes
//! cannot drift apart. In order it:
//!
//! 1. refuses if the working tree has uncommitted changes;
//! 2. fetches tags, so it counts from releases cut on GitHub and not from
//!    the last one this checkout happens to know about;
//! 3. checks the changelog's shape;
//! 4. works out the version from the highest `v*` tag and the changelog,
//!    unless one was given, and refuses one that is not higher than every
//!    tag or is tagged already;
//! 5. stamps the changelog and sets `[workspace.package] version`;
//! 6. brings `Cargo.lock` into line with `cargo update --workspace`;
//! 7. commits `Release vX.Y.Z` and creates the annotated tag `vX.Y.Z`.
//!
//! It never pushes. With `dry_run` it stops after step 5 having written
//! nothing, and says what it would have done.
//!
//! Git and cargo are reached through [`Runner`] and files through [`Files`],
//! so the whole flow is tested against fakes.

use std::io::Write;

use crate::changelog;
use crate::date::Date;
use crate::manifest;
use crate::version::{highest_tag, Version};

/// Paths the release edits, relative to the repository root.
pub const CHANGELOG: &str = "CHANGELOG.md";
/// The workspace manifest that holds the version.
pub const CARGO_TOML: &str = "Cargo.toml";
/// The lock file, which records every workspace crate's version too.
pub const CARGO_LOCK: &str = "Cargo.lock";

/// What a finished command said.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Output {
    /// Whether it exited with status 0.
    pub success: bool,
    /// Its standard output.
    pub stdout: String,
    /// Its standard error.
    pub stderr: String,
}

/// Runs external programs (git and cargo).
pub trait Runner {
    /// Runs `program` with `args` in the repository root and waits for it.
    /// An `Err` means it could not be started at all.
    fn run(&mut self, program: &str, args: &[&str]) -> std::io::Result<Output>;
}

/// Reads and writes files in the repository.
pub trait Files {
    /// The whole file as text.
    fn read(&self, path: &str) -> std::io::Result<String>;
    /// Replaces the file's contents.
    fn write(&mut self, path: &str, contents: &str) -> std::io::Result<()>;
    /// Adds to the end of the file, creating it if needed.
    fn append(&mut self, path: &str, contents: &str) -> std::io::Result<()>;
}

/// How to cut the release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    /// The version to release; `None` lets the changelog decide.
    pub version: Option<Version>,
    /// Say what would happen and change nothing.
    pub dry_run: bool,
    /// The date the changelog section gets.
    pub date: Date,
    /// A file to append `version=`, `why=` and `previous=` lines to, which
    /// is how the workflow reads the outcome (`$GITHUB_OUTPUT`).
    pub github_output: Option<String>,
}

/// What was (or would be) released.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The new version.
    pub version: Version,
    /// Why that number.
    pub why: String,
    /// The highest release tag before it, if any.
    pub previous: Option<Version>,
}

fn git(runner: &mut dyn Runner, args: &[&str]) -> Result<Output, String> {
    runner
        .run("git", args)
        .map_err(|e| format!("Could not run git {}: {e}", args.join(" ")))
}

fn git_ok(runner: &mut dyn Runner, args: &[&str]) -> Result<String, String> {
    let out = git(runner, args)?;
    if out.success {
        Ok(out.stdout)
    } else {
        Err(format!(
            "git {} failed: {}",
            args.join(" "),
            out.stderr.trim()
        ))
    }
}

fn read(files: &dyn Files, path: &str) -> Result<String, String> {
    files
        .read(path)
        .map_err(|e| format!("Could not read {path}: {e}"))
}

fn write(files: &mut dyn Files, path: &str, contents: &str) -> Result<(), String> {
    files
        .write(path, contents)
        .map_err(|e| format!("Could not write {path}: {e}"))
}

fn say(log: &mut dyn Write, line: &str) {
    // A log line that cannot be written is not worth failing a release for.
    let _ = writeln!(log, "{line}");
}

/// Every `v*` tag this checkout knows.
pub fn tags(runner: &mut dyn Runner) -> Result<Vec<String>, String> {
    Ok(git_ok(runner, &["tag", "--list", "v*"])?
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect())
}

/// Cuts the release (or, with `dry_run`, describes it). See the module
/// documentation for the steps.
pub fn cut(
    options: &Options,
    runner: &mut dyn Runner,
    files: &mut dyn Files,
    log: &mut dyn Write,
) -> Result<Plan, String> {
    let status = git_ok(runner, &["status", "--porcelain"])?;
    if !status.trim().is_empty() {
        return Err(format!(
            "The working tree has uncommitted changes:\n{}\nCommit or stash them first; a \
             release commit should carry only the release.",
            status.trim_end()
        ));
    }

    let fetched = git(runner, &["fetch", "--tags", "--force", "--quiet"])?;
    if !fetched.success {
        say(
            log,
            &format!(
                "Could not fetch tags ({}); counting from the tags already here.",
                fetched.stderr.trim()
            ),
        );
    }
    let tags = tags(runner)?;
    let previous = highest_tag(tags.iter().map(String::as_str));

    let text = read(files, CHANGELOG)?;
    let problems = changelog::check(&text);
    if !problems.is_empty() {
        return Err(format!(
            "CHANGELOG.md needs fixing first:\n- {}",
            problems.join("\n- ")
        ));
    }

    let (version, why) = match options.version {
        Some(v) => (v, "the version was given".to_string()),
        None => {
            let bump = changelog::next_version(&text, previous.unwrap_or(Version::new(0, 0, 0)))?;
            (bump.version, bump.why)
        }
    };
    if tags.iter().any(|t| t.trim() == format!("v{version}")) {
        return Err(format!("v{version} already exists. Pick a higher version."));
    }
    if let Some(previous) = previous {
        if version <= previous {
            return Err(format!(
                "{version} is not higher than {previous}, the newest release. Installed copies \
                 only ever update to a higher version, so a release must go up."
            ));
        }
    }

    let stamped = changelog::release(&text, version, options.date)?;
    let cargo = read(files, CARGO_TOML)?;
    let bumped = manifest::set_workspace_version(&cargo, version)?;

    let plan = Plan {
        version,
        why,
        previous,
    };
    let after = previous.map_or("the first release".to_string(), |p| format!("after {p}"));
    say(
        log,
        &format!("Releasing BuildRail {version}, {after}: {}.", plan.why),
    );
    if let Some(path) = &options.github_output {
        let previous = previous.map(|p| p.to_string()).unwrap_or_default();
        files
            .append(
                path,
                &format!("version={version}\nwhy={}\nprevious={previous}\n", plan.why),
            )
            .map_err(|e| format!("Could not write {path}: {e}"))?;
    }

    if options.dry_run {
        say(log, "Dry run. A real run would:");
        say(
            log,
            &format!(
                "  - date Unreleased in CHANGELOG.md as [{version}] - {}",
                options.date
            ),
        );
        say(
            log,
            &format!("  - set [workspace.package] version = \"{version}\" in Cargo.toml"),
        );
        say(log, "  - run cargo update --workspace so Cargo.lock agrees");
        say(
            log,
            &format!("  - commit \"Release v{version}\" and tag v{version}"),
        );
        say(log, "Nothing was changed.");
        return Ok(plan);
    }

    let lock = files.read(CARGO_LOCK).ok();
    write(files, CHANGELOG, &stamped)?;
    write(files, CARGO_TOML, &bumped)?;
    if let Err(error) = update_lock(runner, log) {
        // Put the files back so a failed release leaves nothing behind.
        write(files, CHANGELOG, &text)?;
        write(files, CARGO_TOML, &cargo)?;
        if let Some(lock) = lock {
            write(files, CARGO_LOCK, &lock)?;
        }
        return Err(error);
    }

    git_ok(runner, &["add", "--", CHANGELOG, CARGO_TOML, CARGO_LOCK])?;
    let message = format!("Release v{version}");
    git_ok(runner, &["commit", "--quiet", "-m", &message])?;
    let tag = format!("v{version}");
    let tag_message = format!("BuildRail v{version}");
    git_ok(runner, &["tag", "-a", &tag, "-m", &tag_message])?;
    say(
        log,
        &format!(
            "Committed \"{message}\" and tagged {tag}. Nothing was pushed.\n\
             The Release workflow pushes and builds; a tag pushed by hand builds nothing.\n\
             To undo here: git tag -d {tag} && git reset --hard HEAD~1"
        ),
    );
    Ok(plan)
}

/// `cargo update --workspace` touches only the workspace's own crates in
/// `Cargo.lock`. Offline first, which needs nothing but the local index
/// cache that building xtask has just filled; online if that is not enough.
fn update_lock(runner: &mut dyn Runner, log: &mut dyn Write) -> Result<(), String> {
    let offline = runner
        .run("cargo", &["update", "--workspace", "--offline"])
        .map_err(|e| format!("Could not run cargo: {e}"))?;
    if offline.success {
        return Ok(());
    }
    say(log, "cargo update --offline was not enough; trying online.");
    let online = runner
        .run("cargo", &["update", "--workspace"])
        .map_err(|e| format!("Could not run cargo: {e}"))?;
    if online.success {
        Ok(())
    } else {
        Err(format!(
            "cargo update --workspace failed, so Cargo.lock would disagree with Cargo.toml: {}",
            online.stderr.trim()
        ))
    }
}

#[cfg(test)]
pub(crate) mod fakes {
    //! In-memory stand-ins for git, cargo and the file system.

    use std::collections::BTreeMap;
    use std::io;

    use super::{Files, Output, Runner};

    /// Answers commands from a script and records every call.
    #[derive(Default)]
    pub struct FakeRunner {
        /// `"git status --porcelain"` → what it answers.
        pub answers: BTreeMap<String, io::Result<Output>>,
        /// Every command run, in order.
        pub calls: Vec<String>,
    }

    pub fn ok(stdout: &str) -> io::Result<Output> {
        Ok(Output {
            success: true,
            stdout: stdout.to_string(),
            stderr: String::new(),
        })
    }

    pub fn fail(stderr: &str) -> io::Result<Output> {
        Ok(Output {
            success: false,
            stdout: String::new(),
            stderr: stderr.to_string(),
        })
    }

    impl FakeRunner {
        pub fn answer(&mut self, command: &str, output: io::Result<Output>) {
            self.answers.insert(command.to_string(), output);
        }
    }

    impl Runner for FakeRunner {
        fn run(&mut self, program: &str, args: &[&str]) -> io::Result<Output> {
            let command = format!("{program} {}", args.join(" "));
            self.calls.push(command.clone());
            match self.answers.get(&command) {
                Some(Ok(out)) => Ok(out.clone()),
                Some(Err(e)) => Err(io::Error::new(e.kind(), e.to_string())),
                None => ok(""),
            }
        }
    }

    /// Files in a map.
    #[derive(Default)]
    pub struct FakeFiles {
        pub files: BTreeMap<String, String>,
        pub fail_writes: bool,
    }

    impl Files for FakeFiles {
        fn read(&self, path: &str) -> io::Result<String> {
            self.files
                .get(path)
                .cloned()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such file"))
        }
        fn write(&mut self, path: &str, contents: &str) -> io::Result<()> {
            if self.fail_writes {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "read-only"));
            }
            self.files.insert(path.to_string(), contents.to_string());
            Ok(())
        }
        fn append(&mut self, path: &str, contents: &str) -> io::Result<()> {
            if self.fail_writes {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "read-only"));
            }
            self.files
                .entry(path.to_string())
                .or_default()
                .push_str(contents);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::io;

    use super::fakes::{fail, ok, FakeFiles, FakeRunner};
    use super::*;

    const REPO: &str = changelog::REPO_URL;

    fn changelog_with(unreleased: &str) -> String {
        format!(
            "# Changelog\n\n## [Unreleased]\n\n{unreleased}\n\n## [0.2.0] - 2026-11-17\n\n### Changed\n\n- Restyled.\n\n\
             [Unreleased]: {REPO}/compare/v0.2.0...HEAD\n[0.2.0]: {REPO}/releases/tag/v0.2.0\n"
        )
    }

    const CARGO: &str = "[workspace]\nmembers = [\"crates/*\"]\n\n[workspace.package]\nversion = \"0.2.0\" # here\n";

    fn setup(unreleased: &str, tags: &str) -> (FakeRunner, FakeFiles) {
        let mut runner = FakeRunner::default();
        runner.answer("git tag --list v*", ok(tags));
        let mut files = FakeFiles::default();
        files
            .files
            .insert(CHANGELOG.into(), changelog_with(unreleased));
        files.files.insert(CARGO_TOML.into(), CARGO.into());
        files.files.insert(CARGO_LOCK.into(), "lock v0.2.0".into());
        (runner, files)
    }

    fn options(version: Option<&str>, dry_run: bool) -> Options {
        Options {
            version: version.map(|v| Version::parse(v).unwrap()),
            dry_run,
            date: Date::parse("2026-11-20").unwrap(),
            github_output: None,
        }
    }

    fn run(
        options: &Options,
        runner: &mut FakeRunner,
        files: &mut FakeFiles,
    ) -> (Result<Plan, String>, String) {
        let mut log = Vec::new();
        let result = cut(options, runner, files, &mut log);
        (result, String::from_utf8(log).unwrap())
    }

    #[test]
    fn cuts_a_minor_release_from_the_changelog() {
        let (mut runner, mut files) = setup("### Added\n\n- Updates itself.", "v0.1.0\nv0.2.0\n");
        let (result, log) = run(&options(None, false), &mut runner, &mut files);
        let plan = result.unwrap();
        assert_eq!(plan.version, Version::new(0, 3, 0));
        assert_eq!(plan.previous, Some(Version::new(0, 2, 0)));
        assert!(plan.why.contains("Added"));
        assert_eq!(
            runner.calls,
            [
                "git status --porcelain",
                "git fetch --tags --force --quiet",
                "git tag --list v*",
                "cargo update --workspace --offline",
                "git add -- CHANGELOG.md Cargo.toml Cargo.lock",
                "git commit --quiet -m Release v0.3.0",
                "git tag -a v0.3.0 -m BuildRail v0.3.0",
            ]
        );
        assert!(files.files[CHANGELOG]
            .contains("## [0.3.0] - 2026-11-20\n\n### Added\n\n- Updates itself."));
        assert!(files.files[CARGO_TOML].contains("version = \"0.3.0\" # here\n"));
        assert!(log.contains("Releasing BuildRail 0.3.0, after 0.2.0: a minor release"));
        assert!(log.contains("Nothing was pushed"));
    }

    #[test]
    fn counts_from_the_highest_tag_not_the_last_listed() {
        let (mut runner, mut files) = setup("### Fixed\n\n- A bug.", "v0.10.0\nv0.9.0\nv0.2.0\n");
        let (result, _) = run(&options(None, true), &mut runner, &mut files);
        assert_eq!(result.unwrap().version, Version::new(0, 10, 1));
    }

    #[test]
    fn a_given_version_is_used() {
        let (mut runner, mut files) = setup("### Fixed\n\n- A bug.", "v0.2.0\n");
        let (result, _) = run(&options(Some("1.0.0"), false), &mut runner, &mut files);
        let plan = result.unwrap();
        assert_eq!(plan.version, Version::new(1, 0, 0));
        assert_eq!(plan.why, "the version was given");
        assert!(runner
            .calls
            .contains(&"git tag -a v1.0.0 -m BuildRail v1.0.0".to_string()));
    }

    #[test]
    fn the_first_release_counts_from_nothing() {
        let (mut runner, mut files) = setup("### Added\n\n- A.", "");
        let mut log_text = changelog_with("### Added\n\n- A.");
        log_text = log_text
            .replace("## [0.2.0] - 2026-11-17\n\n### Changed\n\n- Restyled.\n\n", "")
            .replace(&format!("[Unreleased]: {REPO}/compare/v0.2.0...HEAD\n[0.2.0]: {REPO}/releases/tag/v0.2.0\n"), &format!("[Unreleased]: {REPO}/commits/HEAD\n"));
        files.files.insert(CHANGELOG.into(), log_text);
        let (result, log) = run(&options(None, true), &mut runner, &mut files);
        let plan = result.unwrap();
        assert_eq!(plan.version, Version::new(0, 1, 0));
        assert_eq!(plan.previous, None);
        assert!(log.contains("the first release"));
    }

    #[test]
    fn a_dry_run_changes_nothing() {
        let (mut runner, mut files) = setup("### Fixed\n\n- A bug.", "v0.2.0\n");
        let before = files.files.clone();
        let (result, log) = run(&options(None, true), &mut runner, &mut files);
        assert_eq!(result.unwrap().version, Version::new(0, 2, 1));
        assert_eq!(files.files, before);
        assert_eq!(
            runner.calls,
            [
                "git status --porcelain",
                "git fetch --tags --force --quiet",
                "git tag --list v*"
            ]
        );
        assert!(log.contains("Dry run"));
        assert!(log.contains("Nothing was changed."));
    }

    #[test]
    fn writes_the_outcome_for_the_workflow() {
        let (mut runner, mut files) = setup("### Fixed\n\n- A bug.", "v0.2.0\n");
        let mut opts = options(None, true);
        opts.github_output = Some("out.txt".into());
        files.files.insert("out.txt".into(), "earlier=1\n".into());
        run(&opts, &mut runner, &mut files).0.unwrap();
        assert_eq!(
            files.files["out.txt"],
            "earlier=1\nversion=0.2.1\nwhy=a patch, because the changelog lists only changes and fixes\nprevious=0.2.0\n"
        );
    }

    #[test]
    fn refuses_a_dirty_tree() {
        let (mut runner, mut files) = setup("### Fixed\n\n- A bug.", "v0.2.0\n");
        runner.answer("git status --porcelain", ok(" M README.md\n"));
        let error = run(&options(None, true), &mut runner, &mut files)
            .0
            .unwrap_err();
        assert!(
            error.contains("uncommitted changes:\n M README.md\nCommit or stash"),
            "{error}"
        );
        assert_eq!(runner.calls, ["git status --porcelain"]);
    }

    #[test]
    fn carries_on_when_tags_cannot_be_fetched() {
        let (mut runner, mut files) = setup("### Fixed\n\n- A bug.", "v0.2.0\n");
        runner.answer("git fetch --tags --force --quiet", fail("no network"));
        let (result, log) = run(&options(None, true), &mut runner, &mut files);
        assert!(result.is_ok());
        assert!(log.contains("Could not fetch tags (no network)"));
    }

    #[test]
    fn refuses_a_version_already_tagged_or_not_higher() {
        let (mut runner, mut files) = setup("### Fixed\n\n- A bug.", "v0.2.0\nv0.3.0\n");
        let error = run(&options(Some("0.3.0"), true), &mut runner, &mut files)
            .0
            .unwrap_err();
        assert!(error.contains("v0.3.0 already exists"), "{error}");
        let error = run(&options(Some("0.2.5"), true), &mut runner, &mut files)
            .0
            .unwrap_err();
        assert!(error.contains("0.2.5 is not higher than 0.3.0"), "{error}");
    }

    #[test]
    fn refuses_an_empty_changelog() {
        let (mut runner, mut files) = setup("Nothing yet.", "v0.2.0\n");
        let error = run(&options(None, true), &mut runner, &mut files)
            .0
            .unwrap_err();
        assert!(error.contains("nothing to release"), "{error}");
        let error = run(&options(Some("0.2.1"), true), &mut runner, &mut files)
            .0
            .unwrap_err();
        assert!(error.contains("Say what changed in 0.2.1"), "{error}");
    }

    #[test]
    fn refuses_a_malformed_changelog() {
        let (mut runner, mut files) = setup("### Improved\n\n- A.", "v0.2.0\n");
        let error = run(&options(None, true), &mut runner, &mut files)
            .0
            .unwrap_err();
        assert!(
            error.starts_with("CHANGELOG.md needs fixing first:\n- "),
            "{error}"
        );
    }

    #[test]
    fn refuses_a_cargo_toml_without_a_version() {
        let (mut runner, mut files) = setup("### Fixed\n\n- A.", "v0.2.0\n");
        files
            .files
            .insert(CARGO_TOML.into(), "[workspace]\n".into());
        let error = run(&options(None, true), &mut runner, &mut files)
            .0
            .unwrap_err();
        assert!(error.contains("no [workspace.package]"), "{error}");
    }

    #[test]
    fn explains_missing_files_and_git() {
        let (mut runner, mut files) = setup("### Fixed\n\n- A.", "v0.2.0\n");
        files.files.remove(CHANGELOG);
        let error = run(&options(None, true), &mut runner, &mut files)
            .0
            .unwrap_err();
        assert!(error.starts_with("Could not read CHANGELOG.md"), "{error}");

        let (mut runner, mut files) = setup("### Fixed\n\n- A.", "v0.2.0\n");
        runner.answer(
            "git status --porcelain",
            Err(io::Error::new(io::ErrorKind::NotFound, "git missing")),
        );
        let error = run(&options(None, true), &mut runner, &mut files)
            .0
            .unwrap_err();
        assert!(
            error.contains("Could not run git status --porcelain: git missing"),
            "{error}"
        );

        let (mut runner, mut files) = setup("### Fixed\n\n- A.", "v0.2.0\n");
        runner.answer("git tag --list v*", fail("not a git repository"));
        let error = run(&options(None, true), &mut runner, &mut files)
            .0
            .unwrap_err();
        assert!(
            error.contains("git tag --list v* failed: not a git repository"),
            "{error}"
        );
    }

    #[test]
    fn falls_back_to_an_online_lock_update() {
        let (mut runner, mut files) = setup("### Fixed\n\n- A.", "v0.2.0\n");
        runner.answer("cargo update --workspace --offline", fail("offline"));
        let (result, log) = run(&options(None, false), &mut runner, &mut files);
        assert!(result.is_ok());
        assert!(log.contains("trying online"));
        assert!(runner
            .calls
            .contains(&"cargo update --workspace".to_string()));
    }

    #[test]
    fn a_failed_lock_update_puts_the_files_back() {
        let (mut runner, mut files) = setup("### Fixed\n\n- A.", "v0.2.0\n");
        let before = files.files.clone();
        runner.answer("cargo update --workspace --offline", fail("offline"));
        runner.answer("cargo update --workspace", fail("no network"));
        let error = run(&options(None, false), &mut runner, &mut files)
            .0
            .unwrap_err();
        assert!(error.contains("Cargo.lock would disagree"), "{error}");
        assert_eq!(files.files, before);
        assert!(!runner.calls.iter().any(|c| c.starts_with("git commit")));
    }

    #[test]
    fn cargo_that_cannot_start_is_an_error() {
        let (mut runner, mut files) = setup("### Fixed\n\n- A.", "v0.2.0\n");
        runner.answer(
            "cargo update --workspace --offline",
            Err(io::Error::new(io::ErrorKind::NotFound, "no cargo")),
        );
        let error = run(&options(None, false), &mut runner, &mut files)
            .0
            .unwrap_err();
        assert!(error.contains("Could not run cargo: no cargo"), "{error}");

        let (mut runner, mut files) = setup("### Fixed\n\n- A.", "v0.2.0\n");
        runner.answer("cargo update --workspace --offline", fail("offline"));
        runner.answer(
            "cargo update --workspace",
            Err(io::Error::new(io::ErrorKind::NotFound, "gone")),
        );
        let error = run(&options(None, false), &mut runner, &mut files)
            .0
            .unwrap_err();
        assert!(error.contains("Could not run cargo: gone"), "{error}");
    }

    #[test]
    fn a_failed_commit_is_reported() {
        let (mut runner, mut files) = setup("### Fixed\n\n- A.", "v0.2.0\n");
        runner.answer(
            "git commit --quiet -m Release v0.2.1",
            fail("Please tell me who you are"),
        );
        let error = run(&options(None, false), &mut runner, &mut files)
            .0
            .unwrap_err();
        assert!(error.contains("Please tell me who you are"), "{error}");
    }

    #[test]
    fn unwritable_files_are_reported() {
        let (mut runner, mut files) = setup("### Fixed\n\n- A.", "v0.2.0\n");
        files.fail_writes = true;
        let error = run(&options(None, false), &mut runner, &mut files)
            .0
            .unwrap_err();
        assert!(error.contains("Could not write CHANGELOG.md"), "{error}");

        let (mut runner, mut files) = setup("### Fixed\n\n- A.", "v0.2.0\n");
        files.fail_writes = true;
        let mut opts = options(None, true);
        opts.github_output = Some("out".into());
        let error = run(&opts, &mut runner, &mut files).0.unwrap_err();
        assert!(error.contains("Could not write out"), "{error}");
    }

    #[test]
    fn lists_tags() {
        let mut runner = FakeRunner::default();
        runner.answer("git tag --list v*", ok("v0.1.0\n\n v0.2.0 \n"));
        assert_eq!(tags(&mut runner).unwrap(), ["v0.1.0", "v0.2.0"]);
    }
}
