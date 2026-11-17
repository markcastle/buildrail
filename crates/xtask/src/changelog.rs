//! `CHANGELOG.md`, read and written by the release tooling.
//!
//! The file follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/):
//! `## [Unreleased]` at the top collects changes as they land, and cutting a
//! release gives that section a version and a date and opens a fresh
//! Unreleased above it. Every function here is pure: text in, text out.
//!
//! The headings under Unreleased decide the next version number:
//!
//! | Unreleased lists | The release is |
//! | --- | --- |
//! | Something **Added** or **Removed** | a minor release (0.2.0 → 0.3.0) |
//! | Only **Changed**, **Fixed**, **Deprecated** or **Security** | a patch (0.2.0 → 0.2.1) |
//!
//! Deprecated is a patch because nothing has gone yet: it is a warning that
//! something will be Removed later, and that later release is the minor one.
//! Security is a patch because it is a fix. Going to 1.0.0, or any major, is
//! never automatic; type the version.

use crate::date::Date;
use crate::version::Version;

/// Where the compare links point.
pub const REPO_URL: &str = "https://github.com/markcastle/buildrail";

/// What an Unreleased section with nothing in it says.
pub const EMPTY: &str = "Nothing yet.";

/// The headings Keep a Changelog allows, in the order it lists them.
pub const KINDS: [&str; 6] = [
    "Added",
    "Changed",
    "Deprecated",
    "Removed",
    "Fixed",
    "Security",
];

/// Headings that make the next release a minor one.
const MINOR_KINDS: [&str; 2] = ["Added", "Removed"];

const UNRELEASED: &str = "Unreleased";

/// One `## ` section of the file, by line numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Section<'a> {
    /// The heading line itself.
    heading: &'a str,
    /// Index of the heading line.
    start: usize,
    /// Index one past the last line of the body.
    end: usize,
}

impl Section<'_> {
    /// What is between the brackets: `Unreleased` or a version, if the
    /// heading has brackets at all.
    fn title(&self) -> Option<&str> {
        let rest = self.heading.strip_prefix("## [")?;
        let close = rest.find(']')?;
        Some(&rest[..close])
    }
}

fn is_link_definition(line: &str) -> bool {
    line.starts_with('[') && line.contains("]: ")
}

fn sections<'a>(lines: &[&'a str]) -> Vec<Section<'a>> {
    let mut found: Vec<Section<'_>> = Vec::new();
    let mut in_fence = false;
    for (i, line) in lines.iter().enumerate() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
        }
        if !in_fence && line.starts_with("## ") {
            if let Some(last) = found.last_mut() {
                last.end = i;
            }
            found.push(Section {
                heading: line,
                start: i,
                end: lines.len(),
            });
        }
    }
    found
}

fn find<'a>(sections: &'a [Section<'a>], title: &str) -> Option<&'a Section<'a>> {
    sections.iter().find(|s| s.title() == Some(title))
}

/// The body of a section, without link definitions, trimmed.
fn body(lines: &[&str], section: &Section<'_>) -> String {
    lines[section.start + 1..section.end]
        .iter()
        .filter(|line| !is_link_definition(line))
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// `### Kind` headings in a body, in order.
fn kinds(body: &str) -> Vec<&str> {
    body.lines()
        .filter_map(|line| line.strip_prefix("### "))
        .map(str::trim)
        .collect()
}

fn is_empty_notes(notes: &str) -> bool {
    notes.is_empty() || notes == EMPTY
}

fn lines_of(text: &str) -> Vec<&str> {
    text.lines().collect()
}

/// The notes for a version (or for `Unreleased`): the text between its
/// heading and the next one, without link definitions or surrounding blank
/// lines. `None` when the file has no such section.
pub fn section(text: &str, title: &str) -> Option<String> {
    let lines = lines_of(text);
    let all = sections(&lines);
    find(&all, title).map(|s| body(&lines, s))
}

/// The version the next release should have, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bump {
    /// The number to release.
    pub version: Version,
    /// A sentence fragment saying why, for the person reading the log.
    pub why: String,
}

