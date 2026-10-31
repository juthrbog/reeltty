#!/usr/bin/env bash
# Packs target/release/reeltty, the README and the licenses into a release
# archive. The same commit gives the same archive: the files get the commit
# time, a fixed owner, and a fixed order.
#
# Usage: scripts/archive.sh VERSION PLATFORM DIR
#
# VERSION is the version without "v", for example 0.1.0. PLATFORM is, for
# example, linux_amd64. The archive goes to DIR/reeltty_VERSION_PLATFORM.tar.gz.
set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "usage: $0 VERSION PLATFORM DIR" >&2
  exit 2
fi
version="$1"
platform="$2"
dir="$3"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT

cp target/release/reeltty README.md LICENSE-MIT LICENSE-APACHE "$stage/"
mkdir -p "$dir"
tar --sort=name --owner=0 --group=0 --numeric-owner \
  --mtime="@$(git log -1 --format=%ct)" \
  -C "$stage" -cf - reeltty README.md LICENSE-MIT LICENSE-APACHE \
  | gzip -n > "$dir/reeltty_${version}_${platform}.tar.gz"
echo "$dir/reeltty_${version}_${platform}.tar.gz"
