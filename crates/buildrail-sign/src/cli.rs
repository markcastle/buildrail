//! Argument handling and the subcommands, with all I/O injected so tests
//! can drive every path without a process or the real environment.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

use buildrail_update::{
    encode_seed, is_known_target, payload_file_name, sha256_hex, sign, validate_asset_url, verify,
    Manifest, PublicKey, SigningKey, TargetEntry, Version, MAX_PAYLOAD_BYTES, PUBLIC_KEY_ENV,
    SCHEMA_VERSION, SIGNING_KEY_ENV, TARGETS,
};

/// Looks up an environment variable.
pub type EnvLookup<'a> = &'a dyn Fn(&str) -> Option<String>;
/// Fills a buffer from the operating system's random number generator.
pub type Random<'a> = &'a mut dyn FnMut(&mut [u8]) -> Result<(), String>;

const USAGE: &str = "\
Usage: buildrail-sign <command> [options]

Commands:
  generate      Print a new base64 signing seed (store it as BUILDRAIL_UPDATE_SIGNING_KEY).
  public-key    Print the base64 public key for BUILDRAIL_UPDATE_SIGNING_KEY.
  key-id        Print the key id for BUILDRAIL_UPDATE_SIGNING_KEY.
  manifest      --version <semver> --notes-url <url> --base-url <url>
                --payload <target>=<file.gz> [--payload ...]
                Sign the payloads with BUILDRAIL_UPDATE_SIGNING_KEY and print latest.json.
  verify        --manifest <latest.json> --payload <target>=<file.gz> [--payload ...]
                Check payloads against latest.json and BUILDRAIL_UPDATE_PUBLIC_KEY.
  help          Print this text.

Targets: windows-x86_64, macos-aarch64, macos-x86_64, linux-x86_64.
Exit codes: 0 ok, 1 failed, 2 usage or configuration error, 3 no key configured.
";

/// Why a command stopped. The message never contains key material.
#[derive(Debug, PartialEq, Eq)]
enum Failure {
    /// Exit 1: something was wrong with the inputs' contents or files.
    Failed(String),
    /// Exit 2: bad arguments or an invalid key.
    Usage(String),
    /// Exit 3: the key variable is missing or empty.
    NoKey(&'static str),
}

impl Failure {
    fn code(&self) -> u8 {
        match self {
            Self::Failed(_) => 1,
            Self::Usage(_) => 2,
            Self::NoKey(_) => 3,
        }
    }