/// What the release after `current` should be numbered, going by the
/// headings under Unreleased. Refuses when there is nothing to release.
pub fn next_version(text: &str, current: Version) -> Result<Bump, String> {
    let notes = section(text, UNRELEASED).ok_or_else(no_unreleased)?;
    if is_empty_notes(&notes) {
        return Err(
            "CHANGELOG.md has nothing under \"## [Unreleased]\", so there is nothing \
                    to release. Say what changed first."
                .to_string(),
        );
    }
    let found = kinds(&notes);
    if found.is_empty() {
        return Err(
            "CHANGELOG.md has text under \"## [Unreleased]\" but no \"### Added\", \
                    \"### Changed\" or \"### Fixed\" heading, and the heading is what decides \
                    the version. File each line under one."
                .to_string(),
        );
    }
    if let Some(unknown) = found.iter().find(|k| !KINDS.contains(k)) {
        return Err(format!(
            "CHANGELOG.md has \"### {unknown}\" under Unreleased. Use one of: {}.",
            KINDS.join(", ")
        ));
    }
    let mut noticeable: Vec<&str> = MINOR_KINDS
        .iter()
        .copied()
        .filter(|k| found.contains(k))
        .collect();
    noticeable.dedup();
    if noticeable.is_empty() {
        Ok(Bump {
            version: current.next_patch(),
            why: "a patch, because the changelog lists only changes and fixes".to_string(),
        })
    } else {
        Ok(Bump {
            version: current.next_minor(),
            why: format!(
                "a minor release, because the changelog lists something {}",
                noticeable.join(" and ")
            ),
        })
    }
}

fn no_unreleased() -> String {
    "CHANGELOG.md has no \"## [Unreleased]\" section. Add one at the top.".to_string()
}

