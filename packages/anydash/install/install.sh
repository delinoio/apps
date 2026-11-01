#!/bin/sh
# SPDX-License-Identifier: Apache-2.0
#
# Installs the AnyDash CLI on macOS and Linux:
#
#   curl -fsSL https://anydash.delino.io/install.sh | sh
#   curl -fsSL https://anydash.delino.io/install.sh | sh -s -- --version 1.2.3
#
# The script downloads the archive for this OS and CPU and SHA256SUMS from the
# AnyDash GitHub Release, verifies the checksum before extracting, and installs
# `anydash` into ~/.local/bin without elevated privileges. Rerun it to update.
#
# Options and environment:
#   --version X.Y.Z, ANYDASH_VERSION        install this release instead of the latest
#   --install-dir DIR, ANYDASH_INSTALL_DIR  install into DIR (default ~/.local/bin)
#   ANYDASH_RELEASES_URL, ANYDASH_DOWNLOAD_URL
#                                           release list and download base, for mirrors
#                                           and tests (default: GitHub Releases)
set -eu

releases_url=${ANYDASH_RELEASES_URL:-https://api.github.com/repos/delinoio/apps/releases?per_page=100}
download_url=${ANYDASH_DOWNLOAD_URL:-https://github.com/delinoio/apps/releases/download}
version=${ANYDASH_VERSION:-}
install_dir=${ANYDASH_INSTALL_DIR:-}
work=""

log() {
  printf 'anydash install: %s\n' "$1" >&2
}

fail() {
  log "error: $1"
  log "nothing was installed"
  exit 1
}

usage() {
  cat <<'USAGE'
Usage: install.sh [--version X.Y.Z] [--install-dir DIR]

Installs the AnyDash CLI from its GitHub Release after verifying the archive
checksum. The default location is ~/.local/bin. ANYDASH_VERSION and
ANYDASH_INSTALL_DIR set the same options.
USAGE
}

cleanup() {
  if [ -n "$work" ] && [ -d "$work" ]; then rm -rf "$work"; fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version)
      [ "$#" -ge 2 ] || fail "--version needs a value"
      version=$2
      shift 2
      ;;
    --version=*)
      version=${1#--version=}
      shift
      ;;
    --install-dir)
      [ "$#" -ge 2 ] || fail "--install-dir needs a value"
      install_dir=$2
      shift 2
      ;;
    --install-dir=*)
      install_dir=${1#--install-dir=}
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      fail "unknown option $1"
      ;;
  esac
done

detect_platform() {
  os=$(uname -s)
  arch=$(uname -m)
  case "$os" in
    Darwin) os=darwin ;;
    Linux) os=linux ;;
    *) fail "unsupported platform $os/$arch; AnyDash supports macOS, Linux and Windows (use install.ps1)" ;;
  esac
  case "$arch" in
    x86_64 | amd64) arch=x64 ;;
    arm64 | aarch64) arch=arm64 ;;
    *) fail "unsupported platform $os/$arch; AnyDash supports x64 and arm64" ;;
  esac
  # A shell under Rosetta reports x86_64; install the native Apple silicon build.
  if [ "$os" = darwin ] && [ "$arch" = x64 ] &&
    [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || true)" = 1 ]; then
    arch=arm64
  fi
  # The Linux builds link against glibc.
  if [ "$os" = linux ] && ldd --version 2>&1 | grep -qi musl; then
    fail "unsupported platform linux/$arch with musl libc; AnyDash requires glibc"
  fi
  printf '%s-%s' "$os" "$arch"
}

# Only HTTPS is accepted unless a mirror or test location is configured.
curl_proto='=https'
if [ -n "${ANYDASH_RELEASES_URL:-}" ] || [ -n "${ANYDASH_DOWNLOAD_URL:-}" ]; then
  curl_proto='=https,http'
fi

fetch() {
  if command -v curl >/dev/null 2>&1; then
    curl --proto "$curl_proto" --proto-redir "$curl_proto" -fsSL --retry 3 -o "$2" "$1"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -O "$2" "$1"
  else
    fail "curl or wget is required"
  fi
}

digest() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  elif command -v openssl >/dev/null 2>&1; then
    openssl dgst -sha256 -r "$1" | awk '{print $1}'
  else
    fail "sha256sum, shasum or openssl is required to verify the download"
  fi
}

# Prints the checksum listed for $2 in the checksum file $1.
listed() {
  awk -v name="$2" '$2 == name || $2 == "*" name { print $1 }' "$1"
}

platform=$(detect_platform)
if [ -z "$install_dir" ]; then
  [ -n "${HOME:-}" ] || fail "HOME is not set; pass --install-dir"
  install_dir="$HOME/.local/bin"
fi
work=$(mktemp -d 2>/dev/null || mktemp -d -t anydash-install)

if [ -z "$version" ]; then
  fetch "$releases_url" "$work/releases.json" || fail "cannot read the AnyDash release list"
  version=$(grep -o '"tag_name"[[:space:]]*:[[:space:]]*"anydash@v[0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*"' "$work/releases.json" |
    sed 's/.*anydash@v//; s/"$//' |
    sort -t. -k1,1n -k2,2n -k3,3n | tail -n 1)
  [ -n "$version" ] || fail "no published AnyDash release was found"
fi
case "$version" in
  v*) version=${version#v} ;;
esac
printf '%s\n' "$version" | grep -Eq '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' ||
  fail "invalid version $version; expected MAJOR.MINOR.PATCH"

archive="anydash-$platform-$version.tar.gz"
base="$download_url/anydash@v$version"
log "installing AnyDash $version for $platform"
fetch "$base/SHA256SUMS" "$work/SHA256SUMS" || fail "cannot download SHA256SUMS for AnyDash $version"
fetch "$base/$archive" "$work/$archive" || fail "cannot download $archive"

expected=$(listed "$work/SHA256SUMS" "$archive")
[ "$(printf '%s\n' "$expected" | grep -c .)" -eq 1 ] || fail "SHA256SUMS does not list $archive exactly once"
printf '%s\n' "$expected" | grep -Eq '^[a-f0-9]{64}$' || fail "SHA256SUMS has an invalid entry for $archive"
[ "$(digest "$work/$archive")" = "$expected" ] || fail "checksum mismatch for $archive"

mkdir "$work/extract"
tar -xzf "$work/$archive" -C "$work/extract" || fail "cannot extract $archive"
package="$work/extract/anydash-$platform"
[ -f "$package/anydash" ] && [ ! -L "$package/anydash" ] || fail "$archive does not contain the anydash executable"
inner=$(listed "$package/SHA256SUMS" anydash)
[ -n "$inner" ] && [ "$(digest "$package/anydash")" = "$inner" ] || fail "checksum mismatch for the extracted executable"

mkdir -p "$install_dir" || fail "cannot create $install_dir"
staged="$install_dir/.anydash.install.$$"
cp "$package/anydash" "$staged" || fail "cannot write to $install_dir"
chmod 755 "$staged"
mv -f "$staged" "$install_dir/anydash" || {
  rm -f "$staged"
  fail "cannot replace $install_dir/anydash"
}

log "installed AnyDash $version to $install_dir/anydash"
case ":${PATH:-}:" in
  *":$install_dir:"*) ;;
  *) log "add $install_dir to PATH to run anydash" ;;
esac
log "next: run 'anydash login' to connect this device"
