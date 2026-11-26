use std::collections::HashMap;
use std::fs;

use buildrail_update::{Manifest, PublicKey, SigningKey};

use super::*;

const SEED: [u8; 32] = [9; 32];
const BASE: &str = "https://github.com/markcastle/buildrail/releases/download/v0.3.0";
const NOTES: &str = "https://github.com/markcastle/buildrail/releases/tag/v0.3.0";

struct Output {
    code: u8,
    stdout: String,
    stderr: String,
}

fn seed_b64() -> String {
    encode_seed(&SEED)
}

fn public_b64() -> String {
    SigningKey::from_seed(&SEED).public_key().to_base64()
}

fn run_with(args: &[&str], vars: &[(&str, &str)]) -> Output {
    let vars: HashMap<String, String> = vars
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    let env = move |name: &str| vars.get(name).cloned();
    let mut random = |buf: &mut [u8]| {
        buf.iter_mut()
            .enumerate()
            .for_each(|(i, b)| *b = u8::try_from(i).unwrap_or(0));
        Ok(())
    };
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = run(&args, &env, &mut random, &mut out, &mut err);
    Output {
        code,
        stdout: String::from_utf8(out).unwrap(),
        stderr: String::from_utf8(err).unwrap(),
    }
}

fn with_seed(args: &[&str]) -> Output {
    let seed = seed_b64();
    run_with(args, &[(SIGNING_KEY_ENV, seed.as_str())])
}

fn with_public(args: &[&str]) -> Output {
    let public = public_b64();
    run_with(args, &[(PUBLIC_KEY_ENV, public.as_str())])
}

/// Writes `buildrail-v0.3.0-{target}.gz` payloads into a temp dir.
fn payloads(targets: &[&str]) -> (tempfile::TempDir, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    let args = targets
        .iter()
        .map(|target| {
            let path = dir.path().join(payload_file_name("0.3.0", target));
            fs::write(&path, format!("gzip bytes for {target}")).unwrap();
            format!("{target}={}", path.display())
        })
        .collect();
    (dir, args)
}

fn manifest_args(payload_args: &[String]) -> Vec<&str> {
    let mut args = vec![
        "manifest",
        "--version",
        "0.3.0",
        "--notes-url",
        NOTES,
        "--base-url",
        BASE,
    ];
    for p in payload_args {
        args.push("--payload");
        args.push(p);
    }
    args
}

#[test]
fn generate_prints_a_usable_seed() {
    let out = run_with(&["generate"], &[]);
    assert_eq!(out.code, 0);
    assert!(out.stdout.ends_with('\n') && out.stdout.lines().count() == 1);
    assert!(SigningKey::from_base64_seed(out.stdout.trim()).is_ok());
    assert!(out.stderr.is_empty());
}

#[test]
fn generate_reports_rng_failure() {
    let env = |_: &str| None;
    let mut random = |_: &mut [u8]| Err("no entropy".to_owned());
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = run(&["generate".into()], &env, &mut random, &mut out, &mut err);
    assert_eq!(code, 1);
    assert!(String::from_utf8(err).unwrap().contains("no entropy"));
}

#[test]
fn public_key_and_key_id() {
    let out = with_seed(&["public-key"]);
    assert_eq!((out.code, out.stdout.trim()), (0, public_b64().as_str()));
    let out = with_seed(&["key-id"]);
    let id = PublicKey::from_base64(&public_b64()).unwrap().key_id();
    assert_eq!((out.code, out.stdout.trim()), (0, id.as_str()));
    // Surrounding whitespace in the secret is tolerated.
    let padded = format!(" {}\n", seed_b64());
    assert_eq!(
        run_with(&["key-id"], &[(SIGNING_KEY_ENV, padded.as_str())]).code,
        0
    );
}

#[test]
fn missing_or_empty_key_exits_3() {
    for vars in [
        &[][..],
        &[(SIGNING_KEY_ENV, "")][..],
        &[(SIGNING_KEY_ENV, "  \n")][..],
    ] {
        for command in ["public-key", "key-id"] {
            let out = run_with(&[command], vars);
            assert_eq!(out.code, 3, "{command}");
            assert_eq!(out.stderr.lines().count(), 1);
            assert!(out
                .stderr
                .contains("BUILDRAIL_UPDATE_SIGNING_KEY is not set"));
            assert!(out.stdout.is_empty());
        }
    }
    let (_dir, p) = payloads(&["linux-x86_64"]);
    assert_eq!(run_with(&manifest_args(&p), &[]).code, 3);
    let out = run_with(&["verify", "--manifest", "m.json", "--payload", &p[0]], &[]);
    assert_eq!(out.code, 3);
    assert!(out
        .stderr
        .contains("BUILDRAIL_UPDATE_PUBLIC_KEY is not set"));
}