fn released_versions(sections: &[Section<'_>]) -> Vec<Version> {
    sections
        .iter()
        .filter_map(|s| s.title())
        .filter_map(|t| Version::parse(t).ok())
        .collect()
}

fn ends_with_newline(mut out: String) -> String {
    while out.ends_with("\n\n") {
        out.pop();
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Turns Unreleased into `## [version] - date`, opens a fresh Unreleased
/// holding "Nothing yet.", and keeps the compare links at the bottom right.
/// Refuses an empty Unreleased (a release nobody can read about), a version
/// already in the file, and a version not above the newest one in it.
pub fn release(text: &str, version: Version, date: Date) -> Result<String, String> {
    let lines = lines_of(text);
    let all = sections(&lines);
    let unreleased = find(&all, UNRELEASED).ok_or_else(no_unreleased)?;
    if is_empty_notes(&body(&lines, unreleased)) {
        return Err(format!(
            "CHANGELOG.md has nothing under \"## [Unreleased]\". Say what changed in {version} \
             before releasing it."
        ));
    }
    let versions = released_versions(&all);
    if versions.contains(&version) {
        return Err(format!("CHANGELOG.md already has a section for {version}."));
    }
    let previous = versions.iter().max().copied();
    if let Some(previous) = previous {
        if previous > version {
            return Err(format!(
                "{version} is lower than {previous}, the newest version in CHANGELOG.md."
            ));
        }
    }

    let unreleased_link = format!("[Unreleased]: {REPO_URL}/compare/v{version}...HEAD");
    let version_link = match previous {
        Some(p) => format!("[{version}]: {REPO_URL}/compare/v{p}...v{version}"),
        None => format!("[{version}]: {REPO_URL}/releases/tag/v{version}"),
    };

    let mut out: Vec<String> = Vec::with_capacity(lines.len() + 6);
    let mut linked = false;
    for (i, line) in lines.iter().enumerate() {
        if i == unreleased.start {
            out.push("## [Unreleased]".to_string());
            out.push(String::new());
            out.push(EMPTY.to_string());
            out.push(String::new());
            out.push(format!("## [{version}] - {date}"));
        } else if line.starts_with("[Unreleased]: ") && !linked {
            out.push(unreleased_link.clone());
            out.push(version_link.clone());
            linked = true;
        } else {
            out.push((*line).to_string());
        }
    }
    if !linked {
        while out.last().is_some_and(|l| l.is_empty()) {
            out.pop();
        }
        out.push(String::new());
        out.push(unreleased_link);
        out.push(version_link);
    }
    Ok(ends_with_newline(out.join("\n")))
}

/// Adds one line under `### Changed` in Unreleased, making the heading if it
/// is missing and dropping the "Nothing yet." placeholder. With `clear`,
/// whatever Unreleased held is replaced by this one line: a rollback uses
/// that, because the lines there describe work it has just taken off main
/// (reverting the rollback commit brings them back).
pub fn note(text: &str, line: &str, clear: bool) -> Result<String, String> {
    let line = line.trim();
    if line.is_empty() || line.contains('\n') {
        return Err("A changelog note is one sentence on one line.".to_string());
    }
    let lines = lines_of(text);
    let all = sections(&lines);
    let unreleased = find(&all, UNRELEASED).ok_or_else(no_unreleased)?;
    let old_body: Vec<&str> = lines[unreleased.start + 1..unreleased.end].to_vec();
    let links: Vec<&str> = old_body
        .iter()
        .copied()
        .filter(|l| is_link_definition(l))
        .collect();

    // The body as a preamble (free text before the first heading) and the
    // subsections under it, so the new line can be put in the right place.
    let mut preamble: Vec<&str> = Vec::new();
    let mut subsections: Vec<(String, Vec<&str>)> = Vec::new();
    if !clear {
        for l in old_body.iter().copied().filter(|l| !is_link_definition(l)) {
            if let Some(kind) = l.strip_prefix("### ") {
                subsections.push((kind.trim().to_string(), Vec::new()));
            } else if let Some((_, items)) = subsections.last_mut() {
                items.push(l);
            } else if l.trim() != EMPTY {
                preamble.push(l);
            }
        }
    }
    let entry = format!("- {line}");
    if let Some((_, items)) = subsections.iter_mut().find(|(k, _)| k == "Changed") {
        let first = items.iter().position(|l| !l.trim().is_empty()).unwrap_or(0);
        items.insert(first, &entry);
    } else {
        // Keep a Changelog's order: Changed comes after Added and before
        // everything else.
        let at = subsections
            .iter()
            .position(|(k, _)| k != "Added")
            .unwrap_or(subsections.len());
        subsections.insert(at, ("Changed".to_string(), vec![entry.as_str()]));
    }

    let mut new_body: Vec<String> = Vec::new();
    let preamble_text = preamble.join("\n").trim().to_string();
    if !preamble_text.is_empty() {
        new_body.push(preamble_text);
    }
    for (kind, items) in &subsections {
        new_body.push(format!("### {kind}"));
        let text = items.join("\n").trim().to_string();
        if !text.is_empty() {
            new_body.push(text);
        }
    }
    if !links.is_empty() {
        new_body.push(links.join("\n"));
    }

    let mut out: Vec<String> = lines[..=unreleased.start]
        .iter()
        .map(|l| (*l).to_string())
        .collect();
    out.push(String::new());
    out.push(new_body.join("\n\n"));
    out.push(String::new());
    out.extend(lines[unreleased.end..].iter().map(|l| (*l).to_string()));
    Ok(ends_with_newline(out.join("\n")))
}

fn parse_release_heading(heading: &str) -> Result<(Version, Date), String> {
    let refuse = || {
        format!(
            "\"{heading}\" should look like \"## [1.2.3] - YYYY-MM-DD\" or \"## [Unreleased]\"."
        )
    };
    let rest = heading.strip_prefix("## [").ok_or_else(refuse)?;
    let (version, rest) = rest.split_once("] - ").ok_or_else(refuse)?;
    let version = Version::parse(version).map_err(|e| format!("{}: {e}", refuse()))?;
    let date = Date::parse(rest).map_err(|e| format!("{}: {e}", refuse()))?;
    Ok((version, date))
}

/// Everything wrong with the shape of the file, one sentence each. Empty
/// means it is fine. CI runs this so a malformed changelog is caught in the
/// pull request, not when the Release button is pressed.
pub fn check(text: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let lines = lines_of(text);
    let all = sections(&lines);

    match all.first() {
        None => {
            problems.push(no_unreleased());
            return problems;
        }
        Some(first) if first.heading != "## [Unreleased]" => {
            problems.push(format!(
                "The first section must be \"## [Unreleased]\", not \"{}\".",
                first.heading
            ));
        }
        Some(_) => {}
    }

    let mut released: Vec<(Version, Date)> = Vec::new();
    for s in &all {
        let notes = body(&lines, s);
        let found = kinds(&notes);
        let name = s.title().unwrap_or(s.heading);
        if s.heading == "## [Unreleased]" {
            if s.start != all[0].start {
                problems.push("There is more than one \"## [Unreleased]\" section.".to_string());
            }
            if !is_empty_notes(&notes) && found.is_empty() {
                problems.push(
                    "Unreleased has text but no \"### Added\", \"### Changed\" or \"### Fixed\" \
                     heading. The heading decides the next version, so file each line under one."
                        .to_string(),
                );
            }
        } else {
            match parse_release_heading(s.heading) {
                Ok(parsed) => released.push(parsed),
                Err(problem) => problems.push(problem),
            }
            if notes.is_empty() {
                problems.push(format!("{name} has no notes."));
            }
        }
        let mut seen: Vec<&str> = Vec::new();
        for kind in &found {
            if !KINDS.contains(kind) {
                problems.push(format!(
                    "{name} has \"### {kind}\". Use one of: {}.",
                    KINDS.join(", ")
                ));
            }
            if seen.contains(kind) {
                problems.push(format!("{name} has \"### {kind}\" twice."));
            }
            seen.push(kind);
        }
        problems.extend(empty_subsections(&notes, name));
    }

    for pair in released.windows(2) {
        let ((newer, newer_date), (older, older_date)) = (pair[0], pair[1]);
        if newer <= older {
            problems.push(format!(
                "{newer} is listed above {older}. Newest first, and every version once."
            ));
        }
        if newer_date < older_date {
            problems.push(format!(
                "{newer} is dated {newer_date}, before {older} ({older_date})."
            ));
        }
    }

    problems.extend(check_links(&lines, &released));
    problems
}

fn empty_subsections(notes: &str, name: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let mut current: Option<&str> = None;
    let mut has_text = false;
    for line in notes.lines().chain(std::iter::once("### ")) {
        if let Some(kind) = line.strip_prefix("### ") {
            if let Some(previous) = current {
                if !has_text {
                    problems.push(format!("{name} has an empty \"### {previous}\"."));
                }
            }
            current = Some(kind.trim());
            has_text = false;
        } else if !line.trim().is_empty() {
            has_text = true;
        }
    }
    problems
}

fn check_links(lines: &[&str], released: &[(Version, Date)]) -> Vec<String> {
    let mut problems = Vec::new();
    let definitions: Vec<(&str, &str)> = lines
        .iter()
        .filter(|l| is_link_definition(l))
        .filter_map(|l| l.strip_prefix('['))
        .filter_map(|l| l.split_once("]: "))
        .collect();
    let target = |name: &str| {
        definitions
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, t)| *t)
    };

    let expected_unreleased = match released.first() {
        Some((newest, _)) => format!("{REPO_URL}/compare/v{newest}...HEAD"),
        None => format!("{REPO_URL}/commits/HEAD"),
    };
    match target("Unreleased") {
        None => problems.push(format!(
            "The \"[Unreleased]: {expected_unreleased}\" link at the bottom is missing."
        )),
        Some(t) if released.is_empty() || t == expected_unreleased => {}
        Some(t) => problems.push(format!(
            "The [Unreleased] link points at {t}; it should be {expected_unreleased}."
        )),
    }
    for (i, (version, _)) in released.iter().enumerate() {
        let expected = match released.get(i + 1) {
            Some((older, _)) => format!("{REPO_URL}/compare/v{older}...v{version}"),
            None => format!("{REPO_URL}/releases/tag/v{version}"),
        };
        match target(&version.to_string()) {
            None => problems.push(format!("The \"[{version}]: {expected}\" link is missing.")),
            Some(t) if t == expected => {}
            Some(t) => problems.push(format!(
                "The [{version}] link points at {t}; it should be {expected}."
            )),
        }
    }
    for (name, _) in &definitions {
        let known = *name == "Unreleased"
            || Version::parse(name).is_ok_and(|v| released.iter().any(|(r, _)| *r == v));
        if !known && Version::parse(name).is_ok() {
            problems.push(format!(
                "There is a [{name}] link but no \"## [{name}]\" section."
            ));
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const REPO: &str = "https://github.com/markcastle/buildrail";

    fn log(unreleased: &str) -> String {
        format!(
            "# Changelog\n\nIntro.\n\n## [Unreleased]\n\n{unreleased}\n\n## [0.2.0] - 2026-11-17\n\n\
             ### Changed\n\n- Restyled.\n\n## [0.1.0] - 2026-11-16\n\n### Added\n\n- The strip.\n\n\
             [Unreleased]: {REPO}/compare/v0.2.0...HEAD\n\
             [0.2.0]: {REPO}/compare/v0.1.0...v0.2.0\n\
             [0.1.0]: {REPO}/releases/tag/v0.1.0\n"
        )
    }

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    fn date() -> Date {
        Date::parse("2026-11-20").unwrap()
    }

    fn next(unreleased: &str) -> Result<Bump, String> {
        next_version(&log(unreleased), v("0.2.0"))
    }

    // The numbering rule, one row per heading.
    #[test]
    fn the_heading_decides_the_version() {
        let table = [
            ("### Added\n\n- A.", "0.3.0"),
            ("### Removed\n\n- A.", "0.3.0"),
            ("### Changed\n\n- A.", "0.2.1"),
            ("### Fixed\n\n- A.", "0.2.1"),
            ("### Deprecated\n\n- A.", "0.2.1"),
            ("### Security\n\n- A.", "0.2.1"),
            ("### Changed\n\n- A.\n\n### Fixed\n\n- B.", "0.2.1"),
            ("### Added\n\n- A.\n\n### Fixed\n\n- B.", "0.3.0"),
            ("### Fixed\n\n- A.\n\n### Removed\n\n- B.", "0.3.0"),
        ];
        for (unreleased, expected) in table {
            assert_eq!(
                next(unreleased).unwrap().version.to_string(),
                expected,
                "{unreleased}"
            );
        }
    }

    #[test]
    fn the_reason_names_the_headings() {
        let both = next("### Added\n\n- A.\n\n### Removed\n\n- B.").unwrap();
        assert_eq!(
            both.why,
            "a minor release, because the changelog lists something Added and Removed"
        );
        let patch = next("### Fixed\n\n- A.").unwrap();
        assert_eq!(
            patch.why,
            "a patch, because the changelog lists only changes and fixes"
        );
    }

    #[test]
    fn a_minor_release_starts_the_patch_number_again() {
        let bump = next_version(&log("### Added\n\n- A."), v("0.2.7")).unwrap();
        assert_eq!(bump.version, v("0.3.0"));
    }

    #[test]
    fn the_word_added_in_a_sentence_is_not_a_heading() {
        assert_eq!(
            next("### Fixed\n\n- Added builds no longer vanish.")
                .unwrap()
                .version,
            v("0.2.1")
        );
    }

    #[test]
    fn nothing_to_release_is_refused_not_numbered() {
        assert!(next(EMPTY).unwrap_err().contains("nothing to release"));
        assert!(next("").unwrap_err().contains("nothing to release"));
    }

    #[test]
    fn text_without_a_heading_is_refused() {
        assert!(next("- Something.")
            .unwrap_err()
            .contains("no \"### Added\""));
    }

    #[test]
    fn an_unknown_heading_is_refused() {
        assert!(next("### Improved\n\n- A.")
            .unwrap_err()
            .contains("### Improved"));
    }

    #[test]
    fn no_unreleased_section_is_refused() {
        let text = "# Changelog\n\n## [0.1.0] - 2026-11-16\n\n### Added\n\n- A.\n";
        assert!(next_version(text, v("0.1.0"))
            .unwrap_err()
            .contains("no \"## [Unreleased]\""));
    }

    #[test]
    fn section_returns_a_versions_notes() {
        let text = log("### Fixed\n\n- A bug.");
        assert_eq!(
            section(&text, "0.2.0").unwrap(),
            "### Changed\n\n- Restyled."
        );
        assert_eq!(
            section(&text, "0.1.0").unwrap(),
            "### Added\n\n- The strip."
        );
        assert_eq!(
            section(&text, "Unreleased").unwrap(),
            "### Fixed\n\n- A bug."
        );
        assert_eq!(section(&text, "9.9.9"), None);
    }

    #[test]
    fn section_does_not_match_a_longer_version() {
        let text = log("Nothing yet.").replace("[0.1.0] - 2026-11-16", "[0.1.10] - 2026-11-16");
        assert_eq!(section(&text, "0.1.1"), None);
    }

    #[test]
    fn headings_inside_code_fences_are_text() {
        let text = log("### Fixed\n\n- A.\n\n```\n## [9.9.9] - 2026-01-01\n```");
        assert!(section(&text, "Unreleased").unwrap().contains("9.9.9"));
        assert_eq!(section(&text, "9.9.9"), None);
    }

    #[test]
    fn release_stamps_unreleased_and_opens_a_fresh_one() {
        let text = log("### Added\n\n- A feature.");
        let stamped = release(&text, v("0.3.0"), date()).unwrap();
        assert!(stamped.contains("## [Unreleased]\n\nNothing yet.\n\n## [0.3.0] - 2026-11-20\n\n### Added\n\n- A feature.\n\n## [0.2.0]"));
        assert!(stamped.contains(&format!(
            "[Unreleased]: {REPO}/compare/v0.3.0...HEAD\n[0.3.0]: {REPO}/compare/v0.2.0...v0.3.0\n[0.2.0]:"
        )));
        assert!(stamped.starts_with("# Changelog\n\nIntro.\n"));
        assert!(stamped
            .ends_with("[0.1.0]: https://github.com/markcastle/buildrail/releases/tag/v0.1.0\n"));
        assert_eq!(check(&stamped), Vec::<String>::new());
    }

    #[test]
    fn the_number_worked_out_is_the_number_stamped() {
        let text = log("### Added\n\n- A feature.");
        let bump = next_version(&text, v("0.2.0")).unwrap();
        let stamped = release(&text, bump.version, date()).unwrap();
        assert_eq!(
            section(&stamped, "0.3.0").unwrap(),
            "### Added\n\n- A feature."
        );
        assert_eq!(section(&stamped, "Unreleased").unwrap(), EMPTY);
        assert!(next_version(&stamped, bump.version)
            .unwrap_err()
            .contains("nothing"));
    }

    #[test]
    fn release_refuses_an_empty_unreleased() {
        assert!(release(&log(EMPTY), v("0.3.0"), date())
            .unwrap_err()
            .contains("nothing under"));
    }

    #[test]
    fn release_refuses_a_version_already_there_or_lower() {
        let text = log("### Fixed\n\n- A.");
        assert!(release(&text, v("0.2.0"), date())
            .unwrap_err()
            .contains("already has"));
        assert!(release(&text, v("0.1.5"), date())
            .unwrap_err()
            .contains("lower than 0.2.0"));
    }

    #[test]
    fn release_refuses_without_unreleased() {
        let text = "# Changelog\n";
        assert!(release(text, v("0.1.0"), date())
            .unwrap_err()
            .contains("no \"## [Unreleased]\""));
    }

    #[test]
    fn the_first_release_links_to_its_tag_and_adds_the_links() {
        let text = "# Changelog\n\n## [Unreleased]\n\n### Added\n\n- The strip.\n\n\n";
        let stamped = release(text, v("0.1.0"), date()).unwrap();
        assert!(stamped.ends_with(&format!(
            "- The strip.\n\n[Unreleased]: {REPO}/compare/v0.1.0...HEAD\n[0.1.0]: {REPO}/releases/tag/v0.1.0\n"
        )));
        assert_eq!(check(&stamped), Vec::<String>::new());
    }

    #[test]
    fn note_replaces_the_placeholder() {
        let noted = note(&log(EMPTY), "Rolled back to 0.1.0. It crashed.", false).unwrap();
        assert_eq!(
            section(&noted, "Unreleased").unwrap(),
            "### Changed\n\n- Rolled back to 0.1.0. It crashed."
        );
        assert!(noted.contains("- Rolled back to 0.1.0. It crashed.\n\n## [0.2.0]"));
        assert_eq!(check(&noted), Vec::<String>::new());
    }

    #[test]
    fn note_goes_first_under_an_existing_changed() {
        let noted = note(&log("### Changed\n\n- Old line."), "New line.", false).unwrap();
        assert_eq!(
            section(&noted, "Unreleased").unwrap(),
            "### Changed\n\n- New line.\n- Old line."
        );
    }

    #[test]
    fn note_puts_changed_after_added_and_before_fixed() {
        let noted = note(&log("### Added\n\n- A.\n\n### Fixed\n\n- F."), "C.", false).unwrap();
        assert_eq!(
            section(&noted, "Unreleased").unwrap(),
            "### Added\n\n- A.\n\n### Changed\n\n- C.\n\n### Fixed\n\n- F."
        );
        let only_added = note(&log("### Added\n\n- A."), "C.", false).unwrap();
        assert_eq!(
            section(&only_added, "Unreleased").unwrap(),
            "### Added\n\n- A.\n\n### Changed\n\n- C."
        );
    }

    #[test]
    fn note_keeps_free_text_before_the_headings() {
        let noted = note(&log("Some intro.\n\n### Fixed\n\n- F."), "C.", false).unwrap();
        assert_eq!(
            section(&noted, "Unreleased").unwrap(),
            "Some intro.\n\n### Changed\n\n- C.\n\n### Fixed\n\n- F."
        );
    }

    #[test]
    fn note_with_clear_replaces_everything() {
        let noted = note(
            &log("### Added\n\n- A.\n\n### Fixed\n\n- F."),
            "Rolled back.",
            true,
        )
        .unwrap();
        assert_eq!(
            section(&noted, "Unreleased").unwrap(),
            "### Changed\n\n- Rolled back."
        );
    }

    #[test]
    fn note_works_when_unreleased_is_the_only_section() {
        let text = format!(
            "# Changelog\n\n## [Unreleased]\n\nNothing yet.\n\n[Unreleased]: {REPO}/commits/HEAD\n"
        );
        let noted = note(&text, "C.", false).unwrap();
        assert_eq!(
            noted,
            format!("# Changelog\n\n## [Unreleased]\n\n### Changed\n\n- C.\n\n[Unreleased]: {REPO}/commits/HEAD\n")
        );
    }

    #[test]
    fn note_refuses_blank_or_multiline_and_needs_unreleased() {
        assert!(note(&log(EMPTY), "  ", false).is_err());
        assert!(note(&log(EMPTY), "a\nb", false).is_err());
        assert!(note("# Changelog\n", "a", false)
            .unwrap_err()
            .contains("Unreleased"));
    }

    #[test]
    fn check_accepts_a_good_file() {
        assert_eq!(check(&log(EMPTY)), Vec::<String>::new());
        assert_eq!(check(&log("### Added\n\n- A.")), Vec::<String>::new());
    }

    #[test]
    fn check_accepts_a_file_with_nothing_released() {
        let text = format!(
            "# Changelog\n\n## [Unreleased]\n\nNothing yet.\n\n[Unreleased]: {REPO}/commits/HEAD\n"
        );
        assert_eq!(check(&text), Vec::<String>::new());
    }

    fn problems(text: &str) -> String {
        check(text).join("\n")
    }

    #[test]
    fn check_wants_unreleased_first_and_once() {
        assert!(problems("# Changelog\n").contains("no \"## [Unreleased]\""));
        let swapped = log(EMPTY)
            .replace("## [Unreleased]\n\nNothing yet.\n\n", "")
            .replace(
                "### Added\n\n- The strip.\n\n",
                "### Added\n\n- The strip.\n\n## [Unreleased]\n\nNothing yet.\n\n",
            );
        assert!(problems(&swapped).contains("The first section must be"));
        let twice = log(EMPTY).replace(
            "## [0.1.0]",
            "## [Unreleased]\n\nNothing yet.\n\n## [0.1.0]",
        );
        assert!(problems(&twice).contains("more than one"));
    }

    #[test]
    fn check_wants_headings_under_unreleased() {
        assert!(problems(&log("- Loose line.")).contains("no \"### Added\""));
    }

    #[test]
    fn check_wants_well_formed_release_headings() {
        let bad_version = log(EMPTY).replace("## [0.1.0] - 2026-11-16", "## [0.1] - 2026-11-16");
        assert!(problems(&bad_version).contains("is not a version"));
        let bad_date = log(EMPTY).replace("2026-11-16", "2026-11-31");
        assert!(problems(&bad_date).contains("is not a date"));
        let no_date = log(EMPTY).replace("## [0.1.0] - 2026-11-16", "## 0.1.0");
        assert!(problems(&no_date).contains("should look like"));
    }

    #[test]
    fn check_wants_newest_first() {
        let text = format!(
            "## [Unreleased]\n\nNothing yet.\n\n## [0.1.0] - 2026-11-16\n\n### Added\n\n- A.\n\n\
             ## [0.2.0] - 2026-11-17\n\n### Fixed\n\n- B.\n\n\
             [Unreleased]: {REPO}/compare/v0.1.0...HEAD\n"
        );
        let found = problems(&text);
        assert!(found.contains("0.1.0 is listed above 0.2.0"), "{found}");
        assert!(
            found.contains("0.1.0 is dated 2026-11-16, before 0.2.0 (2026-11-17)"),
            "{found}"
        );
    }

    #[test]
    fn check_wants_known_unique_non_empty_subsections() {
        let text =
            log("### Added\n\n- A.\n\n### Added\n\n- B.\n\n### Improved\n\n- C.\n\n### Fixed\n");
        let found = problems(&text);
        assert!(found.contains("\"### Added\" twice"), "{found}");
        assert!(found.contains("\"### Improved\""), "{found}");
        assert!(found.contains("an empty \"### Fixed\""), "{found}");
    }

    #[test]
    fn check_wants_notes_in_every_release() {
        let text = log(EMPTY).replace("### Changed\n\n- Restyled.\n\n", "");
        assert!(problems(&text).contains("0.2.0 has no notes"));
    }

    #[test]
    fn check_wants_the_links_right() {
        let missing = log(EMPTY).replace(&format!("[0.2.0]: {REPO}/compare/v0.1.0...v0.2.0\n"), "");
        assert!(problems(&missing).contains("\"[0.2.0]: https://github.com/markcastle/buildrail/compare/v0.1.0...v0.2.0\" link is missing"));
        let wrong = log(EMPTY).replace("compare/v0.2.0...HEAD", "compare/v0.1.0...HEAD");
        assert!(problems(&wrong).contains("The [Unreleased] link points at"));
        let no_unreleased =
            log(EMPTY).replace(&format!("[Unreleased]: {REPO}/compare/v0.2.0...HEAD\n"), "");
        assert!(problems(&no_unreleased).contains("[Unreleased]: https://github.com/markcastle/buildrail/compare/v0.2.0...HEAD\" link at the bottom is missing"));
        let wrong_version = log(EMPTY).replace("releases/tag/v0.1.0", "releases/tag/v0.0.9");
        assert!(problems(&wrong_version).contains("The [0.1.0] link points at"));
        let stray = format!("{}[0.0.1]: {REPO}/releases/tag/v0.0.1\n", log(EMPTY));
        assert!(problems(&stray).contains("a [0.0.1] link but no"));
    }
}
