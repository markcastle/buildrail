//! The `buildrail` executable. Everything lives in the library; this only
//! hands the command line to the composition root
//! ([`buildrail_app::compose`]) and turns failures into an exit code.

// A release build on Windows is a GUI program: without this, double-clicking
// the exe would open a console window behind the strip. Debug builds keep the
// console so `cargo run` still shows logs. Release builds attach to the
// console of the terminal they were started from, if any (see below).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::process::ExitCode;

use buildrail_app::compose::{self, StartError};

fn main() -> ExitCode {
    // First, before anything prints: on Windows, show output in the
    // terminal that started us (double-clicked, there is none; nothing opens).
    buildrail_platform::console::attach_to_parent();
    let options = match compose::parse_args(std::env::args_os().skip(1)) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("buildrail: {error}");
            return ExitCode::from(2);
        }
    };
    for note in &options.notes {
        eprintln!("buildrail: {note}");
    }
    if options.help {
        println!("{}", compose::USAGE);
        return ExitCode::SUCCESS;
    }
    if options.version {
        println!("{}", compose::version_text());
        return ExitCode::SUCCESS;
    }
    match compose::run(&options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error @ StartError::Usage(_)) => {
            eprintln!("buildrail: {error}");
            ExitCode::from(2)
        }
        Err(error) => {
            tracing::error!(%error, "BuildRail stopped");
            eprintln!("buildrail: {error}");
            ExitCode::FAILURE
        }
    }
}
