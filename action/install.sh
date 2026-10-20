#!/usr/bin/env bash
#
# The install step of the action.
#
# The runner needs a shell, a downloader and a SHA-256 tool — nothing else.
# The asset is the **raw binary**, not an archive: there is no `tar`, no
# `unzip` and no platform-specific unpack step, so the same script works on
# ubuntu, on macOS and on windows.
#
# The download is **verified**: the SHA-256 of the asset is compared with the
# `checksums.txt` of the release.  A missing checksum or a mismatch fails the
# step — nothing is installed from an unverified download.
#
# The name of the asset is defined **once**, in the binary, and mirrored here.
# The test substitutes this template and compares the result with the
# function of the binary for every platform, so a renamed asset fails the
# test instead of the action.
#
# The verification protects against **corruption**, not against a hostile
# origin: the checksum is fetched from the same origin as the asset, so the
# origin is the trust anchor.  Only the calling workflow — trusted code — may
# move it (`RATCHET_ASSET_BASE`); a workflow that takes the base from an
# untrusted input has moved the trust anchor itself.
#
# The directory of the binary is written to `GITHUB_PATH` with forward
# slashes: the shell needs them.  On windows the native form (`D:\…`) is not
# used — the runner accepts the forward-slash form.

set -euo pipefail

# The base of the release assets.  `RATCHET_ASSET_BASE` points the step at
# another server (the tests use a local one); the default is the base of the
# releases of this repository.
ASSET_BASE="${RATCHET_ASSET_BASE:-https://github.com/all-the-rest/ratchet-cli/releases/download}"

# The template of the name of the asset — the same one the release workflow
# uses.  The binary computes the name; the test compares the two.
ASSET_TEMPLATE='ratchet-{version}-{os}-{arch}{exe}'

# The name of the checksums file of a release.
CHECKSUMS_NAME='checksums.txt'

# The name of the installed binary.
BINARY_NAME='ratchet'

# Fails the step with a message the runner shows as an error.
fail() {
  echo "::error::$1" >&2
  exit 1
}

# True when the text is a number: a version is three numbers.
is_number() {
  case "$1" in
    '' | *[!0-9]*) return 1 ;;
  esac
  return 0
}

# True when the text has the shape of a version: MAJOR.MINOR.PATCH.
is_version() {
  text=$1
  major=${text%%.*}
  rest=${text#*.}
  [ "$major" != "$text" ] || return 1
  minor=${rest%%.*}
  [ "$minor" != "$rest" ] || return 1
  patch=${rest#*.}
  [ "$patch" != "$rest" ] || return 1
  is_number "$major" && is_number "$minor" && is_number "$patch"
}

# The version of the binary the action installs.  The tag of a release may
# carry a `v`; the name of the asset does not.
version="${RATCHET_VERSION:-}"
[ -n "$version" ] || fail "the version of the binary is not set"
version="${version#v}"
# The shape is checked here and not at the download: a version that is not a
# version has to say so, and the download would only say that it failed.
is_version "$version" ||
  fail "the version '$version' is not a version — expected MAJOR.MINOR.PATCH"
tag="v$version"

# The platform of the runner, as the release names it.  A platform the
# release does not publish fails the step: an install of nothing is not a
# green.
case "${RUNNER_OS:-}" in
  Linux) os=linux ;;
  macOS) os=macos ;;
  Windows) os=windows ;;
  *) fail "unknown runner OS '${RUNNER_OS:-}' — the release publishes linux, macos and windows" ;;
esac
case "${RUNNER_ARCH:-}" in
  X64) arch=x86_64 ;;
  ARM64) arch=aarch64 ;;
  *) fail "unknown runner architecture '${RUNNER_ARCH:-}' — the release publishes x86_64 and aarch64" ;;
esac
case "$os" in
  windows) exe=.exe ;;
  *) exe= ;;
esac

# The name of the asset of this platform.
asset="$ASSET_TEMPLATE"
asset="${asset//\{version\}/$version}"
asset="${asset//\{os\}/$os}"
asset="${asset//\{arch\}/$arch}"
asset="${asset//\{exe\}/$exe}"

# The directory the binary is installed into.  It goes on the PATH of the
# following steps.
root="${RUNNER_TEMP:-${TMPDIR:-/tmp}}"
install_dir="${RATCHET_INSTALL_DIR:-$root/ratchet}"
# A windows path with backslashes is not a path for a shell.
install_dir=$(printf '%s' "$install_dir" | tr '\\' '/')

# The directory of the download; it is removed when the step ends.
work=$(mktemp -d) || fail "a temporary directory cannot be created"
trap 'rm -rf "$work"' EXIT

# Downloads a file.  `curl` on ubuntu and on windows, `wget` as the second
# choice; a runner with neither fails the step.
download() {
  url="$1"
  out="$2"
  if command -v curl >/dev/null 2>&1; then
    curl --fail --silent --show-error --location --output "$out" "$url"
  elif command -v wget >/dev/null 2>&1; then
    wget --quiet --output-document "$out" "$url"
  else
    fail "neither curl nor wget is available — the action cannot download the binary"
  fi
}

# The SHA-256 of a file, in lower case hex.
#
# `sha256sum` on ubuntu, `shasum -a 256` on macOS — the action may run on a
# self-hosted macOS runner, and the script uses the one it has.
sha256() {
  file="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$file" | cut -d' ' -f1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$file" | cut -d' ' -f1
  else
    fail "neither sha256sum nor shasum is available — the download cannot be verified"
  fi
}

# The binary of this platform.
if ! download "$ASSET_BASE/$tag/$asset" "$work/$asset"; then
  fail "the download of $ASSET_BASE/$tag/$asset failed"
fi

# The checksums of the release.
if ! download "$ASSET_BASE/$tag/$CHECKSUMS_NAME" "$work/$CHECKSUMS_NAME"; then
  fail "the download of $ASSET_BASE/$tag/$CHECKSUMS_NAME failed"
fi

# The checksum the release states for this asset.  The format is the one of
# `sha256sum`: the hash, a space and the file name; a name that starts with
# `*` is the binary-mode marker of the same format.
expected=$(awk -v name="$asset" '{ file = $2; sub(/^\*/, "", file); if (file == name) { print $1; exit } }' "$work/$CHECKSUMS_NAME") ||
  fail "the checksums of the release cannot be read"
[ -n "$expected" ] || fail "the checksums of the release do not name $asset — nothing is installed"

# The checksum of the download.
actual=$(sha256 "$work/$asset") || fail "the SHA-256 of the download cannot be computed"
expected=$(printf '%s' "$expected" | tr 'A-F' 'a-f')
actual=$(printf '%s' "$actual" | tr 'A-F' 'a-f')
if [ "$actual" != "$expected" ]; then
  fail "the checksum of $asset does not match: the release says $expected, the download is $actual — nothing is installed"
fi

# The binary goes into place.
mkdir -p "$install_dir" || fail "the directory $install_dir cannot be created"
target="$install_dir/$BINARY_NAME$exe"
cp "$work/$asset" "$target" || fail "the binary cannot be copied to $target"
if [ "$os" != "windows" ]; then
  chmod 0755 "$target" || fail "the binary $target cannot be made executable"
fi

# The directory goes on the PATH of the following steps.
github_path="${GITHUB_PATH:-}"
[ -n "$github_path" ] || fail "GITHUB_PATH is not set — the action must run on a GitHub runner"
github_path=$(printf '%s' "$github_path" | tr '\\' '/')
printf '%s\n' "$install_dir" >>"$github_path" || fail "the PATH file $github_path cannot be written"

echo "ratchet $version is installed in $install_dir"