#[test]
fn invalid_keys_exit_2_without_echoing_them() {
    let out = run_with(&["public-key"], &[(SIGNING_KEY_ENV, "c2VjcmV0LXNlZWQ=")]);
    assert_eq!(out.code, 2);
    assert!(!out.stderr.contains("c2VjcmV0LXNlZWQ"));
    assert!(out.stderr.contains("not a valid signing seed"));
    let out = run_with(
        &["verify", "--manifest", "m", "--payload", "linux-x86_64=x"],
        &[(PUBLIC_KEY_ENV, "nope")],
    );
    assert_eq!(out.code, 2);
    assert!(out.stderr.contains("not a valid public key"));
}

#[test]
fn the_seed_never_appears_in_any_output() {
    let seed = seed_b64();
    let (_dir, p) = payloads(&["linux-x86_64"]);
    for args in [
        vec!["public-key"],
        vec!["key-id"],
        manifest_args(&p),
        vec!["bogus"],
    ] {
        let out = with_seed(&args);
        assert!(!out.stdout.contains(&seed) && !out.stderr.contains(&seed));
    }
}

#[test]
fn usage_errors_exit_2() {
    let cases: &[&[&str]] = &[
        &[],
        &["bogus"],
        &["generate", "extra"],
        &["key-id", "--x"],
        &["manifest"],
        &["manifest", "--version"],
        &["manifest", "stray"],
        &["manifest", "--colour", "red", "--payload", "linux-x86_64=a"],
        &["manifest", "--payload", "linux-x86_64=a"],
        &[
            "manifest",
            "--version",
            "1.0.0",
            "--version",
            "1.0.0",
            "--payload",
            "linux-x86_64=a",
        ],
        &["manifest", "--payload", "linux-aarch64=a"],
        &["manifest", "--payload", "linux-x86_64"],
        &["manifest", "--payload", "linux-x86_64="],
        &["verify", "--payload", "linux-x86_64=a"],
        &["verify", "--manifest", "m"],
    ];
    for args in cases {
        let out = with_public(args);
        assert_eq!(out.code, 2, "{args:?}: {}", out.stderr);
        assert!(out.stderr.starts_with("buildrail-sign: "), "{args:?}");
    }
}

#[test]
fn non_utf8_arguments_are_a_usage_error() {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt as _;
        let env = |_: &str| None;
        let mut random = |_: &mut [u8]| Ok(());
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let bad = OsString::from_vec(vec![0xff, 0xfe]);
        assert_eq!(run(&[bad], &env, &mut random, &mut out, &mut err), 2);
    }
}

#[test]
fn help_prints_usage() {
    for flag in ["help", "--help", "-h"] {
        let out = run_with(&[flag], &[]);
        assert_eq!(out.code, 0);
        assert!(out.stdout.contains("Exit codes"));
    }
}

#[test]
fn manifest_then_verify_round_trip() {
    let (dir, p) = payloads(&[
        "linux-x86_64",
        "windows-x86_64",
        "macos-aarch64",
        "macos-x86_64",
    ]);
    let out = with_seed(&manifest_args(&p));
    assert_eq!(out.code, 0, "{}", out.stderr);
    let manifest = Manifest::parse(out.stdout.as_bytes()).unwrap();
    assert_eq!(manifest.schema, 1);
    assert_eq!(manifest.version, "0.3.0");
    assert_eq!(manifest.notes_url, NOTES);
    assert_eq!(manifest.targets.len(), 4);
    let linux = &manifest.targets["linux-x86_64"];
    assert_eq!(
        linux.url,
        format!("{BASE}/buildrail-v0.3.0-linux-x86_64.gz")
    );
    assert_eq!(linux.size, "gzip bytes for linux-x86_64".len() as u64);
    assert_eq!(linux.sha256, sha256_hex(b"gzip bytes for linux-x86_64"));

    let manifest_path = dir.path().join("latest.json");
    fs::write(&manifest_path, &out.stdout).unwrap();
    let mut args = vec![
        "verify".to_owned(),
        format!("--manifest={}", manifest_path.display()),
    ];
    for payload in &p {
        args.push(format!("--payload={payload}"));
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = with_public(&args);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(
        out.stdout.lines().filter(|l| l.starts_with("ok ")).count(),
        4
    );
}

#[test]
fn trailing_slash_on_base_url_is_fine() {
    let (_dir, p) = payloads(&["linux-x86_64"]);
    let base = format!("{BASE}/");
    let args = [
        "manifest",
        "--version",
        "0.3.0",
        "--notes-url",
        NOTES,
        "--base-url",
        &base,
        "--payload",
        &p[0],
    ];
    let out = with_seed(&args);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out
        .stdout
        .contains("v0.3.0/buildrail-v0.3.0-linux-x86_64.gz"));
}

