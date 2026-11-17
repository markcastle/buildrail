#!/bin/sh
# Installs or updates BuildRail on macOS or Linux from the latest GitHub
# release. The same line does both:
#
#   curl -fsSL https://raw.githubusercontent.com/markcastle/buildrail/main/scripts/install.sh | sh
#
# What it does, in order:
#
#   1. Asks GitHub for the latest release and picks this machine's download
#      by the end of its name (never the full name, which carries a version).
#   2. Downloads it and the release's SHA256SUMS.txt, and refuses to go on
#      unless the download's SHA-256 matches. A truncated or tampered
#      download is never installed.
#   3. Stops a running copy, puts the new one at ~/.local/bin/buildrail (just
#      for you, no sudo), clears the macOS download quarantine, and starts it.
#
# Settings:
#   BUILDRAIL_INSTALL_DIR   install somewhere else (default ~/.local/bin)
#   GITHUB_TOKEN            needed only while the repository is private

set -eu

REPO="markcastle/buildrail"
BIN_DIR="${BUILDRAIL_INSTALL_DIR:-$HOME/.local/bin}"
WINDOWS_LINE="irm https://raw.githubusercontent.com/$REPO/main/scripts/install.ps1 | iex"

say() { printf '%s\n' "$*"; }
fail() {
  printf 'BuildRail was not installed: %s\n' "$*" >&2
  exit 1
}

case "$(uname -s)" in
  Darwin) platform=macos ;;
  Linux) platform=linux ;;
  *) fail "this installer is for macOS and Linux. On Windows, run this in PowerShell: $WINDOWS_LINE" ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) cpu=x86_64 ;;
  arm64 | aarch64) cpu=aarch64 ;;
  *) fail "there is no BuildRail download for a $(uname -m) processor yet." ;;
esac
target="$platform-$cpu"
if [ "$target" = "linux-aarch64" ]; then
  fail "there is no BuildRail download for Linux on ARM yet."
fi

command -v curl >/dev/null 2>&1 || fail "curl is needed."
command -v tar >/dev/null 2>&1 || fail "tar is needed."

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d ' ' -f 1
  else
    shasum -a 256 "$1" | cut -d ' ' -f 1
  fi
}

# Downloads $1 to $2. With a token, through the API (the only way a private
# repository hands out release files); otherwise straight from github.com.
download() {
  if [ -n "${GITHUB_TOKEN:-}" ]; then
    curl -fsSL -H "Authorization: Bearer $GITHUB_TOKEN" -H "Accept: application/octet-stream" -o "$2" "$1"
  else
    curl -fsSL -o "$2" "$1"
  fi
}

say "Looking up the latest BuildRail release..."
api="https://api.github.com/repos/$REPO/releases/latest"
if [ -n "${GITHUB_TOKEN:-}" ]; then
  json=$(curl -fsSL -H "Authorization: Bearer $GITHUB_TOKEN" "$api") || fail "could not read the latest release of $REPO. Check GITHUB_TOKEN."
else
  json=$(curl -fsSL "$api") || fail "could not read the latest release of $REPO. If the repository is private, set GITHUB_TOKEN first."
fi
json=$(printf '%s' "$json" | tr -d '\n')

tag=$(printf '%s' "$json" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p')
[ -n "$tag" ] || fail "the latest release has no tag."

# One line per file on the release: "<name> <API address>". Each asset's API
# address comes just before its name in GitHub's answer.
assets=$(printf '%s' "$json" |
  grep -oE '"url": *"https://api\.github\.com/repos/[^"]+/releases/assets/[0-9]+"|"name": *"[^"]*"' |
  awk -F '"' '/^"url"/ { url = $4; next } url != "" { print $4 " " url; url = "" }')

pick() { printf '%s\n' "$assets" | grep -E "$1" | head -n 1; }
archive=$(pick "^[^ ]*-$target\\.tar\\.gz ")
sums=$(pick "^SHA256SUMS\\.txt ")
[ -n "$archive" ] || fail "release $tag has no download for $target."
[ -n "$sums" ] || fail "release $tag has no SHA256SUMS.txt, so its download cannot be checked."

archive_name=${archive%% *}
if [ -n "${GITHUB_TOKEN:-}" ]; then
  archive_url=${archive#* }
  sums_url=${sums#* }
else
  archive_url="https://github.com/$REPO/releases/download/$tag/$archive_name"
  sums_url="https://github.com/$REPO/releases/download/$tag/SHA256SUMS.txt"
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

say "Downloading BuildRail $tag for $target..."
download "$archive_url" "$tmp/$archive_name" || fail "the download failed."
download "$sums_url" "$tmp/SHA256SUMS.txt" || fail "SHA256SUMS.txt could not be downloaded."

expected=$(awk -v name="$archive_name" '{ file = $2; sub(/^\*/, "", file) } file == name { print tolower($1); exit }' "$tmp/SHA256SUMS.txt")
[ -n "$expected" ] || fail "SHA256SUMS.txt does not list $archive_name."
actual=$(sha256 "$tmp/$archive_name")
if [ "$actual" != "$expected" ]; then
  fail "the download does not match its checksum (expected $expected, got $actual). Nothing was changed; try again."
fi
say "Checksum matches."

tar -xzf "$tmp/$archive_name" -C "$tmp"
new="$tmp/${archive_name%.tar.gz}/buildrail"
[ -f "$new" ] || fail "the download did not contain buildrail."

# A running copy keeps its old file open; stop it so the new one starts clean.
if pkill -x buildrail >/dev/null 2>&1; then
  say "Stopping the running copy..."
  i=0
  while pgrep -x buildrail >/dev/null 2>&1 && [ "$i" -lt 10 ]; do
    sleep 0.5
    i=$((i + 1))
  done
fi

mkdir -p "$BIN_DIR"
dest="$BIN_DIR/buildrail"
# Copy beside it, then rename over it, so a half-written file is never run.
cp "$new" "$dest.new"
chmod 755 "$dest.new"
mv -f "$dest.new" "$dest"

if [ "$platform" = "macos" ]; then
  # Downloads can carry a quarantine flag that makes macOS refuse to run an
  # unsigned program. And Apple silicon runs nothing without at least an
  # ad-hoc signature, so give it one if it has none.
  xattr -d com.apple.quarantine "$dest" >/dev/null 2>&1 || true
  codesign -v "$dest" >/dev/null 2>&1 || codesign --force --sign - "$dest" >/dev/null 2>&1 || true
fi

say "BuildRail $tag is installed at $dest."
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) say "$BIN_DIR is not on your PATH. Add it to run buildrail by name, or run $dest." ;;
esac

if [ "$platform" = "linux" ] && [ -z "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]; then
  say "No desktop session here, so it was not started. Run buildrail from your desktop."
  exit 0
fi
nohup "$dest" >/dev/null 2>&1 &
say "It is running: look along the bottom of your screen. Run this line again at any time to update."
