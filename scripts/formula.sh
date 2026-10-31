#!/usr/bin/env bash
# Writes the Homebrew formula for a release to standard output. The formula
# builds reeltty from the source archive of the release, so it always links
# the libmpv of Homebrew.
#
# Usage: scripts/formula.sh VERSION SOURCE
#
# VERSION is the version without "v", for example 0.1.0. SOURCE is the
# source archive of the release, reeltty_VERSION_source.tar.gz.
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 VERSION SOURCE" >&2
  exit 2
fi
version="$1"
source="$2"
sum="$(sha256sum "$source" | cut -d' ' -f1)"
if [[ ! "$sum" =~ ^[0-9a-f]{64}$ ]]; then
  echo "no checksum for $source" >&2
  exit 1
fi
base="https://github.com/juthrbog/reeltty/releases/download/v${version}"

cat <<FORMULA
# Generated from the release's source archive. Do not edit manually.
class Reeltty < Formula
  desc "Keyboard-driven YouTube player that runs in the terminal"
  homepage "https://github.com/juthrbog/reeltty"
  url "${base}/reeltty_${version}_source.tar.gz"
  sha256 "${sum}"
  license any_of: ["MIT", "Apache-2.0"]

  # Video frames use Linux shared memory (/dev/shm).
  depends_on :linux
  depends_on "rust" => :build
  depends_on "mpv"
  depends_on "yt-dlp"
  # yt-dlp needs a JavaScript runtime for full YouTube support.
  depends_on "deno"

  def install
    system "cargo", "install", *std_cargo_args
  end

  def caveats
    <<~EOS
      Video plays in Ghostty 1.3.1+ and kitty 0.49+, run directly (not in
      tmux or over SSH). Other terminals can browse, but not play.
      Run "reeltty doctor" to check your setup.
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/reeltty --version")
  end
end
FORMULA