    fn message(&self) -> String {
        match self {
            Self::Failed(text) | Self::Usage(text) => text.clone(),
            Self::NoKey(var) => format!("{var} is not set: no update signing key is configured"),
        }
    }
}

type Outcome = Result<String, Failure>;

/// Runs one command; returns the process exit code.
pub fn run(
    args: &[OsString],
    env: EnvLookup<'_>,
    random: Random<'_>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> u8 {
    let result = to_strings(args).and_then(|args| dispatch(&args, env, random));
    match result {
        Ok(text) => match out.write_all(text.as_bytes()) {
            Ok(()) => 0,
            Err(e) => report(
                err,
                &Failure::Failed(format!("could not write output: {e}")),
            ),
        },
        Err(failure) => report(err, &failure),
    }
}

fn report(err: &mut dyn Write, failure: &Failure) -> u8 {
    let _ = writeln!(err, "buildrail-sign: {}", failure.message());
    failure.code()
}

fn to_strings(args: &[OsString]) -> Result<Vec<String>, Failure> {
    args.iter()
        .map(|arg| {
            arg.to_str()
                .map(str::to_owned)
                .ok_or_else(|| Failure::Usage("arguments must be valid UTF-8".into()))
        })
        .collect()
}

fn dispatch(args: &[String], env: EnvLookup<'_>, random: Random<'_>) -> Outcome {
    let Some((command, rest)) = args.split_first() else {
        return Err(Failure::Usage(format!("no command given\n\n{USAGE}")));
    };
    match command.as_str() {
        "generate" => no_options(rest).and_then(|()| generate(random)),
        "public-key" => no_options(rest)
            .and_then(|()| signing_key(env))
            .map(|key| format!("{}\n", key.public_key().to_base64())),
        "key-id" => no_options(rest)
            .and_then(|()| signing_key(env))
            .map(|key| format!("{}\n", key.public_key().key_id())),
        "manifest" => manifest(
            &Options::parse(rest, &["version", "notes-url", "base-url"])?,
            env,
        ),
        "verify" => verify_command(&Options::parse(rest, &["manifest"])?, env),
        "help" | "--help" | "-h" => Ok(USAGE.to_owned()),
        other => Err(Failure::Usage(format!(
            "unknown command {other:?}\n\n{USAGE}"
        ))),
    }
}

fn no_options(rest: &[String]) -> Result<(), Failure> {
    match rest.first() {
        None => Ok(()),
        Some(extra) => Err(Failure::Usage(format!("unexpected argument {extra:?}"))),
    }
}

fn generate(random: Random<'_>) -> Outcome {
    let mut seed = [0u8; 32];
    random(&mut seed)
        .map_err(|e| Failure::Failed(format!("the system random generator failed: {e}")))?;
    Ok(format!("{}\n", encode_seed(&seed)))
}

fn key_text(env: EnvLookup<'_>, var: &'static str) -> Result<String, Failure> {
    env(var)
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .ok_or(Failure::NoKey(var))
}

fn signing_key(env: EnvLookup<'_>) -> Result<SigningKey, Failure> {
    SigningKey::from_base64_seed(&key_text(env, SIGNING_KEY_ENV)?).map_err(|e| {
        Failure::Usage(format!(
            "{SIGNING_KEY_ENV} is not a valid signing seed: {e}"
        ))
    })
}

fn public_key(env: EnvLookup<'_>) -> Result<PublicKey, Failure> {
    PublicKey::from_base64(&key_text(env, PUBLIC_KEY_ENV)?)
        .map_err(|e| Failure::Usage(format!("{PUBLIC_KEY_ENV} is not a valid public key: {e}")))
}

/// Parsed `--name value` / `--name=value` options plus repeated `--payload`s.
#[derive(Debug, Default)]
struct Options {
    single: BTreeMap<String, String>,
    payloads: Vec<(String, PathBuf)>,
}

impl Options {
    fn parse(args: &[String], allowed: &[&str]) -> Result<Self, Failure> {
        let mut options = Self::default();
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            let Some(flag) = arg.strip_prefix("--") else {
                return Err(Failure::Usage(format!("unexpected argument {arg:?}")));
            };
            let (name, value) = match flag.split_once('=') {
                Some((name, value)) => (name, value.to_owned()),
                None => {
                    let value = iter
                        .next()
                        .ok_or_else(|| Failure::Usage(format!("--{flag} needs a value")))?;
                    (flag, value.clone())
                }
            };
            if name == "payload" {
                options.payloads.push(parse_payload(&value)?);
            } else if allowed.contains(&name) {
                if options.single.insert(name.to_owned(), value).is_some() {
                    return Err(Failure::Usage(format!("--{name} given more than once")));
                }
            } else {
                return Err(Failure::Usage(format!("unknown option --{name}")));
            }
        }
        if options.payloads.is_empty() {
            return Err(Failure::Usage(
                "at least one --payload <target>=<file> is required".into(),
            ));
        }
        for name in allowed {
            options.required(name)?;
        }
        Ok(options)
    }

    fn required(&self, name: &str) -> Result<&str, Failure> {
        self.single
            .get(name)
            .map(String::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| Failure::Usage(format!("--{name} is required")))
    }
}

fn parse_payload(value: &str) -> Result<(String, PathBuf), Failure> {
    let (target, path) = value
        .split_once('=')
        .filter(|(_, path)| !path.is_empty())
        .ok_or_else(|| {
            Failure::Usage(format!("--payload must be <target>=<file>, got {value:?}"))
        })?;
    if !is_known_target(target) {
        return Err(Failure::Usage(format!(
            "unknown target {target:?}; expected one of {}",
            TARGETS.join(", ")
        )));
    }
    Ok((target.to_owned(), PathBuf::from(path)))
}

fn read_file(path: &Path, what: &str) -> Result<Vec<u8>, Failure> {
    std::fs::read(path)
        .map_err(|e| Failure::Failed(format!("cannot read {what} {}: {e}", path.display())))
}

fn file_name(path: &Path) -> Result<&str, Failure> {
    path.file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Failure::Usage(format!("{} has no usable file name", path.display())))
}

