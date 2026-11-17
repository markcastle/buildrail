# Releasing BuildRail

How BuildRail ships: one button cuts a release, installed copies update themselves, and a second button undoes a bad release. This page is written for someone who has never seen the repository. If you only want the routine, read [Day to day](#day-to-day) and stop there.

## What you get

- **Releasing is one click.** Open Actions, pick Release, press Run. No terminal, no version number to choose, no tag to make.
- **The changelog decides the version.** Something new makes a minor release; changes and fixes make a patch.
- **The release notes write themselves.** They are the changelog's section for that version, written for the people using BuildRail.
- **A release is never half there.** It stays a draft until every download is on it and matches its checksum, and only then becomes "latest".
- **Installed copies keep themselves current.** Each one reads a small signed file on the latest release, and only installs a download whose signature checks out.
- **A bad release can be undone from a button,** including for people who already installed it.
- **A broken main cannot be released.** The button refuses unless CI passed on that exact commit.
- **Nothing is ever typed twice.** The version in `Cargo.toml`, `Cargo.lock`, the changelog and the tag are set by one command, together, so they cannot disagree.

## The idea in one paragraph

Write what changed into `CHANGELOG.md` as the work lands. That one habit drives everything else. The release command reads it to choose the version number, stamps it with the version and the date, and the workflow copies that section into the GitHub release. The installed app reads `latest.json` from the latest release to learn there is something newer. Nobody types a version, writes release notes at release time, makes a tag or uploads a file by hand.

## How a release flows

1. A pull request lands on `main` with a line under **Unreleased** in the changelog.
2. CI runs on that commit and passes.
3. Someone presses **Release**.
4. The workflow works out the version, stamps the changelog, sets the version in `Cargo.toml` and `Cargo.lock`, commits `Release vX.Y.Z`, tags it, and pushes both.
5. It creates the GitHub release as a **draft** straight away, with the changelog section as its notes.
6. Four runners build BuildRail for Windows, Apple silicon Macs, Intel Macs and Linux, each from the tag, and upload two files each to the draft.
7. Once every build has succeeded, it signs the update files into `latest.json`, writes `SHA256SUMS.txt`, checks that everything is on the draft and matches, and **publishes** it. GitHub now calls it "latest".
8. Running copies of BuildRail find `latest.json`, see a higher version, check its signature and update.

It takes about twenty minutes, nearly all of it building.

## Day to day

**While working:** add a line under `## [Unreleased]` in `CHANGELOG.md`, in the same pull request as the change. A check on the pull request asks for it when the app's code changed. If nobody using BuildRail would notice the change, add the label `no-changelog` to the pull request instead.

**To release:** open the repository's **Actions** tab, pick **Release**, press **Run workflow**, leave the version blank, and press the green **Run workflow** button. The run's page says which number it chose and why.

**To check it worked:** the release page lists the downloads, `SHA256SUMS.txt` and `latest.json`, and is marked "Latest".

**If a release is bad:** open Actions, pick **Roll back**, give the last good version and a sentence on why. See [Roll back](#roll-back).

**If the button refuses,** it says why in one line at the top of the run: CI still running, CI failed, nothing under Unreleased, not on `main`, a version that is not higher than the last, or a problem with the signing key. Fix that and press Run again.

## Writing the changelog line

The format is [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), with `## [Unreleased]` always at the top:

```markdown
## [Unreleased]

### Added

- **BuildRail updates itself.** It checks for a newer release while it runs...

### Fixed

- **Starting BuildRail a second time no longer opens a second strip.**
```

Three rules make it work:

- **Write for the person using BuildRail.** Say what they will see, not which function moved. These lines become the release notes word for word. Sentence case and British spelling, as in the rest of the app.
- **One change, one line, in the same pull request.** Not at release time, when nobody remembers.
- **File it under the right heading,** because the heading decides the version number.

| Unreleased lists | The release is | From 0.3.2 |
| --- | --- | --- |
| Something **Added** or **Removed** | A minor release | 0.4.0 |
| Only **Changed**, **Fixed**, **Deprecated** or **Security** | A patch | 0.3.3 |

A new ability goes under Added even when it is small. A change to something that already exists goes under Changed. **Deprecated** (something will go in a later release) is a patch because nothing has gone yet; the release that removes it lists it under Removed and is a minor. **Security** fixes are fixes, so a patch.

You can always overrule the rule by typing a version in the Release form. That is how 1.0.0 happens: leaving 0.x is a judgement no heading can make.

CI checks the file's shape on every pull request: Unreleased first, versions newest first and each once, real dates, only the six headings above, and the compare links at the bottom.

## Rehearse

Tick **Rehearse** in the Release form to run everything except the push, the draft, the builds and publishing. It can run from any branch, so it is the safe way to:

- ask "what number would the next release be, and why?";
- check the signing key secret is set and readable;
- see the exact release notes;
- try a change to the Release workflow itself before merging it.

A rehearsal also reports whether CI passed on the commit, as a warning rather than a stop. The run's page shows the version, the reason, the signing state and the notes.

## Roll back

An installed copy only ever updates to a **higher** version. A copy that took a bad 0.5.0 will ignore 0.4.2 for ever, however the releases are arranged. So a rollback is a new, higher release that carries the old code.

Open Actions, pick **Roll back**, and give the last good version (such as `0.4.2`) and one sentence on why. The workflow then:

1. **Withdraws every release newer than the good one** by marking it a pre-release. GitHub's "latest" skips pre-releases, so straight away the update feed and the one-line installers hand out the good version again, and copies that had not updated yet are no longer offered the bad one.
2. **Puts the good version's code back on `main`** as one commit. The release tooling stays as it is today (`.github`, `crates/xtask`, `CHANGELOG.md`, `scripts`, this page), since that is what is running. Unreleased in the changelog becomes one line saying what happened and why.
3. **Releases it** as the next patch number (0.5.1 in the example), so copies that took 0.5.0 move to the good code too. CI is not checked, because that code was released before.

Work that landed after the good version is gone from `main` afterwards, but not lost: revert the "Roll back to" commit to bring it back, changelog lines included, fix it, and release again.

It refuses to roll back to a version from before BuildRail could update itself, because everyone who took that release could never update again.

## Update signing

Installed copies only install an update whose signature checks out against a public key built into them. The private half of that key lives in one repository secret. Until that secret exists, releases still go out, but their builds cannot update themselves, and the release run says so loudly.

### One-time setup

This takes two minutes and needs nothing installed.

1. **Make the key.** Open PowerShell (Start, type PowerShell, press Enter) and paste this one line:

    ```powershell
    $b = New-Object byte[] 32; [Security.Cryptography.RandomNumberGenerator]::Create().GetBytes($b); Set-Clipboard ([Convert]::ToBase64String($b)); $b = $null
    ```

    Nothing appears, and that is right: the key is now on your clipboard and has not been shown on screen. It is 32 random bytes from Windows' cryptographic random number generator, written as one line of text.

    On macOS or Linux the same is `openssl rand -base64 32 | pbcopy` (macOS) or `openssl rand -base64 32 | xclip -selection clipboard` (Linux).

2. **Save it in your password manager first.** Paste it into a new entry called "BuildRail update signing key". GitHub will never show it to you again. If it is lost, installed copies can no longer verify updates, and everyone has to reinstall BuildRail by hand once.

3. **Give it to GitHub.** Open [the new secret page](https://github.com/markcastle/buildrail/settings/secrets/actions/new). For **Name** type:

    ```text
    BUILDRAIL_UPDATE_SIGNING_KEY
    ```

    For **Secret**, paste. Press **Add secret**.

4. **Copy something else,** so the key does not linger on your clipboard.

5. **Check it.** Run the Release workflow with **Rehearse** ticked. The run's page should say "Update signing: configured" and give a key id, a short code that names your key without revealing it.

From then on every release is signed. The public half is worked out from the secret during each release and built into the app; there is nothing else to set.

### What signing does and does not do

**It does** prove that an update came from this repository's release process, for that version and that platform. A tampered download, a file swapped on the release, an old build relabelled as new, or the Windows build offered to a Mac are all refused, and nothing is installed.

**It does not** replace operating-system code signing. Someone who downloads BuildRail by hand still meets Windows SmartScreen or macOS Gatekeeper the first time they run it; that needs a Windows code signing certificate and an Apple Developer ID, which BuildRail does not have yet. Updates through the app are not affected.

**It does not** make a release good. It proves where it came from. A bad release is undone with [Roll back](#roll-back).

### If the key is lost, or you must change it

Make a new key and replace the secret as above, then run Release with **I replaced the update signing key on purpose** ticked. Without that tick the release stops, because a changed key is usually an accident: the run compares the new key's id with the one in the latest release's `latest.json`.

Copies already installed carry the old public key, so they refuse the new release. They fail safe, nothing is installed, and people install that one release by hand (the one-line install does it). After that they update normally again.

If the secret is deleted after releases have been signed, the next release stops too: shipping it unsigned would leave every installed copy without an update feed.

## What a release contains

For each platform, a download for people and an update file for the app:

| Platform | Download | Update file |
| --- | --- | --- |
| Windows (64-bit) | `buildrail-vX.Y.Z-windows-x86_64.zip` | `buildrail-vX.Y.Z-windows-x86_64.gz` |
| macOS, Apple silicon | `buildrail-vX.Y.Z-macos-aarch64.tar.gz` | `buildrail-vX.Y.Z-macos-aarch64.gz` |
| macOS, Intel | `buildrail-vX.Y.Z-macos-x86_64.tar.gz` | `buildrail-vX.Y.Z-macos-x86_64.gz` |
| Linux (64-bit) | `buildrail-vX.Y.Z-linux-x86_64.tar.gz` | `buildrail-vX.Y.Z-linux-x86_64.gz` |

The Windows zip holds `buildrail.exe`, a `Try the demo.cmd` to double-click, the README and the changelog. The others hold the `buildrail` program, the README and the changelog. Each update file is the bare program, gzipped.

Then `SHA256SUMS.txt`, the SHA-256 of every file, and, when signing is set up, `latest.json`, which installed copies read at `https://github.com/markcastle/buildrail/releases/latest/download/latest.json`. GitHub redirects that address to whichever release is "latest", so there is no update server to run.

The Linux build is made on Ubuntu 22.04, the oldest Ubuntu that GitHub still runs, so it needs only glibc 2.35 and runs on more distributions than a build made on the newest. The Intel Mac build is made on an Apple silicon runner, which builds for both.

## One-line install

The same line installs and updates. Windows, in PowerShell:

```powershell
irm https://raw.githubusercontent.com/markcastle/buildrail/main/scripts/install.ps1 | iex
```

macOS and Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/markcastle/buildrail/main/scripts/install.sh | sh
```

Both scripts:

1. ask GitHub for the latest release and pick this machine's download by the end of its name, never the full name, which carries the version;
2. download it and `SHA256SUMS.txt`, and stop unless the checksum matches;
3. stop a running copy;
4. install for the current user only, with no administrator prompt: `%LOCALAPPDATA%\Programs\BuildRail\buildrail.exe` with a Start menu entry on Windows, `~/.local/bin/buildrail` on macOS and Linux (clearing the macOS download quarantine);
5. start it.

While the repository is private, set `GITHUB_TOKEN` to a token that can read it first; the scripts then download through the API.

## The release command in a terminal

The Release workflow runs the same tool you can run yourself. It is the `xtask` crate, which has no dependencies and builds in seconds:

```bash
cargo run -p xtask -- release --dry-run
```

That fetches tags, checks the changelog and says what the next release would be and why, without changing anything. The other commands:

| Command | What it does |
| --- | --- |
| `next 0.3.2` | Prints the version after 0.3.2, with the reason on standard error |
| `changelog check` | Checks the changelog's shape, as CI does |
| `changelog section 0.4.0` | Prints that version's notes (or `Unreleased`) |
| `changelog release 0.4.0` | Dates Unreleased as 0.4.0 and opens a fresh one |
| `changelog note "..."` | Adds a line under Changed in Unreleased; the rollback uses it |
| `release [version]` | Cuts a release here: stamps, sets the version, updates `Cargo.lock`, commits, tags |
| `highest-tag` | Prints the highest `vX.Y.Z` tag |

A real `release` in a terminal commits and tags in your checkout and pushes nothing. Pushing that tag would build nothing either: releases are cut by the button, so that the tag, the version and the changelog always come from one place. Use it in a scratch clone to see what the release commit looks like; it prints how to undo it.

## The pieces

| File | What it does |
| --- | --- |
| `CHANGELOG.md` | The source of the release notes and of the version number |
| `crates/xtask` | The release command and the changelog rules, with tests |
| `.github/workflows/release.yml` | The Release button: prepare, build, publish |
| `.github/workflows/rollback.yml` | The Roll back button |
| `.github/workflows/ci.yml` | The gate the Release button checks; also lints the workflows and checks the changelog |
| `.github/workflows/changelog.yml` | Asks a pull request that changes the app for a changelog line |
| `.github/workflows/test-build.yml` | A Windows build on every pull request, to try before merging |
| `crates/buildrail-sign` | Signs the update files into `latest.json` and checks the result |
| `crates/buildrail-update` | What the app does with `latest.json` |
| `scripts/install.ps1`, `scripts/install.sh` | The one-line install and update |

## The traps

Most of these cost a failed or confusing release somewhere before they were understood. Each is handled in the pipeline; the right-hand column says how.

| What you would see | Why | How it is handled |
| --- | --- | --- |
| A tag and the version in `Cargo.toml` disagree | The tag and the version were set by hand, separately | Only the Release button cuts a release, and one command sets the changelog, `Cargo.toml`, `Cargo.lock` and the tag together. There is no trigger on pushing a tag. Each build checks the tag's `Cargo.toml` says the version being built |
| A tag pushed by the workflow starts no build | GitHub does not start workflows for events made with the workflow's own token | Build in the same run, after the push |
| A tag on GitHub without its release commit on `main`, or the reverse | Two separate pushes, and the second failed | One `git push --atomic` for both |
| "Resource not accessible by integration" when creating the release | The workflow token may only create a release while the tag's commit matches `main`'s workflow files, and `main` had moved on | Create the release as a draft the moment the tag exists |
| A published release with nothing to download | It was published before the builds finished, and one failed | Draft first, publish last, after checking every expected file is on the draft and matches `SHA256SUMS.txt` |
| A build cannot find the draft, or makes a second release | Looking a release up by tag through the API does not see drafts | Upload with `gh release upload`, which finds drafts; no third-party release action |
| Installed copies ignore the release you rolled back to | Updaters only install a higher version | A rollback is a new, higher release of the old code |
| The rollback releases the wrong commit | A called workflow checks out the commit its caller started on, not the one the caller just pushed | The rollback passes its new commit to the Release workflow |
| The rollback waits for ever, or is cancelled as a deadlock | Caller and called workflow both hold the `release` concurrency group | The rollback holds it only in its own job, then hands over |
| A rollback strands everyone on old code | The good version predates self-update | The rollback refuses such a version |
| The next number is wrong when worked out from a laptop | The checkout had not fetched tags made on GitHub | The release command fetches tags first |
| CI never ran on the commit being released | Commits pushed by the workflow do not trigger CI | Check CI on the commit a person merged; skip the check for a rollback |
| The release stops at "the lock file needs to be updated" | `Cargo.lock` on `main` disagrees with `Cargo.toml` | Every build and the release command use `--locked`, so it is caught before anything is pushed. Commit an up-to-date `Cargo.lock` |
| Installed copies stop updating after the key secret was replaced | Each copy only trusts the key it was built with | The release compares key ids with the latest `latest.json` and stops unless the change is ticked as deliberate |
| Installed copies stop hearing about updates | The key secret was deleted, so the release had no `latest.json` | The release stops if the latest release was signed and the secret is now missing |
| An unusable key quietly ships an unsigned release | A typo in the secret | "Not set" and "set but unusable" are different answers; only "not set" releases unsigned |
| The Windows update file is not really gzip | Zip tools are not gzip | Git Bash's `gzip`, then `gzip -t` to prove it |
| An action warns it runs on Node 20 | `upload-artifact@v5` still runs on Node 20 | `checkout@v7`, `upload-artifact@v7`, `download-artifact@v8`, all on Node 24 |
| The install script cannot find the download | The file name carries a version that changes | Match by the end of the name, such as `-windows-x86_64.zip` |
| A broken or tampered download is installed | Nothing checked it | The install scripts check the SHA-256 against `SHA256SUMS.txt`; the app checks the signature |
| Release notes are a wall of commit messages | They were generated from commits | The notes are the changelog section |
| Nobody is sure what number to type | A rule that lives in someone's head | The changelog decides |
| Someone forgot the changelog line | Release day is too late to remember | A pull request check asks for it |
| A typo in a workflow is found on release day | Workflows are only run when used | `actionlint` checks every workflow in every pull request |

## Not done yet

- **Operating-system code signing.** A Windows certificate and an Apple Developer ID with notarisation, so a hand-downloaded copy does not meet SmartScreen or Gatekeeper. They would go in as repository secrets used by the build job.
- **A macOS app bundle.** On macOS BuildRail is a single program in `~/.local/bin`, not a `.app` in Applications, so it has no Dock or Launchpad icon.
- **A staged rollout.** Every running copy is offered a release as soon as it is published. There is no way to offer it to a few first.
- **More platforms.** No builds for Windows on ARM (the x64 build runs under emulation) or Linux on ARM.
- **Build provenance.** GitHub's artifact attestations would let anyone check a download was built by this workflow from this source.
- **An uninstall script.** Removing BuildRail is deleting the program and, on Windows, the Start menu entry.