#[test]
fn manifest_rejects_bad_inputs() {
    let (dir, p) = payloads(&["linux-x86_64"]);
    let seed = seed_b64();
    let vars = [(SIGNING_KEY_ENV, seed.as_str())];
    let check = |args: Vec<&str>, code: u8, needle: &str| {
        let out = run_with(&args, &vars);
        assert_eq!(out.code, code, "{args:?}: {}", out.stderr);
        assert!(out.stderr.contains(needle), "{args:?}: {}", out.stderr);
    };
    let swap = |from: &str, to: &str| -> Vec<String> {
        manifest_args(&p)
            .iter()
            .map(|a| {
                if *a == from {
                    to.to_owned()
                } else {
                    (*a).to_owned()
                }
            })
            .collect()
    };
    let owned = swap("0.3.0", "v0.3.0");
    check(
        owned.iter().map(String::as_str).collect(),
        2,
        "not a semantic version",
    );
    let owned = swap(NOTES, "http://example.com");
    check(
        owned.iter().map(String::as_str).collect(),
        2,
        "--notes-url must be",
    );
    let owned = swap(BASE, "https://example.com/dl");
    check(
        owned.iter().map(String::as_str).collect(),
        2,
        "not a BuildRail release",
    );
    // Duplicate target.
    let mut args = manifest_args(&p);
    args.extend(["--payload", p[0].as_str()]);
    check(args, 2, "more than once");
    // Wrong file name for the version.
    let wrong = dir.path().join("buildrail-v0.2.0-linux-x86_64.gz");
    fs::write(&wrong, b"x").unwrap();
    let arg = format!("linux-x86_64={}", wrong.display());
    check(
        manifest_args(std::slice::from_ref(&arg)),
        2,
        "must be named buildrail-v0.3.0-linux-x86_64.gz",
    );
    // Missing file (exit 1).
    let missing = format!(
        "macos-x86_64={}",
        dir.path()
            .join("buildrail-v0.3.0-macos-x86_64.gz")
            .display()
    );
    check(
        manifest_args(std::slice::from_ref(&missing)),
        1,
        "cannot read payload",
    );
    // Empty file (exit 1).
    let empty = dir.path().join("buildrail-v0.3.0-windows-x86_64.gz");
    fs::write(&empty, b"").unwrap();
    let arg = format!("windows-x86_64={}", empty.display());
    check(manifest_args(std::slice::from_ref(&arg)), 1, "is empty");
    // A path with no file name.
    check(
        manifest_args(&["linux-x86_64=/".to_owned()]),
        2,
        "no usable file name",
    );
}

#[cfg(unix)]
#[test]
fn manifest_rejects_oversized_payloads() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("buildrail-v0.3.0-linux-x86_64.gz");
    let file = fs::File::create(&path).unwrap();
    file.set_len(MAX_PAYLOAD_BYTES + 1).unwrap(); // sparse: no real disk use
    drop(file);
    let arg = format!("linux-x86_64={}", path.display());
    let out = with_seed(&manifest_args(std::slice::from_ref(&arg)));
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("accepts at most"));
}

/// A signed single-target release in a temp dir: (dir, manifest path, payload arg).
fn signed_release() -> (tempfile::TempDir, std::path::PathBuf, String) {
    let (dir, p) = payloads(&["linux-x86_64"]);
    let out = with_seed(&manifest_args(&p));
    let path = dir.path().join("latest.json");
    fs::write(&path, out.stdout).unwrap();
    (dir, path, p[0].clone())
}

