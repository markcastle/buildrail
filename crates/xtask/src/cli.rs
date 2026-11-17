//! The command line: arguments in, an exit code out.
//!
//! Exit codes: 0 done, 1 refused (the message says why), 2 the command was
//! not understood.

use std::io::Write;

use crate::changelog;
use crate::date::Date;
use crate::release::{self, Files, Options, Runner, CHANGELOG};
use crate::version::{highest_tag, Version};

/// What `--help` prints.
pub const USAGE: &str = "\
BuildRail release tooling. Run from anywhere in the repository:

  cargo run -p xtask -- <command>

Commands:
  next <current-version>        Print the next version (the reason goes to stderr)
  changelog check               Check CHANGELOG.md's shape; CI runs this
  changelog section <version>   Print a version's notes (or \"Unreleased\")
  changelog release <version>   Date Unreleased as that version [--date YYYY-MM-DD]
  changelog note \"<sentence>\"   Add a line under Changed in Unreleased [--clear]
  release [<version>]           Cut a release: stamp, set the version, commit, tag
                                [--dry-run] [--date YYYY-MM-DD] [--github-output <file>]
  highest-tag                   Print the highest vX.Y.Z tag here, or 0.0.0

Options:
  --changelog <path>   Read another changelog file (next, changelog), such as
                       an older copy in CI. Relative to the repository root.
  --clear              With note: replace whatever Unreleased held.
  --dry-run            With release: say what would happen and change nothing.
  --date <YYYY-MM-DD>  The date to stamp; today in UTC if left out.
  --github-output <f>  With release: append version=, why= and previous= lines.

Releases are normally cut by the Release workflow. See docs/releasing.md.
";

/// Everything a command needs from the outside world.
pub struct Context<'a> {
    /// Runs git and cargo.
    pub runner: &'a mut dyn Runner,
    /// Reads and writes the repository's files.
    pub files: &'a mut dyn Files,
    /// Standard output: results meant for scripts.
    pub out: &'a mut dyn Write,
    /// Standard error: messages meant for people.
    pub err: &'a mut dyn Write,
    /// Today's date, for stamping.
    pub today: Date,
}

/// The parsed command line.
#[derive(Debug, Default, PartialEq, Eq)]
struct Args {
    positional: Vec<String>,
    changelog: Option<String>,
    date: Option<String>,
    github_output: Option<String>,
    dry_run: bool,
    clear: bool,
    help: bool,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut parsed = Args::default();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut value = |name: &str| {
            iter.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value."))
        };
        match arg.as_str() {
            "--changelog" => parsed.changelog = Some(value("--changelog")?),
            "--date" => parsed.date = Some(value("--date")?),
            "--github-output" => parsed.github_output = Some(value("--github-output")?),
            "--dry-run" => parsed.dry_run = true,
            "--clear" => parsed.clear = true,
            "-h" | "--help" | "help" => parsed.help = true,
            flag if flag.starts_with("--") => return Err(format!("Unknown option {flag}.")),
            _ => parsed.positional.push(arg.clone()),
        }
    }
    Ok(parsed)
}

/// Why a command stopped.
enum Stop {
    /// It refused, and says why. Exit 1.
    Refused(String),
    /// It was not understood. Exit 2.
    Usage(String),
}

impl From<String> for Stop {
    fn from(message: String) -> Self {
        Self::Refused(message)
    }
}

fn usage<T>(message: &str) -> Result<T, Stop> {
    Err(Stop::Usage(message.to_string()))
}

fn print(stream: &mut dyn Write, text: &str) -> Result<(), Stop> {
    writeln!(stream, "{text}").map_err(|e| Stop::Refused(format!("Could not write output: {e}")))
}

/// Runs one command. `args` excludes the program name.
pub fn run(args: &[String], cx: &mut Context<'_>) -> u8 {
    match dispatch(args, cx) {
        Ok(()) => 0,
        Err(Stop::Refused(message)) => {
            let _ = writeln!(cx.err, "{message}");
            1
        }
        Err(Stop::Usage(message)) => {
            let _ = writeln!(cx.err, "{message}\n\n{USAGE}");
            2
        }
    }
}

