# BuildRail

BuildRail is a thin strip along the bottom (or top) of your screen that tells you, at a glance, whether the builds you care about are passing. Pin a GitHub Actions workflow or a Cloudflare Worker, and its entry stays in the same place all day, so you learn where to look.

![The BuildRail strip in demo mode](docs/images/strip-dark.png)

It talks to GitHub and Cloudflare directly with a read-only token that you supply. There is no BuildRail account and no BuildRail server.

## Installing

Every release has BuildRail for Windows. Builds for Macs (Apple silicon and Intel) and 64-bit Linux are published only with major releases, so they can be a few versions behind Windows. One line installs it for you alone, with no administrator prompt, checks the download against the release's checksums, and starts it. Run the same line again to update.

Windows, in PowerShell:

```powershell
irm https://raw.githubusercontent.com/markcastle/buildrail/main/scripts/install.ps1 | iex
```

macOS and Linux:

```bash
curl -fsSL https://raw.githubusercontent.com/markcastle/buildrail/main/scripts/install.sh | sh
```

On macOS and Linux it installs the newest release that has a build for your system, and says which one if that is not the latest.

You can also download a release from the [releases page](https://github.com/markcastle/buildrail/releases) and run it from wherever you unpack it.

### Staying up to date

BuildRail looks for a new release about once an hour while it runs. When one is ready, a small **Update** mark appears at the right end of the strip. Hover over it for **Install now**, **What's new** (the release page) and **Not now**. Clicking it, or pressing Enter on it, opens Settings › About, so a stray click never installs anything; nothing happens until you choose. Settings › About has **Check for updates** and an **Update automatically** switch, which is off unless you turn it on. With it on, a new release installs when none of your builds is running and Settings is closed, and BuildRail starts again by itself.

Before installing, BuildRail checks that the download was signed by BuildRail's own release process and is the build for your system and that version. Anything else is refused and nothing changes. A copy you build yourself has no signing key in it, so it says "This build does not update itself" and never downloads anything.

## What the strip shows

Each entry is the provider, your short label, a status icon and a status word.

| Colour | Meaning |
|---|---|
| Green | Passed, or deployment confirmed |
| Amber | Queued or building |
| Red | Failed |
| Neutral | Cancelled, skipped or unavailable (each has its own word and icon) |

Hover over an entry, or move to it with Tab or the arrow keys, to see the branch, commit, age and failing job. Click it, or press Enter, to open the run on the provider's site. When the strip is too narrow, the entries that do not fit collapse into an overflow marker that shows how many of them are failing.

If the network drops, the strip keeps showing the last known result with its age, and marks it as offline. It refreshes immediately when the machine wakes or you reconnect.

## Try it without any accounts

Demo mode runs the real interface against made-up builds. It needs no network, no tokens and no keyring.

```bash
cargo run -p buildrail-app -- --demo
```

## Running it for real

```bash
cargo run --release -p buildrail-app
```

On first launch you see **Add build**. The wizard asks you to choose a provider, paste a token, pick the target, check the latest matching run, and give it a short label.

### Tokens

Create a token with read access only. BuildRail stores it in the operating system's credential store (Windows Credential Manager, macOS Keychain, or the Secret Service on Linux) and never writes it to a file, a log or an export.

| Provider | Token type | Permissions |
|---|---|---|
| GitHub | Fine-grained personal access token | Actions: Read, Metadata: Read |
| Cloudflare | User API token (account-owned tokens are rejected by the Builds API) | Workers CI: Read, Workers Scripts: Read |

### Sharing a setup with a teammate

Settings can export your pinned builds as a TOML file. The file holds targets, labels and order, and it has no field that could hold a token.

Your teammate imports it, sees a preview of what will change, and maps each connection name in the file to one of their own connections (or creates one on the spot). Nothing is written until the whole import validates, and the save is atomic, so a broken file cannot damage a working configuration.

```toml
version = 1

[[watch]]
id = "4f6c1a52-93d3-4bd5-8f0e-6f7c1b2a9e10"
label = "API CI"
provider = "github"
connection = "work-github"
owner = "acme"
repo = "api"
workflow = "ci.yml"
branch = "main"            # optional

[[watch]]
id = "marketing-site"
label = "Marketing site"
provider = "cloudflare"
connection = "acme-cf"
account_id = "0123456789abcdef0123456789abcdef"
worker = "marketing-site"
```

Monitor, scale, theme, placement, animation, start-at-login and automatic updates are kept in a separate per-device file and are never exported.

## How it behaves on each system

| System | Placement | State of testing |
|---|---|---|
| Windows | Registers as an AppBar at the bottom or top, so it sits against the taskbar or screen edge and maximised windows stay clear of it | Docking tested against the real Explorer on CI's Windows runner (one display at 100 %), plus a demo smoke run with a screenshot. Two displays at different scales only by hand. |
| Linux, X11 | Sets dock type and struts for the bottom or top edge, so the window manager reserves it | Docking tested against a real X server (Xvfb), not against a real window manager |
| Linux, Wayland | Compact movable window. A winit window cannot become a layer-shell surface yet. | Not run |
| macOS | Compact movable window. macOS has no public way to reserve a screen edge. | Type-checked only. Never run. |

Windows builds come with every release. macOS and Linux builds are published only with major releases and may lag behind; a copy on those systems treats a release without its build as nothing to install and says "Up to date."

The strip hides while a fullscreen app is on its monitor (Windows and X11), never takes keyboard focus on its own, and follows the system light or dark theme unless you override it. Only one strip runs at a time: starting BuildRail again leaves the running copy as it is. It draws with OpenGL on every system, and an idle strip draws nothing at all until something on it changes.

Updating replaces the program file in place and starts the new copy, after giving back the screen edge and the one-strip lock. It is tested with fakes and against a local web server, but has not yet replaced a real installed copy on any system: that is step 10 of the checklist below.

> **Before you rely on it:** the real window has so far only been run by hand on Windows. Everything above the window layer is tested, and the strip is rendered and checked headlessly, but the first job on each system is to run `--demo` and then the acceptance scenario below.

## Finding your way around the code

The app is five crates, plus three for updates and releases. The rule is that knowledge flows one way: everything depends on `buildrail-core`, and `buildrail-core` depends on nothing else in the workspace.

| Crate | What lives there | Start reading at |
|---|---|---|
| `buildrail-core` | The vocabulary (watch, build record, status), the traits the outside world implements, which run wins, and when to poll | `src/domain.rs`, then `src/ports.rs` |
| `buildrail-profile` | The TOML profile, the one validation path shared by the picker and the importer, import preview, atomic saves, per-device preferences | `src/lib.rs` (written as a guide) |
| `buildrail-providers` | GitHub and Cloudflare adapters, and the lists the picker needs | `src/github/runs.rs`, `src/cloudflare/status.rs` |
| `buildrail-platform` | Credential store, edge docking per OS, fullscreen detection, opening URLs, start at login | `src/dock/mod.rs` |
| `buildrail-app` | The egui interface and the single place where real adapters are chosen | `src/compose.rs`, then `src/presentation/` |
| `buildrail-update` | Self-update: the signed `latest.json` format, verification, and (feature `client`) checking, installing and restarting | `src/lib.rs` (the trust model in plain words) |
| `buildrail-sign` | The release-time tool that signs update payloads and writes `latest.json` | `src/cli.rs` |
| `xtask` | Release tooling: the changelog check, the next version, cutting a release | `src/release.rs` |

A few things are worth knowing before you change anything.

- **The interface is deliberately thin.** What an entry says, the wizard steps, import, and docking decisions are plain state machines with no egui in them, which is why they can be tested without a window. If you find yourself putting a decision inside a widget, it probably belongs in `presentation`, `settings` or `shell`.
- **Adding a provider** means implementing `RunSource` and registering it. The refresh engine does not change.
- **An older run can never replace a newer one.** That rule lives in `buildrail-core/src/selection.rs` and nowhere else.
- **Polling** is 60 seconds when idle and 15 seconds while a build is active. Watches that point at the same target share one request. Failures back off from 30 seconds up to 10 minutes, and a rate limit pauses every request on that connection.

### Cloudflare details to confirm

The Cloudflare adapter was written from the API specification without access to a live account. Each unconfirmed detail sits in one small tested function, so correcting it is a one-line change.

- When a successful build counts as "Deployed" rather than "Passed" (`cloudflare/status.rs`, `is_production_deploy`).
- The dashboard link format for a build (`dashboard_build_url`).
- Whether a Worker that is not connected to Builds answers with "not found" or an empty list.
- Which Cloudflare error codes mean "access denied" (`CLOUDFLARE_AUTH_ERROR_CODES` in `http.rs`).
- That a `terminated` build should show as Failed.

## Building and testing

You need Rust 1.95 or newer. No system packages are needed to build. The window needs an OpenGL driver; on Linux it also needs `libxkbcommon-x11` at run time, and token storage needs a running Secret Service such as GNOME Keyring or KWallet.

```bash
cargo test --workspace
```

On Linux the docking tests want a display. Without one they skip; to run them headlessly:

```bash
xvfb-run -a cargo test --workspace
```

Coverage, which CI holds at 90% of lines or better:

```bash
xvfb-run -a cargo llvm-cov --workspace --summary-only
```

Lint, with warnings treated as errors:

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

Set `BUILDRAIL_CONFIG_DIR` to point the app at a scratch folder, so you can experiment without touching your real configuration.

## Releasing

Releases are cut by a button, and the changelog decides the version.

- **While working,** add a line under `## [Unreleased]` in `CHANGELOG.md` in the same pull request, written for the people using BuildRail. Anything **Added** or **Removed** makes the next release a minor one; changes and fixes make a patch.
- **To release,** open Actions, pick **Release**, press **Run workflow** and leave the version blank. It checks CI passed, sets the version everywhere, tags, builds for Windows, and publishes once every download is attached. For a major release, also tick **Also build macOS and Linux**: it then tests on macOS first and builds for macOS (Apple silicon and Intel) and Linux too. Tick **Rehearse** to see what it would do without doing it.
- **To undo a bad release,** run **Roll back** with the last good version and a sentence on why.
- **To try a pull request,** download the Windows test build from its checks.

Do not make tags or set the version by hand: a pushed tag builds nothing. Everything else, including the one-time update signing setup, is in [docs/releasing.md](docs/releasing.md).

## Acceptance checklist for each operating system

These are the checks that cannot be automated here and need a person at a real desktop.

1. Run `--demo`. Confirm the strip is 24 pixels tall at your display scale, sits where the table above says, and does not take focus from the app you were typing in.
2. Maximise a window and confirm it stops above the strip (Windows and X11). Switch Placement to Top and confirm the strip moves to the top edge and maximised windows start below it.
3. With two displays at different scales, choose the other display: the strip sits flush against that display's taskbar (or top edge) with no gap, and its details open on that display. Quit and start again, and confirm it is still flush and the first display's work area is whole.
4. Open a fullscreen app and confirm the strip gets out of the way.
5. Pin a GitHub workflow and a Worker, trigger builds, and watch queued, building and the final state appear. Open each run from the strip.
6. Re-run a build and confirm the entries do not change order.
7. Disconnect the network and confirm the entries show their age and an offline note. Sleep and wake the machine and confirm they recover.
8. Export the profile, import it on a second operating system, and confirm the same targets in the same order after you supply local tokens.
9. Import a deliberately broken file and confirm your configuration is untouched.
10. Install the previous release with the one-line installer, publish a new one, and confirm the Update mark appears within the hour (or at once with Check for updates), that Install now restarts into the new version with the strip docked where it was, and that a second start shows no leftover files beside the program.

## Not in this release

- A hosted team service. Sharing a profile file covers teams whose members already have provider access.
- Sign-in through the browser. Connecting is by pasted token; the code has a seam for a device flow.
- Reserving a screen edge on Wayland and macOS.
- Operating-system code signing. Windows SmartScreen and macOS Gatekeeper warn the first time you run a copy you downloaded by hand; updates through the app are not affected.
- A macOS app bundle. On macOS BuildRail is a single program, with no Dock or Launchpad icon.
