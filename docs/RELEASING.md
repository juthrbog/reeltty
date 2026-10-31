# Releasing reeltty

A tag that starts with `v` publishes a release. `.github/workflows/release.yml` checks the code, builds the Linux archives, signs the checksums, and updates the Homebrew formula.

## Make a release

1. Change `version` in `Cargo.toml` to the new version, in a pull request, and merge it. The tag and `Cargo.toml` must name the same version, or the release stops.
2. Make sure that CI passes on `main`.
3. Switch to `main`, and pull.
4. Run the release task with the new version ([Task](https://taskfile.dev); the program is `go-task` on Arch Linux):

   ```sh
   task release -- v0.1.0
   ```

   The task stops if the version does not look like `v1.2.3` or `v1.2.3-rc.1`, if `Cargo.toml` has another version, if the branch is not `main`, if the tree has changes, if `main` is not the same as `origin/main`, or if the tag exists. Then it pushes the tag.
5. Watch the Release workflow on the Actions tab of the repository.

A version with a hyphen, such as `v0.2.0-rc.1`, makes a prerelease. A prerelease does not change the Homebrew formula.

## What a tag publishes

The `test` job runs first. It checks that the tag and `Cargo.toml` name the same version, then runs `cargo fmt --check`, `cargo clippy`, `cargo test` (with libmpv), and `cargo deny` (RustSec advisories, licenses, bans, and sources). It has no write permission. A failure stops the release.

Then these jobs run:

- `build` builds `reeltty` on Ubuntu 24.04 for Linux amd64 and arm64, each on a runner of its own architecture, and packs it with `README.md`, `LICENSE-MIT` and `LICENSE-APACHE`. The binary links the system `libmpv.so.2`, so it needs mpv 0.37 or later and glibc 2.39 or later. Release builds use no cache.
- `verify` unpacks each archive on its architecture, with only the run-time `libmpv2` package installed, and runs `reeltty --version`.
- `recipes` writes the source archive with `git archive`, the Homebrew formula with `scripts/formula.sh`, and the PKGBUILD and `.SRCINFO` with `scripts/pkgbuild.sh`. The formula and the PKGBUILD build reeltty from the source archive.

The `publish` job then:

- Writes `checksums.txt`, with the SHA-256 checksum of every other file.
- Signs `checksums.txt` with cosign, without a key, into `checksums.txt.sigstore.json`. The signature holds the identity of the release workflow.
- Adds a build provenance attestation for each file in `checksums.txt`.
- Creates the GitHub release. GitHub writes the release notes from the merged pull requests, and `.github/release.yml` puts the Dependabot updates in a section of their own.
- Pushes `Formula/reeltty.rb` to [juthrbog/homebrew-tap](https://github.com/juthrbog/homebrew-tap).

The archives use the commit time, a fixed owner, and a fixed file order, and the binary holds no paths of the build machine.

The release has these files:

| File | What it is |
|---|---|
| `reeltty_<version>_linux_amd64.tar.gz` | The binary for x86_64 Linux |
| `reeltty_<version>_linux_arm64.tar.gz` | The binary for aarch64 Linux |
| `reeltty_<version>_source.tar.gz` | The source of the tag |
| `reeltty.rb` | The Homebrew formula |
| `PKGBUILD`, `SRCINFO` | The AUR recipe. `SRCINFO` goes into the AUR as `.SRCINFO`. |
| `checksums.txt`, `checksums.txt.sigstore.json` | The checksums and their signature |

## Platforms

Only Linux. reeltty links the system libmpv, and the video frames use Linux shared memory (`/dev/shm`). macOS and Windows builds can come when they are tested.

## The Homebrew formula

The formula builds from the source archive with Homebrew's Rust, so it links Homebrew's libmpv. It depends on `mpv`, `yt-dlp`, and `deno`, and it is for Linux only (`depends_on :linux`). Homebrew's Rust must be at least the `rust-version` in `Cargo.toml`.

yt-dlp also needs the yt-dlp-ejs scripts for full YouTube support. Homebrew's yt-dlp includes them, and it depends on `deno` too. `reeltty doctor` checks both.

## The AUR recipe

The release has a PKGBUILD and its `.SRCINFO` (as `SRCINFO`). They build from the source archive and run `cargo test`. To publish to the AUR, review them, copy them into the AUR repository of reeltty (renaming `SRCINFO` to `.SRCINFO`), and push. The release workflow does not push to the AUR.

- With rustup, `rust-toolchain.toml` picks the Rust version that CI uses. With the `rust` package, it must be at least the `rust-version` in `Cargo.toml`.
- A prerelease such as `v0.2.0-rc.1` gets the pkgver `0.2.0rc1`. pkgver cannot hold a hyphen, and pacman sorts `0.2.0rc1` before `0.2.0`, so the final release is an upgrade.
- `options=('!lto')` is needed: rusqlite builds SQLite from C, and with LTO, makepkg's `CFLAGS` make GCC bitcode that the Rust linker cannot read.

## Test the release setup

A pull request that changes `.github/release.yml`, `.github/workflows/release.yml`, `scripts/formula.sh` or `scripts/pkgbuild.sh` runs every job except `publish`. You can also start them by hand on the Actions tab (`workflow_dispatch`). It publishes nothing. One more job runs then:

- `homebrew` installs the formula with Linuxbrew from the source archive of the run, in a local tap, and runs `brew test`.

On your computer:

- `task release:dry-run` builds the archive of your machine with `scripts/archive.sh`, as CI does, and writes the source archive, the formula and the PKGBUILD into `dist/`. It checks the formula with `ruby -c` and the PKGBUILD with `bash -n`. It does not sign.
- `task release:pkgbuild` (Arch Linux) builds the package of this commit with `makepkg` in `dist/pkg`, which runs the tests, and checks that `.SRCINFO` is what `makepkg --printsrcinfo` writes.

## The Homebrew deploy key

The release workflow pushes to juthrbog/homebrew-tap with a deploy key. The key has write access to that repository only. Its private key is the `HOMEBREW_TAP_SSH_KEY` Actions secret of this repository. No repository holds the private key.

If the secret is missing, the release still publishes the archives. It skips the formula and shows a warning.

To set up or replace the key:

1. Make a new key with no passphrase: `ssh-keygen -t ed25519 -N "" -C "reeltty release publisher" -f reeltty_tap`.
2. Add the public key to the tap with write access: `gh repo deploy-key add reeltty_tap.pub --repo juthrbog/homebrew-tap --title "reeltty release publisher" --allow-write`.
3. Set the secret: `gh secret set HOMEBREW_TAP_SSH_KEY --repo juthrbog/reeltty < reeltty_tap`.
4. Delete an old deploy key in the tap under Settings, Deploy keys.
5. Delete the two key files from your computer.

The tap's `validate.yml` tests each published formula. Add an `install-reeltty` job there, like the others, after the first release.
