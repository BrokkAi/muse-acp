#!/bin/sh
# Copyright 2026 Brokk.ai.
# SPDX-License-Identifier: Apache-2.0

set -eu

repo="BrokkAi/muse-acp"

say() {
  printf '%s\n' "$*"
}

fail() {
  printf 'muse-acp installer: %s\n' "$*" >&2
  exit 1
}

need() {
  command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"
}

secure_curl() {
  curl --proto '=https' --tlsv1.2 --retry 3 --retry-delay 1 "$@"
}

need curl
need install
need tar
need uname
need awk
need tr

os="$(uname -s)"
arch="$(uname -m)"

# Prefer the native Apple Silicon build when invoked by an x86_64 shell under
# Rosetta. Failure to query sysctl is harmless on Intel Macs.
if [ "$os" = "Darwin" ] && [ "$arch" = "x86_64" ] && command -v sysctl >/dev/null 2>&1; then
  if [ "$(sysctl -in sysctl.proc_translated 2>/dev/null || true)" = "1" ]; then
    arch="arm64"
  fi
fi

case "$os" in
  Linux) os_target="unknown-linux-gnu" ;;
  Darwin) os_target="apple-darwin" ;;
  *) fail "unsupported operating system: $os (use a release archive instead)" ;;
esac

case "$arch" in
  x86_64 | amd64) arch_target="x86_64" ;;
  arm64 | aarch64) arch_target="aarch64" ;;
  *) fail "unsupported architecture: $arch (use a release archive instead)" ;;
esac

target="${arch_target}-${os_target}"

if [ -n "${MUSE_ACP_VERSION:-}" ]; then
  tag="$MUSE_ACP_VERSION"
  case "$tag" in
    v*) ;;
    *) tag="v${tag}" ;;
  esac
else
  latest_url="$(secure_curl -fsSL -o /dev/null -w '%{url_effective}' \
    "https://github.com/${repo}/releases/latest")" \
    || fail "could not determine the latest release"
  latest_url="${latest_url%/}"
  tag="${latest_url##*/}"
fi

case "$tag" in
  v[0-9]*) ;;
  *) fail "invalid release version: $tag" ;;
esac
case "$tag" in
  *[!A-Za-z0-9._+-]*) fail "invalid release version: $tag" ;;
esac

if [ -n "${MUSE_ACP_INSTALL_DIR:-}" ]; then
  install_dir="$MUSE_ACP_INSTALL_DIR"
elif [ -n "${HOME:-}" ]; then
  install_dir="${HOME}/.local/bin"
else
  fail 'HOME is not set; set MUSE_ACP_INSTALL_DIR to an absolute directory'
fi

case "$install_dir" in
  /*) ;;
  *) fail "MUSE_ACP_INSTALL_DIR must be an absolute path: $install_dir" ;;
esac

package="muse-acp-${tag}-${target}"
archive="${package}.tar.gz"
release_base="https://github.com/${repo}/releases/download/${tag}"

tmp_dir="$(mktemp -d 2>/dev/null || mktemp -d -t muse-acp)" \
  || fail "could not create a temporary directory"
cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup 0
trap 'exit 1' HUP INT TERM

archive_path="${tmp_dir}/${archive}"
checksum_path="${archive_path}.sha256"

say "Downloading muse-acp ${tag} for ${target}..."
secure_curl -fsSL -o "$archive_path" "${release_base}/${archive}" \
  || fail "could not download ${archive}"
secure_curl -fsSL -o "$checksum_path" "${release_base}/${archive}.sha256" \
  || fail "could not download ${archive}.sha256"

expected_hash="$(awk 'NR == 1 { print $1 }' "$checksum_path" | tr '[:upper:]' '[:lower:]')"
case "$expected_hash" in
  *[!0-9a-f]* | '') fail "release checksum is malformed" ;;
esac
[ "${#expected_hash}" -eq 64 ] || fail "release checksum is malformed"

if command -v sha256sum >/dev/null 2>&1; then
  actual_hash="$(sha256sum "$archive_path" | awk '{ print $1 }')"
elif command -v shasum >/dev/null 2>&1; then
  actual_hash="$(shasum -a 256 "$archive_path" | awk '{ print $1 }')"
elif command -v openssl >/dev/null 2>&1; then
  actual_hash="$(openssl dgst -sha256 "$archive_path" | awk '{ print $NF }')"
else
  fail "no SHA-256 tool found (install sha256sum, shasum, or openssl)"
fi
actual_hash="$(printf '%s' "$actual_hash" | tr '[:upper:]' '[:lower:]')"

[ "$actual_hash" = "$expected_hash" ] \
  || fail "checksum mismatch for ${archive}; refusing to install"

tar -xzf "$archive_path" -C "$tmp_dir" "${package}/muse-acp" \
  || fail "could not extract ${archive}"
binary_path="${tmp_dir}/${package}/muse-acp"
[ -f "$binary_path" ] || fail "release archive does not contain muse-acp"
chmod 0755 "$binary_path"
"$binary_path" --version >/dev/null 2>&1 \
  || fail "the downloaded binary cannot run on this system"

mkdir -p "$install_dir" \
  || fail "cannot create ${install_dir}; set MUSE_ACP_INSTALL_DIR to a writable directory"
install -m 0755 "$binary_path" "${install_dir}/muse-acp" \
  || fail "cannot write to ${install_dir}; set MUSE_ACP_INSTALL_DIR to a writable directory"

say "Installed muse-acp ${tag} to ${install_dir}/muse-acp"
case ":${PATH:-}:" in
  *":${install_dir}:"*) ;;
  *)
    say "Add ${install_dir} to PATH, then restart your shell."
    say "For sh-compatible shells: export PATH=\"${install_dir}:\$PATH\""
    ;;
esac
say "Register it with an editor:"
say "  Zed:       muse-acp install"
say "  JetBrains: muse-acp install-intellij"