fn dispatch(args: &[String], cx: &mut Context<'_>) -> Result<(), Stop> {
    let args = parse(args).map_err(Stop::Usage)?;
    if args.help {
        return print(cx.out, USAGE);
    }
    let words: Vec<&str> = args.positional.iter().map(String::as_str).collect();
    let changelog_path = args
        .changelog
        .clone()
        .unwrap_or_else(|| CHANGELOG.to_string());
    let date = match &args.date {
        Some(text) => Date::parse(text).map_err(Stop::Usage)?,
        None => cx.today,
    };
    match words.as_slice() {
        ["next", current] => {
            let current = Version::parse(current).map_err(Stop::Usage)?;
            let text = read(cx, &changelog_path)?;
            let bump = changelog::next_version(&text, current)?;
            print(
                cx.err,
                &format!("After {current} comes {}: {}.", bump.version, bump.why),
            )?;
            print(cx.out, &bump.version.to_string())
        }
        ["changelog", "check"] => {
            let problems = changelog::check(&read(cx, &changelog_path)?);
            if problems.is_empty() {
                print(cx.err, &format!("{changelog_path} is in good shape."))
            } else {
                Err(Stop::Refused(format!(
                    "{changelog_path} needs fixing:\n- {}",
                    problems.join("\n- ")
                )))
            }
        }
        ["changelog", "section", version] => {
            if *version != "Unreleased" {
                Version::parse(version).map_err(Stop::Usage)?;
            }
            match changelog::section(&read(cx, &changelog_path)?, version) {
                Some(notes) => print(cx.out, &notes),
                None => Err(Stop::Refused(format!(
                    "{changelog_path} has no section for {version}."
                ))),
            }
        }
        ["changelog", "release", version] => {
            let version = Version::parse(version).map_err(Stop::Usage)?;
            let stamped = changelog::release(&read(cx, &changelog_path)?, version, date)?;
            write(cx, &changelog_path, &stamped)?;
            print(
                cx.err,
                &format!("{changelog_path}: Unreleased is now {version} ({date})."),
            )
        }
        ["changelog", "note", sentence] => {
            let noted = changelog::note(&read(cx, &changelog_path)?, sentence, args.clear)?;
            write(cx, &changelog_path, &noted)?;
            print(
                cx.err,
                &format!("{changelog_path}: noted under Unreleased."),
            )
        }
        ["release", rest @ ..] if rest.len() <= 1 => {
            if args.changelog.is_some() {
                return usage("release always uses CHANGELOG.md; --changelog does not apply.");
            }
            let version = match rest.first() {
                Some(v) => Some(Version::parse(v).map_err(Stop::Usage)?),
                None => None,
            };
            let options = Options {
                version,
                dry_run: args.dry_run,
                date,
                github_output: args.github_output.clone(),
            };
            release::cut(&options, cx.runner, cx.files, cx.err)?;
            Ok(())
        }
        ["highest-tag"] => {
            let tags = release::tags(cx.runner)?;
            let highest =
                highest_tag(tags.iter().map(String::as_str)).unwrap_or(Version::new(0, 0, 0));
            print(cx.out, &highest.to_string())
        }
        [] => usage("Which command?"),
        _ => usage(&format!("Not a command: {}", words.join(" "))),
    }
}

fn read(cx: &Context<'_>, path: &str) -> Result<String, Stop> {
    cx.files
        .read(path)
        .map_err(|e| Stop::Refused(format!("Could not read {path}: {e}")))
}

