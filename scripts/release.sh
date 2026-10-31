#!/usr/bin/env bash
# Tags a release and pushes the tag. The Release workflow does the rest.
#
# Usage: scripts/release.sh v0.1.0
#
# It stops if the version does not look like v1.2.3 or v1.2.3-rc.1, if
# Cargo.toml has another version, if the branch is not main, if the tree has
# changes, or if main is not the same as origin/main.
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 vVERSION" >&2
  exit 2
fi
tag="$1"
if [[ ! "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]]; then
  echo "The version must look like v1.2.3 or v1.2.3-rc.1, not $tag." >&2
  exit 1
fi
crate="$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version')"
if [[ "${tag#v}" != "$crate" ]]; then
  echo "Cargo.toml has version $crate. Change it to ${tag#v} first, in a pull request." >&2
  exit 1
fi
if [[ "$(git branch --show-current)" != main ]]; then
  echo "Switch to main first." >&2
  exit 1
fi
if [[ -n "$(git status --porcelain)" ]]; then
  echo "The tree has changes. Commit or remove them first." >&2
  exit 1
fi
git fetch --quiet origin main
if [[ "$(git rev-parse HEAD)" != "$(git rev-parse origin/main)" ]]; then
  echo "main is not the same as origin/main. Pull or push first." >&2
  exit 1
fi
if git rev-parse --quiet --verify "refs/tags/$tag" > /dev/null; then
  echo "The tag $tag exists already." >&2
  exit 1
fi

git tag -a "$tag" -m "$tag"
git push origin "$tag"
echo "Pushed $tag. Watch the Release workflow on the Actions tab."
