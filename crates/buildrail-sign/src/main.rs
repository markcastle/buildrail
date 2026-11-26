//! `buildrail-sign`: the release-time half of BuildRail's self-update.
//!
//! Generates the signing key, signs each platform's gzip-compressed
//! executable, writes `latest.json`, and checks the result before the
//! release workflow publishes it. See the `buildrail-update` crate docs for
//! the trust model and file formats, and `buildrail-sign help` for usage.
//!
//! Exit codes: 0 success, 1 failure (unreadable file, verification
//! failed), 2 usage or configuration error (bad arguments, invalid key),
//! 3 no key configured (the workflow then releases without updater files).

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod cli;

use std::io::{self, Write as _};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let env = |name: &str| std::env::var(name).ok();
    let mut random = |buf: &mut [u8]| getrandom::fill(buf).map_err(|e| e.to_string());
    let mut out = io::stdout().lock();
    let mut err = io::stderr().lock();
    let code = cli::run(&args, &env, &mut random, &mut out, &mut err);
    let _ = out.flush();
    ExitCode::from(code)
}
