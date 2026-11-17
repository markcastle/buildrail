//! `cargo run -p xtask -- <command>`: see `xtask::cli::USAGE`.

use std::process::ExitCode;

use xtask::cli::{run, Context};
use xtask::date::Date;
use xtask::system::{repository_root, DiskFiles, ProcessRunner};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = repository_root();
    let mut runner = ProcessRunner { root: root.clone() };
    let mut files = DiskFiles::new(root);
    let mut out = std::io::stdout();
    let mut err = std::io::stderr();
    let mut cx = Context {
        runner: &mut runner,
        files: &mut files,
        out: &mut out,
        err: &mut err,
        today: Date::today_utc(),
    };
    ExitCode::from(run(&args, &mut cx))
}
