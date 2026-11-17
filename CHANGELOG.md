# Changelog

Every release of BuildRail, newest first. Versions follow [semantic versioning](https://semver.org/): while the version starts with 0, a change in the middle number can change how things look or behave.

When you prepare a release, add a section here and set the same version in the root `Cargo.toml`. The release workflow refuses a tag that does not match.

## 0.2.0

The interface is restyled on the Task Rail design system.

### Changed

- The strip, settings and the add-a-build steps use Task Rail's colours, spacing and shapes, in light and dark.
- Text is set in the operating system's own font (Segoe UI on Windows, San Francisco on macOS) in place of the bundled one.
- Settings has side navigation with Builds, Connections, Strip, Share and About. Preferences apply as you change them; there is no Save button.
- Wording follows the Task Rail voice: sentence case, British spelling, verb-first buttons.
- Status glyphs are redrawn, and the details and overflow pop-ups are raised cards.

### Added

- A screenshot test that renders every screen in both themes into `docs/images`.

## 0.1.0

The first release.

### Added

- A 24 pixel strip that shows pinned GitHub Actions workflows and Cloudflare Workers Builds, in pinned order, with status, details on hover or focus, and a click through to the run.
- Add a build by connecting with a read-only token, picking the target and checking its latest run.
- Settings to rename, reorder, remove and reconnect builds.
- Profile export and import as TOML, with a preview, connection mapping and an atomic save.
- Docking above the Windows taskbar and on X11, with a floating window on Wayland and macOS.
- A demo mode that needs no accounts.
- A Windows build, packaged as a zip with a double-click demo launcher.
