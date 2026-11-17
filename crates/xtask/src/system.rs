//! The real [`Runner`] and [`Files`]: processes and the disk.

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::release::{Files, Output, Runner};

/// The repository root: two levels above this crate's manifest.
pub fn repository_root() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .and_then(Path::parent)
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// Runs programs in the repository root.
pub struct ProcessRunner {
    /// Where commands run.
    pub root: PathBuf,
}

impl Runner for ProcessRunner {
    fn run(&mut self, program: &str, args: &[&str]) -> io::Result<Output> {
        let output = Command::new(program)
            .args(args)
            .current_dir(&self.root)
            .output()?;
        Ok(Output {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// Files under the repository root. Relative paths are taken from the root;
/// absolute ones are used as they are.
///
/// A checkout on Windows may have CRLF line endings. The pure functions
/// work in LF, so a file read with CRLF is handed over as LF and written
/// back with CRLF, and git sees only the lines that really changed.
pub struct DiskFiles {
    /// The repository root.
    pub root: PathBuf,
    crlf: BTreeSet<PathBuf>,
}

impl DiskFiles {
    /// Files under `root`.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            crlf: BTreeSet::new(),
        }
    }
}

impl Files for DiskFiles {
    fn read(&self, path: &str) -> io::Result<String> {
        let text = fs::read_to_string(self.root.join(path))?;
        Ok(text.replace("\r\n", "\n"))
    }

    fn write(&mut self, path: &str, contents: &str) -> io::Result<()> {
        let full = self.root.join(path);
        let crlf = self.crlf.contains(&full)
            || fs::read(&full).is_ok_and(|bytes| bytes.windows(2).any(|w| w == b"\r\n"));
        if crlf {
            self.crlf.insert(full.clone());
            fs::write(&full, contents.replace("\r\n", "\n").replace('\n', "\r\n"))
        } else {
            fs::write(&full, contents)
        }
    }

    fn append(&mut self, path: &str, contents: &str) -> io::Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join(path))?;
        file.write_all(contents.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn scratch() -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "xtask-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_root_holds_the_workspace_manifest() {
        let root = repository_root();
        let manifest = fs::read_to_string(root.join("Cargo.toml")).unwrap();
        assert!(manifest.contains("[workspace.package]"));
    }

    #[test]
    fn runs_a_program_and_reports_how_it_went() {
        let mut runner = ProcessRunner {
            root: repository_root(),
        };
        let ok = runner.run("git", &["--version"]).unwrap();
        assert!(ok.success);
        assert!(ok.stdout.starts_with("git version"));
        let bad = runner.run("git", &["no-such-command"]).unwrap();
        assert!(!bad.success);
        assert!(!bad.stderr.is_empty());
        assert!(runner.run("no-such-program-xtask", &[]).is_err());
    }

    #[test]
    fn reads_writes_and_appends() {
        let dir = scratch();
        let mut files = DiskFiles::new(dir.clone());
        files.write("a.md", "one\ntwo\n").unwrap();
        assert_eq!(files.read("a.md").unwrap(), "one\ntwo\n");
        files.append("out.txt", "x=1\n").unwrap();
        files.append("out.txt", "y=2\n").unwrap();
        assert_eq!(
            fs::read_to_string(dir.join("out.txt")).unwrap(),
            "x=1\ny=2\n"
        );
        let absolute = dir.join("abs.txt");
        files.append(absolute.to_str().unwrap(), "z\n").unwrap();
        assert_eq!(fs::read_to_string(&absolute).unwrap(), "z\n");
        assert!(files.read("missing.md").is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn keeps_crlf_files_crlf() {
        let dir = scratch();
        fs::write(dir.join("c.md"), "one\r\ntwo\r\n").unwrap();
        let mut files = DiskFiles::new(dir.clone());
        let text = files.read("c.md").unwrap();
        assert_eq!(text, "one\ntwo\n");
        files.write("c.md", &format!("{text}three\n")).unwrap();
        assert_eq!(
            fs::read(dir.join("c.md")).unwrap(),
            b"one\r\ntwo\r\nthree\r\n"
        );
        // Remembered even after the file on disk has changed.
        files.write("c.md", "four\n").unwrap();
        files.write("c.md", "five\n").unwrap();
        assert_eq!(fs::read(dir.join("c.md")).unwrap(), b"five\r\n");
        fs::remove_dir_all(dir).unwrap();
    }
}