fn verify_out(manifest: &Path, payload: &str) -> Output {
    let m = manifest.display().to_string();
    with_public(&["verify", "--manifest", &m, "--payload", payload])
}

fn edit_manifest(path: &Path, edit: impl FnOnce(&mut Manifest)) {
    let mut manifest = Manifest::parse(&fs::read(path).unwrap()).unwrap();
    edit(&mut manifest);
    fs::write(path, manifest.to_json()).unwrap();
}

#[test]
fn verify_catches_tampering() {
    // Tampered payload bytes, same length.
    let (_dir, m, p) = signed_release();
    let file = p.split_once('=').unwrap().1;
    let mut bytes = fs::read(file).unwrap();
    bytes[0] ^= 1;
    fs::write(file, bytes).unwrap();
    let out = verify_out(&m, &p);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("SHA-256 does not match"));

    // Payload of a different size.
    let (_dir, m, p) = signed_release();
    fs::write(p.split_once('=').unwrap().1, b"short").unwrap();
    assert!(verify_out(&m, &p).stderr.contains("size is 5"));

    // Version bumped in the manifest after signing.
    let (_dir, m, p) = signed_release();
    edit_manifest(&m, |x| x.version = "0.4.0".into());
    assert!(verify_out(&m, &p)
        .stderr
        .contains("signature does not match"));

    // Manifest URL pointing elsewhere.
    let (_dir, m, p) = signed_release();
    edit_manifest(&m, |x| {
        x.targets.get_mut("linux-x86_64").unwrap().url = "https://evil.example/a.gz".into()
    });
    assert!(verify_out(&m, &p)
        .stderr
        .contains("not a BuildRail release"));

    // Manifest URL naming another file.
    let (_dir, m, p) = signed_release();
    edit_manifest(&m, |x| {
        x.targets.get_mut("linux-x86_64").unwrap().url = format!("{BASE}/other.gz");
    });
    assert!(verify_out(&m, &p).stderr.contains("does not end in"));

    // Target not in the manifest (relabelled as another platform).
    let (_dir, m, p) = signed_release();
    let relabelled = p.replacen("linux-x86_64", "windows-x86_64", 1);
    assert!(verify_out(&m, &relabelled)
        .stderr
        .contains("windows-x86_64: not in the manifest"));

    // Missing payload file.
    let (dir, m, _) = signed_release();
    let gone = format!(
        "linux-x86_64={}",
        dir.path()
            .join("buildrail-v0.3.0-linux-x86_64.gz.missing")
            .display()
    );
    let out = verify_out(&m, &gone);
    assert_eq!(out.code, 1);
}

#[test]
fn verify_rejects_wrong_keys_and_bad_manifests() {
    let (dir, m, p) = signed_release();
    let other = SigningKey::from_seed(&[1; 32]).public_key().to_base64();
    let ms = m.display().to_string();
    let out = run_with(
        &["verify", "--manifest", &ms, "--payload", &p],
        &[(PUBLIC_KEY_ENV, other.as_str())],
    );
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("does not match the public key"));

    // Key id forged to match, but signatures are by another key.
    edit_manifest(&m, |x| {
        x.key_id = PublicKey::from_base64(&other).unwrap().key_id()
    });
    let out = run_with(
        &["verify", "--manifest", &ms, "--payload", &p],
        &[(PUBLIC_KEY_ENV, other.as_str())],
    );
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("signature does not match"));

    let bad = dir.path().join("bad.json");
    fs::write(&bad, "{\"schema\": 2}").unwrap();
    let out = verify_out(&bad, &p);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("needs a newer BuildRail"));

    edit_manifest(&m, |x| x.version = "latest".into());
    assert!(verify_out(&m, &p)
        .stderr
        .contains("is not a semantic version"));

    let out = verify_out(&dir.path().join("absent.json"), &p);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("cannot read manifest"));
}

#[test]
fn write_failures_are_reported() {
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("pipe closed"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let env = |_: &str| None;
    let mut random = |_: &mut [u8]| Ok(());
    let mut err = Vec::new();
    let code = run(&["help".into()], &env, &mut random, &mut Broken, &mut err);
    assert_eq!(code, 1);
    assert!(String::from_utf8(err).unwrap().contains("pipe closed"));
}
