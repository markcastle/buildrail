# Changelog

Every change to BuildRail that a person would notice, by version, newest first. Each release says what was **added**, **changed**, **fixed** or **removed**, written for the people using the strip rather than for the people building it. The Release workflow copies a version's section from here into its GitHub release, word for word.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html), with one plain rule: anything **Added** or **Removed** makes a minor release, and changes and fixes make a patch. Add your line under Unreleased in the same pull request as the change. How the rest happens is in [docs/releasing.md](docs/releasing.md).

## [Unreleased]

### Added

- **BuildRail updates itself.** It checks for a newer release while it runs, makes sure the download really came from BuildRail's own release process, and replaces itself with it. Nothing to download by hand, and a tampered or mislabelled download is refused rather than installed.
- **Install or update in one line.** Paste one command into PowerShell on Windows, or a terminal on macOS and Linux, and BuildRail is installed for you alone, with no administrator prompt, and started. Run the same line again to update. It checks the download against the release's published checksums before using it.
- **Downloads for macOS and Linux.** Each release now has BuildRail for Windows, for Macs with Apple silicon and with Intel, and for 64-bit Linux.

### Changed

- **The strip uses far less power when nothing is happening.** It no longer redraws itself many times a second while your builds sit still, so a laptop on battery barely notices it.
- **The download is smaller.**

### Fixed

- **`--help` shows in a terminal on Windows.** Before, running `buildrail --help` from PowerShell or Command Prompt printed nothing.
- **Starting BuildRail a second time no longer opens a second strip.** The copy that is already running stays the only one.

## [0.2.0] - 2026-11-17

### Changed

- **A new look, shared with Task Rail.** The strip, Settings and the steps for adding a build use Task Rail's colours, spacing and shapes, in light and dark.
- **Text uses your system's own font:** Segoe UI on Windows and San Francisco on macOS.
- **Settings is easier to find your way around.** It has side navigation with Builds, Connections, Strip, Share and About, and your choices apply as you make them. There is no Save button to forget.
- **Clearer wording throughout,** with buttons that say what they do.
- **The status icons are redrawn,** and the details and overflow pop-ups are raised cards that stand out from what is behind them.

## [0.1.0] - 2026-11-16

### Added

- **A 24 pixel strip that shows the builds you care about.** Pin GitHub Actions workflows and Cloudflare Workers Builds, and each keeps its place with its status, details when you hover or tab to it, and a click through to the run.
- **Add a build in a few steps.** Connect with a read-only token, pick what to watch, and check its latest run before you save it.
- **Settings to rename, reorder, remove and reconnect builds.**
- **Share your setup.** Export your pinned builds to a file a teammate can import, with a preview of what will change first. Tokens are never in the file.
- **The strip docks above the Windows taskbar and on X11,** so maximised windows stop short of it. On Wayland and macOS it is a small window you can move.
- **A demo that needs no accounts.** It shows the real strip with made-up builds.
- **A Windows download,** a zip with a double-click "Try the demo" launcher.

[Unreleased]: https://github.com/markcastle/buildrail/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/markcastle/buildrail/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/markcastle/buildrail/releases/tag/v0.1.0
