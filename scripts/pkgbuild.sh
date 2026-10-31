#!/usr/bin/env bash
# Writes the PKGBUILD and the .SRCINFO for a release into a folder. They
# build reeltty from the source archive of the release, for the AUR.
#
# Usage: scripts/pkgbuild.sh VERSION SOURCE DIR
#
# VERSION is the version without "v", for example 0.1.0. SOURCE is the
# source archive of the release, reeltty_VERSION_source.tar.gz. A version
# with a hyphen, such as 0.2.0-rc.1, gets a pkgver with an underscore,
# because pkgver cannot hold a hyphen.
set -euo pipefail

if [[ $# -ne 3 ]]; then
  echo "usage: $0 VERSION SOURCE DIR" >&2
  exit 2
fi
version="$1"
source="$2"
dir="$3"
sum="$(sha256sum "$source" | cut -d' ' -f1)"
if [[ ! "$sum" =~ ^[0-9a-f]{64}$ ]]; then
  echo "no checksum for $source" >&2
  exit 1
fi
pkgver="${version//-/_}"
url="https://github.com/juthrbog/reeltty"
desc="Keyboard-driven YouTube player that runs in the terminal"
tarball="reeltty-${version}.tar.gz::${url}/releases/download/v${version}/reeltty_${version}_source.tar.gz"
mkdir -p "$dir"

cat > "$dir/PKGBUILD" <<PKGBUILD
# Generated for a tagged release. Review before submitting to the AUR.
pkgname=reeltty
pkgver=${pkgver}
pkgrel=1
pkgdesc='${desc}'
arch=('x86_64' 'aarch64')
url='${url}'
license=('MIT OR Apache-2.0')
depends=('mpv' 'yt-dlp' 'yt-dlp-ejs' 'deno' 'gcc-libs' 'glibc')
makedepends=('cargo')
# rusqlite builds SQLite from C. With LTO, makepkg's CFLAGS make GCC bitcode
# that the Rust linker cannot read.
options=('!lto')
_release_version='${version}'
source=("${tarball}")
sha256sums=('${sum}')

prepare() {
  cd "reeltty-\${_release_version}"
  # With rustup, rust-toolchain.toml picks the Rust version that CI uses.
  # Without it, the rust package must be at least the rust-version of
  # Cargo.toml.
  cargo fetch --locked --target "\$(rustc -vV | sed -n 's/host: //p')"
}

build() {
  cd "reeltty-\${_release_version}"
  export CARGO_TARGET_DIR=target
  cargo build --frozen --release
}

check() {
  cd "reeltty-\${_release_version}"
  cargo test --frozen
  ./target/release/reeltty --version
}

package() {
  cd "reeltty-\${_release_version}"
  install -Dm755 target/release/reeltty "\${pkgdir}/usr/bin/reeltty"
  install -Dm644 LICENSE-MIT "\${pkgdir}/usr/share/licenses/reeltty/LICENSE-MIT"
  install -Dm644 LICENSE-APACHE "\${pkgdir}/usr/share/licenses/reeltty/LICENSE-APACHE"
  install -Dm644 README.md "\${pkgdir}/usr/share/doc/reeltty/README.md"
}
PKGBUILD

# .SRCINFO is what the AUR reads. makepkg --printsrcinfo writes the same,
# but it needs Arch Linux.
cat > "$dir/.SRCINFO" <<SRCINFO
pkgbase = reeltty
	pkgdesc = ${desc}
	pkgver = ${pkgver}
	pkgrel = 1
	url = ${url}
	arch = x86_64
	arch = aarch64
	license = MIT OR Apache-2.0
	makedepends = cargo
	depends = mpv
	depends = yt-dlp
	depends = yt-dlp-ejs
	depends = deno
	depends = gcc-libs
	depends = glibc
	options = !lto
	source = ${tarball}
	sha256sums = ${sum}

pkgname = reeltty
SRCINFO