fn write(cx: &mut Context<'_>, path: &str, contents: &str) -> Result<(), Stop> {
    cx.files
        .write(path, contents)
        .map_err(|e| Stop::Refused(format!("Could not write {path}: {e}")))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::io;

    use super::*;
    use crate::release::fakes::{ok, FakeFiles, FakeRunner};

    const REPO: &str = changelog::REPO_URL;

    fn changelog_text(unreleased: &str) -> String {
        format!(
            "# Changelog\n\n## [Unreleased]\n\n{unreleased}\n\n## [0.2.0] - 2026-11-17\n\n### Changed\n\n- Restyled.\n\n\
             [Unreleased]: {REPO}/compare/v0.2.0...HEAD\n[0.2.0]: {REPO}/releases/tag/v0.2.0\n"
        )
    }

    struct Outcome {
        code: u8,
        out: String,
        err: String,
        files: FakeFiles,
        calls: Vec<String>,
    }

    fn run_with(
        args: &[&str],
        unreleased: &str,
        setup: impl FnOnce(&mut FakeRunner, &mut FakeFiles),
    ) -> Outcome {
        let mut runner = FakeRunner::default();
        let mut files = FakeFiles::default();
        files
            .files
            .insert(CHANGELOG.into(), changelog_text(unreleased));
        files.files.insert(
            "Cargo.toml".into(),
            "[workspace.package]\nversion = \"0.2.0\"\n".into(),
        );
        setup(&mut runner, &mut files);
        let mut out = Vec::new();
        let mut err = Vec::new();
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let code = {
            let mut cx = Context {
                runner: &mut runner,
                files: &mut files,
                out: &mut out,
                err: &mut err,
                today: Date::parse("2026-11-20").unwrap(),
            };
            run(&args, &mut cx)
        };
        Outcome {
            code,
            out: String::from_utf8(out).unwrap(),
            err: String::from_utf8(err).unwrap(),
            files,
            calls: runner.calls,
        }
    }

    fn run_cli(args: &[&str], unreleased: &str) -> Outcome {
        run_with(args, unreleased, |_, _| {})
    }

    #[test]
    fn next_prints_the_number_and_explains_on_stderr() {
        let r = run_cli(&["next", "0.2.0"], "### Added\n\n- A.");
        assert_eq!(r.code, 0);
        assert_eq!(r.out, "0.3.0\n");
        assert_eq!(r.err, "After 0.2.0 comes 0.3.0: a minor release, because the changelog lists something Added.\n");
    }

    #[test]
    fn next_refuses_when_there_is_nothing() {
        let r = run_cli(&["next", "0.2.0"], "Nothing yet.");
        assert_eq!(r.code, 1);
        assert_eq!(r.out, "");
        assert!(r.err.contains("nothing to release"));
    }

    #[test]
    fn next_wants_a_version() {
        let r = run_cli(&["next", "latest"], "### Added\n\n- A.");
        assert_eq!(r.code, 2);
        assert!(r.err.contains("is not a version"));
        assert!(r.err.contains("Commands:"));
    }

    #[test]
    fn check_passes_and_fails() {
        let good = run_cli(&["changelog", "check"], "Nothing yet.");
        assert_eq!(good.code, 0);
        assert!(good.err.contains("good shape"));
        let bad = run_cli(&["changelog", "check"], "Loose text.");
        assert_eq!(bad.code, 1);
        assert!(bad
            .err
            .starts_with("CHANGELOG.md needs fixing:\n- Unreleased has text"));
    }

    #[test]
    fn section_prints_notes() {
        let r = run_cli(&["changelog", "section", "0.2.0"], "Nothing yet.");
        assert_eq!(
            (r.code, r.out.as_str()),
            (0, "### Changed\n\n- Restyled.\n")
        );
        let r = run_cli(&["changelog", "section", "Unreleased"], "### Fixed\n\n- F.");
        assert_eq!(r.out, "### Fixed\n\n- F.\n");
        let r = run_cli(&["changelog", "section", "0.9.0"], "Nothing yet.");
        assert_eq!(r.code, 1);
        assert!(r.err.contains("no section for 0.9.0"));
        let r = run_cli(&["changelog", "section", "v0.2.0"], "Nothing yet.");
        assert_eq!(r.code, 2);
    }

    #[test]
    fn changelog_release_stamps_with_today_or_a_given_date() {
        let r = run_cli(&["changelog", "release", "0.3.0"], "### Added\n\n- A.");
        assert_eq!(r.code, 0, "{}", r.err);
        assert!(r.files.files[CHANGELOG].contains("## [0.3.0] - 2026-11-20"));
        assert!(r.err.contains("Unreleased is now 0.3.0 (2026-11-20)"));
        let r = run_cli(
            &["changelog", "release", "0.3.0", "--date", "2026-12-01"],
            "### Added\n\n- A.",
        );
        assert!(r.files.files[CHANGELOG].contains("## [0.3.0] - 2026-12-01"));
        let r = run_cli(
            &["changelog", "release", "0.3.0", "--date", "tomorrow"],
            "### Added\n\n- A.",
        );
        assert_eq!(r.code, 2);
        let r = run_cli(&["changelog", "release", "0.3.0"], "Nothing yet.");
        assert_eq!(r.code, 1);
    }

    #[test]
    fn note_adds_a_line_and_clear_replaces() {
        let r = run_cli(&["changelog", "note", "Rolled back."], "### Fixed\n\n- F.");
        assert_eq!(r.code, 0);
        assert!(r.files.files[CHANGELOG].contains("### Changed\n\n- Rolled back.\n\n### Fixed"));
        let r = run_cli(
            &["changelog", "note", "Rolled back.", "--clear"],
            "### Fixed\n\n- F.",
        );
        assert!(!r.files.files[CHANGELOG].contains("- F."));
        assert!(r.err.contains("noted under Unreleased"));
    }

    #[test]
    fn another_changelog_file_can_be_read() {
        let r = run_with(
            &[
                "--changelog",
                "old.md",
                "changelog",
                "section",
                "Unreleased",
            ],
            "Nothing yet.",
            |_, files| {
                files
                    .files
                    .insert("old.md".into(), changelog_text("### Fixed\n\n- Old."));
            },
        );
        assert_eq!(r.out, "### Fixed\n\n- Old.\n");
        let r = run_cli(
            &["--changelog", "missing.md", "changelog", "check"],
            "Nothing yet.",
        );
        assert_eq!(r.code, 1);
        assert!(r.err.contains("Could not read missing.md"));
    }

    #[test]
    fn release_runs_the_flow() {
        let r = run_with(
            &["release", "--dry-run", "--github-output", "gh.txt"],
            "### Fixed\n\n- F.",
            |runner, _| {
                runner.answer("git tag --list v*", ok("v0.2.0\n"));
            },
        );
        assert_eq!(r.code, 0, "{}", r.err);
        assert!(r.err.contains("Releasing BuildRail 0.2.1"));
        assert!(r.files.files["gh.txt"].starts_with("version=0.2.1\n"));

        let r = run_with(
            &["release", "1.0.0", "--date", "2027-01-01"],
            "### Fixed\n\n- F.",
            |runner, _| {
                runner.answer("git tag --list v*", ok("v0.2.0\n"));
            },
        );
        assert_eq!(r.code, 0, "{}", r.err);
        assert!(r.files.files[CHANGELOG].contains("## [1.0.0] - 2027-01-01"));
        assert!(r
            .calls
            .contains(&"git tag -a v1.0.0 -m BuildRail v1.0.0".to_string()));
    }

    #[test]
    fn release_refusals_and_misuse() {
        let r = run_with(&["release"], "Nothing yet.", |runner, _| {
            runner.answer("git tag --list v*", ok("v0.2.0\n"));
        });
        assert_eq!(r.code, 1);
        assert!(r.err.contains("nothing to release"));
        assert_eq!(run_cli(&["release", "0.3"], "Nothing yet.").code, 2);
        assert_eq!(
            run_cli(&["release", "0.3.0", "0.4.0"], "Nothing yet.").code,
            2
        );
        assert_eq!(
            run_cli(&["--changelog", "x.md", "release"], "Nothing yet.").code,
            2
        );
    }

    #[test]
    fn highest_tag_prints_a_number() {
        let r = run_with(&["highest-tag"], "", |runner, _| {
            runner.answer("git tag --list v*", ok("v0.9.0\nv0.10.0\nv1.0.0-rc1\n"));
        });
        assert_eq!(r.out, "0.10.0\n");
        let r = run_cli(&["highest-tag"], "");
        assert_eq!(r.out, "0.0.0\n");
        let r = run_with(&["highest-tag"], "", |runner, _| {
            runner.answer(
                "git tag --list v*",
                Err(io::Error::new(io::ErrorKind::NotFound, "no git")),
            );
        });
        assert_eq!(r.code, 1);
    }

    #[test]
    fn help_and_mistakes() {
        let r = run_cli(&["--help"], "");
        assert_eq!(r.code, 0);
        assert!(r.out.contains("cargo run -p xtask"));
        assert_eq!(run_cli(&[], "").code, 2);
        assert_eq!(run_cli(&["publish"], "").code, 2);
        assert_eq!(run_cli(&["--verbose"], "").code, 2);
        let r = run_cli(&["changelog", "section", "0.2.0", "--changelog"], "");
        assert_eq!(r.code, 2);
        assert!(r.err.contains("--changelog needs a value"));
    }

    #[test]
    fn unwritable_changelog_is_reported() {
        let r = run_with(&["changelog", "note", "x"], "Nothing yet.", |_, files| {
            files.fail_writes = true
        });
        assert_eq!(r.code, 1);
        assert!(r.err.contains("Could not write CHANGELOG.md"));
    }

    #[test]
    fn output_that_cannot_be_written_is_a_refusal() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut runner = FakeRunner::default();
        let mut files = FakeFiles::default();
        let mut err = Vec::new();
        let mut cx = Context {
            runner: &mut runner,
            files: &mut files,
            out: &mut Broken,
            err: &mut err,
            today: Date::parse("2026-11-20").unwrap(),
        };
        assert_eq!(run(&["--help".to_string()], &mut cx), 1);
        assert!(String::from_utf8(err)
            .unwrap()
            .contains("Could not write output"));
    }
}
