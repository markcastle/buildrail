//! The real binary: exit codes as the release workflow sees them.

#![allow(clippy::unwrap_used)]

use std::process::Command;

fn sign() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_buildrail-sign"));
    command
        .env_remove("BUILDRAIL_UPDATE_SIGNING_KEY")
        .env_remove("BUILDRAIL_UPDATE_PUBLIC_KEY");
    command
}

#[test]
fn generate_then_derive_the_public_key() {
    let out = sign().arg("generate").output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let seed = String::from_utf8(out.stdout).unwrap();
    let out = sign()
        .arg("public-key")
        .env("BUILDRAIL_UPDATE_SIGNING_KEY", seed.trim())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim().len(), 44);
}

#[test]
fn no_key_configured_exits_3() {
    let out = sign().arg("key-id").output().unwrap();
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(String::from_utf8(out.stderr).unwrap().lines().count(), 1);
}

#[test]
fn usage_error_exits_2() {
    assert_eq!(
        sign().arg("nonsense").output().unwrap().status.code(),
        Some(2)
    );
}