fn manifest(options: &Options, env: EnvLookup<'_>) -> Outcome {
    let version_text = options.required("version")?;
    let version = Version::parse(version_text)
        .map_err(|_| {
            Failure::Usage(format!(
                "--version {version_text:?} is not a semantic version (no leading v)"
            ))
        })?
        .to_string();
    let notes_url = options.required("notes-url")?;
    if !notes_url.starts_with("https://") {
        return Err(Failure::Usage(
            "--notes-url must be an https:// address".into(),
        ));
    }
    let base_url = options.required("base-url")?.trim_end_matches('/');
    let key = signing_key(env)?;

    let mut targets = BTreeMap::new();
    for (target, path) in &options.payloads {
        let name = file_name(path)?;
        let expected = payload_file_name(&version, target);
        if name != expected {
            return Err(Failure::Usage(format!(
                "the {target} payload must be named {expected}, got {name}"
            )));
        }
        let url = format!("{base_url}/{name}");
        validate_asset_url(&url).map_err(|e| Failure::Usage(format!("{url}: {e}")))?;
        let bytes = read_file(path, "payload")?;
        let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if size == 0 {
            return Err(Failure::Failed(format!(
                "payload {} is empty",
                path.display()
            )));
        }
        if size > MAX_PAYLOAD_BYTES {
            return Err(Failure::Failed(format!(
                "payload {} is {size} bytes; the updater accepts at most {MAX_PAYLOAD_BYTES}",
                path.display()
            )));
        }
        let sha256 = sha256_hex(&bytes);
        let signature = sign(&key, &version, target, &sha256);
        let entry = TargetEntry {
            url,
            sha256,
            size,
            signature,
        };
        if targets.insert(target.clone(), entry).is_some() {
            return Err(Failure::Usage(format!(
                "--payload for {target} given more than once"
            )));
        }
    }

    let manifest = Manifest {
        schema: SCHEMA_VERSION,
        version,
        notes_url: notes_url.to_owned(),
        key_id: key.public_key().key_id(),
        targets,
    };
    Ok(manifest.to_json())
}

fn verify_command(options: &Options, env: EnvLookup<'_>) -> Outcome {
    let key = public_key(env)?;
    let manifest_path = Path::new(options.required("manifest")?);
    let manifest = Manifest::parse(&read_file(manifest_path, "manifest")?)
        .map_err(|e| Failure::Failed(format!("{}: {e}", manifest_path.display())))?;
    let version = Version::parse(&manifest.version)
        .map_err(|_| {
            Failure::Failed(format!(
                "manifest version {:?} is not a semantic version",
                manifest.version
            ))
        })?
        .to_string();
    if manifest.key_id != key.key_id() {
        return Err(Failure::Failed(format!(
            "manifest key_id {} does not match the public key (key id {})",
            manifest.key_id,
            key.key_id()
        )));
    }

    let mut report = String::new();
    let mut problems = Vec::new();
    for (target, path) in &options.payloads {
        match check_payload(&manifest, &version, &key, target, path) {
            Ok(line) => report.push_str(&line),
            Err(problem) => problems.push(format!("{target}: {problem}")),
        }
    }
    if problems.is_empty() {
        Ok(report)
    } else {
        Err(Failure::Failed(format!(
            "verification failed\n  {}",
            problems.join("\n  ")
        )))
    }
}

fn check_payload(
    manifest: &Manifest,
    version: &str,
    key: &PublicKey,
    target: &str,
    path: &Path,
) -> Result<String, String> {
    let entry = manifest
        .targets
        .get(target)
        .ok_or_else(|| "not in the manifest".to_owned())?;
    validate_asset_url(&entry.url).map_err(|e| e.to_string())?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if !entry.url.ends_with(&format!("/{name}")) {
        return Err(format!("the manifest's address does not end in /{name}"));
    }
    let bytes = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let size = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if size != entry.size {
        return Err(format!("size is {size}, manifest says {}", entry.size));
    }
    let sha256 = sha256_hex(&bytes);
    if !sha256.eq_ignore_ascii_case(&entry.sha256) {
        return Err("SHA-256 does not match the manifest".into());
    }
    verify(key, version, target, &sha256, &entry.signature).map_err(|e| e.to_string())?;
    Ok(format!("ok {target}: {size} bytes, sha256 {sha256}\n"))
}

#[cfg(test)]
mod tests;
