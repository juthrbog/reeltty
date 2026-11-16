//! On macOS, finds libmpv with pkg-config, so the linker also finds it in
//! Homebrew (/opt/homebrew/lib), which is not a default linker folder.
//!
//! Linux builds do not use pkg-config: the `#[link(name = "mpv")]` in
//! src/player/ffi.rs links the libmpv of the system, as before. If
//! pkg-config does not find libmpv on macOS, the build goes on the same way,
//! and the warning tells why the linker may then fail.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    // The version in mpv.pc is the client API version. 2.2 is the oldest
    // that reeltty supports (MIN_API in src/player/mod.rs).
    match pkg_config::Config::new().atleast_version("2.2").probe("mpv") {
        // Homebrew's folders have the mpv version in them. After an upgrade
        // removes them, this runs again and finds the new ones.
        Ok(lib) => {
            for path in &lib.link_paths {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
        Err(e) => {
            let reason = e.to_string().split_whitespace().collect::<Vec<_>>().join(" ");
            println!(
                "cargo:warning=pkg-config did not find libmpv, so the linker looks only in its default folders: {reason}"
            );
            // A path that never exists makes this run again at each build,
            // so it finds libmpv as soon as mpv or pkg-config is installed.
            let out = std::env::var("OUT_DIR").unwrap_or_default();
            println!("cargo:rerun-if-changed={out}/libmpv-not-found");
        }
    }
}
