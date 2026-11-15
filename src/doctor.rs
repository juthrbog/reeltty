//! `reeltty doctor`: checks the programs and libraries that reeltty needs.

use std::fmt;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{self, Config, Paths};
use crate::player;
use crate::source::download::{FileFormat, size_text};
use crate::store::{DownloadState, Store};
use crate::terminal::detect::{self, Graphics};
use crate::theme::ThemeName;

/// yt-dlp older than this gets a warning. YouTube breakage follows the
/// upstream releases closely.
const YT_DLP_MAX_AGE_DAYS: i64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone)]
pub struct Check {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
}

impl fmt::Display for Check {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mark = match self.status {
            Status::Ok => "ok  ",
            Status::Warn => "warn",
            Status::Fail => "FAIL",
        };
        write!(f, "[{mark}] {:<10} {}", self.name, self.detail)
    }
}

fn check(name: &'static str, status: Status, detail: impl Into<String>) -> Check {
    Check { name, status, detail: detail.into() }
}

/// Runs all checks. Prints each result as it is ready. Returns `true` if no check failed.
pub fn run(paths: &Paths) -> bool {
    let mut checks = Vec::new();
    let mut emit = |c: Check| {
        println!("{c}");
        checks.push(c);
    };

    let config = match Config::load(&paths.config_file) {
        Ok((c, source)) => {
            let detail = match source {
                config::Source::File => paths.config_file.display().to_string(),
                config::Source::Defaults => format!("{} (not found, using defaults)", paths.config_file.display()),
            };
            emit(check("config", Status::Ok, detail));
            // Keys that the file gave away from an action that then has none.
            if let Ok(keymap) = c.keymap() {
                for w in keymap.warnings() {
                    emit(check("keys", Status::Warn, w));
                }
            }
            c
        }
        Err(e) => {
            emit(check("config", Status::Fail, format!("{e:#}")));
            Config::default()
        }
    };

    let store = Store::open(&paths.database);
    emit(
        match store
            .as_ref()
            .map_err(|e| format!("{e:#}"))
            .and_then(|s| s.schema_version().map_err(|e| format!("{e:#}")))
        {
            Ok(v) => check("database", Status::Ok, format!("{} (schema {v})", paths.database.display())),
            Err(e) => check("database", Status::Fail, e),
        },
    );
    let saved = store.ok().and_then(|s| s.downloads().ok()).map(|all| {
        let done: Vec<_> = all.iter().filter(|d| d.state == DownloadState::Done).collect();
        (done.len(), done.iter().filter_map(|d| d.size_bytes).sum())
    });
    for c in check_downloads(&paths.downloads, saved, free_bytes(&paths.downloads)) {
        emit(c);
    }
    if config.downloads.format == FileFormat::Mkv {
        emit(check_ffmpeg());
    }

    emit(check_libmpv());
    for c in check_yt_dlp(&config.yt_dlp_program(), days_since_epoch_now()) {
        emit(c);
    }
    emit(check_terminal());
    emit(check_theme(config.theme, std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())));

    !checks.iter().any(|c| c.status == Status::Fail)
}

/// The downloads folder: where it is, whether reeltty can write to it, how
/// much it holds, and the free space on its disk. `saved` is the number of
/// downloaded videos and their size, from the database.
fn check_downloads(dir: &Path, saved: Option<(usize, u64)>, free: Option<u64>) -> Vec<Check> {
    let mut detail = dir.display().to_string();
    let mut notes = Vec::new();
    if let Some((count, size)) = saved.filter(|(count, _)| *count > 0) {
        notes.push(match count {
            1 => format!("1 video, {}", size_text(size)),
            n => format!("{n} videos, {}", size_text(size)),
        });
    }
    if let Some(free) = free {
        notes.push(format!("{} free", size_text(free)));
    }
    let status = match std::fs::metadata(dir) {
        Ok(m) if !m.is_dir() => {
            notes.push("not a folder".into());
            Status::Fail
        }
        Ok(_) if !writable(dir) => {
            notes.push("reeltty cannot write to it".into());
            Status::Fail
        }
        Ok(_) => Status::Ok,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            notes.push("not made yet".into());
            Status::Ok
        }
        Err(e) => {
            notes.push(e.to_string());
            Status::Fail
        }
    };
    if !notes.is_empty() {
        detail = format!("{detail} ({})", notes.join(", "));
    }
    let mut checks = vec![check("downloads", status, detail)];
    // Files of downloads that stopped before their end. A download that
    // goes on uses them; the others only take space.
    let parts: Vec<u64> = std::fs::read_dir(dir.join(".part"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.metadata().ok().filter(std::fs::Metadata::is_file).map(|m| m.len()))
        .collect();
    if !parts.is_empty() {
        let total = size_text(parts.iter().sum());
        let files = if parts.len() == 1 { "1 file".to_string() } else { format!("{} files", parts.len()) };
        checks.push(check("downloads", Status::Ok, format!("{files} ({total}) of unfinished downloads in .part")));
    }
    checks
}

/// Whether this user can make files in the folder `path`, which needs the
/// right to write and to search.
fn writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt as _;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
    // SAFETY: `c` is a valid C string for the length of the call.
    unsafe { libc::access(c.as_ptr(), libc::W_OK | libc::X_OK) == 0 }
}

/// The free space for this user on the disk of `path`, or of its nearest
/// folder that exists.
fn free_bytes(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt as _;
    let existing = path.ancestors().find(|p| p.exists())?;
    let c = std::ffi::CString::new(existing.as_os_str().as_bytes()).ok()?;
    // SAFETY: `stat` is plain data that `statvfs` fills, and `c` is a valid
    // C string for the length of the call.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &raw mut stat) } != 0 {
        return None;
    }
    #[allow(clippy::useless_conversion, reason = "the field types differ between platforms")]
    let (blocks, size) = (u64::from(stat.f_bavail), u64::from(stat.f_frsize));
    Some(blocks.saturating_mul(size))
}

/// yt-dlp needs ffmpeg to merge the video and the audio into one mkv file.
fn check_ffmpeg() -> Check {
    match Command::new("ffmpeg").arg("-version").output() {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            let version = text.lines().next().and_then(|l| l.split_whitespace().nth(2)).unwrap_or("found");
            check("ffmpeg", Status::Ok, format!("{version}, for format = \"mkv\" in [downloads]"))
        }
        _ => check(
            "ffmpeg",
            Status::Warn,
            "not found. format = \"mkv\" in [downloads] needs it, so downloads fail. Install ffmpeg, or remove the setting.",
        ),
    }
}

/// reeltty links to libmpv, so it only starts when the library is there.
fn check_libmpv() -> Check {
    let (major, minor) = player::api_version();
    let (min_major, min_minor) = player::MIN_API;
    if major == min_major && minor >= min_minor {
        check("libmpv", Status::Ok, format!("client API {major}.{minor}"))
    } else {
        check(
            "libmpv",
            Status::Fail,
            format!("client API {major}.{minor}, need {min_major}.{min_minor} or a later 2.x (mpv 0.37 or later)"),
        )
    }
}

fn check_yt_dlp(program: &Path, today: i64) -> Vec<Check> {
    // `-v` with no URL prints the debug header, then exits with an error.
    let out = match Command::new(program).arg("-v").output() {
        Ok(o) => o,
        Err(e) => {
            return vec![check("yt-dlp", Status::Fail, format!("cannot run {}: {e}", program.display()))];
        }
    };
    let info = parse_yt_dlp_debug(&String::from_utf8_lossy(&out.stderr));
    let mut checks = Vec::new();

    checks.push(match &info.version {
        Some(v) => match version_age_days(v, today) {
            Some(age) if age > YT_DLP_MAX_AGE_DAYS => check(
                "yt-dlp",
                Status::Warn,
                format!(
                    "{v} at {}, {age} days old. YouTube fixes ship in new releases. \
                     For the fastest fixes, set yt_dlp in the config to a nightly build (uv tool or pipx).",
                    program.display()
                ),
            ),
            Some(age) => check("yt-dlp", Status::Ok, format!("{v} at {}, {age} days old", program.display())),
            None => check("yt-dlp", Status::Warn, format!("{v}: cannot read the release date")),
        },
        None => check("yt-dlp", Status::Fail, format!("{} did not print a version", program.display())),
    });

    checks.push(match &info.ejs {
        Some(v) => check("yt-dlp-ejs", Status::Ok, v.clone()),
        None => check(
            "yt-dlp-ejs",
            Status::Fail,
            "not found. yt-dlp needs it for full YouTube support. Install yt-dlp-ejs.",
        ),
    });

    checks.push(match info.js_runtimes.as_deref() {
        Some(r) if r.contains("deno") => check("js runtime", Status::Ok, r),
        Some(r) => check("js runtime", Status::Warn, format!("{r}. yt-dlp recommends deno.")),
        None => check("js runtime", Status::Fail, "none. Without one, yt-dlp cannot play many videos. Install deno."),
    });
    checks
}

fn check_terminal() -> Check {
    match detect::detect() {
        Graphics::KittyShm => check("terminal", Status::Ok, "Kitty graphics with shared memory works"),
        g @ Graphics::Unavailable(_) => check(
            "terminal",
            Status::Warn,
            format!("{} Browsing works without thumbnails.", g.message().unwrap_or_default()),
        ),
    }
}

fn check_theme(name: ThemeName, no_color: bool) -> Check {
    if no_color {
        return check("theme", Status::Ok, "none, because NO_COLOR is set");
    }
    if name != ThemeName::Auto {
        return check("theme", Status::Ok, name.as_str());
    }
    check("theme", Status::Ok, auto_theme(detect::background()))
}

/// What `theme = "auto"` picks for the terminal `background`.
fn auto_theme(background: Option<(u8, u8, u8)>) -> String {
    match background {
        Some(c @ (r, g, b)) => {
            let (name, kind) = if detect::is_light(c) {
                (ThemeName::CatppuccinLatte, "light")
            } else {
                (ThemeName::CatppuccinMocha, "dark")
            };
            format!("auto: {} (the background #{r:02x}{g:02x}{b:02x} is {kind})", name.as_str())
        }
        None => format!("auto: {} (the terminal did not say its background)", ThemeName::CatppuccinMocha.as_str()),
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct YtDlpInfo {
    version: Option<String>,
    ejs: Option<String>,
    js_runtimes: Option<String>,
}

fn parse_yt_dlp_debug(text: &str) -> YtDlpInfo {
    let mut info = YtDlpInfo::default();
    for line in text.lines() {
        let Some(line) = line.strip_prefix("[debug] ") else { continue };
        if let Some(rest) = line.strip_prefix("yt-dlp version ") {
            // "stable@2026.10.26 from yt-dlp/yt-dlp [c91f8de0b]"
            let v = rest.split_whitespace().next().unwrap_or(rest);
            info.version = Some(v.to_string());
        } else if let Some(rest) = line.strip_prefix("Optional libraries: ") {
            info.ejs = rest.split(", ").find(|l| l.starts_with("yt_dlp_ejs")).map(str::to_string);
        } else if let Some(rest) = line.strip_prefix("JS runtimes: ")
            && rest != "none"
        {
            info.js_runtimes = Some(rest.to_string());
        }
    }
    info
}

/// Age in days of a yt-dlp version such as `stable@2026.10.26` or `nightly@2026.10.27.232410`.
fn version_age_days(version: &str, today: i64) -> Option<i64> {
    let v = version.rsplit('@').next()?;
    let mut parts = v.split('.');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    Some(today - days_from_civil(y, m, d)?)
}

fn days_since_epoch_now() -> i64 {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    i64::try_from(secs / 86_400).unwrap_or(0)
}

/// Days since 1970-01-01 for a date in the Gregorian calendar (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: u32, d: u32) -> Option<i64> {
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146_097 + doe - 719_468)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "\
[debug] Command-line config: ['-v']
[debug] yt-dlp version stable@2026.10.26 from yt-dlp/yt-dlp [c91f8de0b]
[debug] Optional libraries: certifi-2026.10.19, requests-2.34.2, sqlite3-3.54.0, urllib3-2.8.0, yt_dlp_ejs-0.8.0
[debug] JS runtimes: deno-2.9.8
[debug] Loaded 1747 extractors
";

    #[test]
    fn downloads_folder_checks() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("downloads");
        let c = check_downloads(&missing, Some((0, 0)), Some(5_000_000_000));
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].status, Status::Ok);
        assert!(c[0].detail.ends_with("downloads (5.0 GB free, not made yet)"), "{}", c[0].detail);

        std::fs::create_dir_all(missing.join(".part")).unwrap();
        std::fs::write(missing.join(".part/a.f136.mp4.part"), b"12345").unwrap();
        let c = check_downloads(&missing, Some((2, 412_000_000)), None);
        assert_eq!(c[0].status, Status::Ok);
        assert!(c[0].detail.ends_with("downloads (2 videos, 412 MB)"), "{}", c[0].detail);
        assert_eq!(c[1].detail, "1 file (5 B) of unfinished downloads in .part");
        assert!(check_downloads(&missing, Some((1, 9)), None)[0].detail.contains("(1 video, 9 B)"));

        let file = dir.path().join("file");
        std::fs::write(&file, b"").unwrap();
        let c = check_downloads(&file, None, None);
        assert_eq!((c[0].status, c[0].detail.ends_with("(not a folder)")), (Status::Fail, true), "{}", c[0].detail);

        // Without the right to search, no file can be made in it.
        let closed = dir.path().join("closed");
        std::fs::create_dir(&closed).unwrap();
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o600)).unwrap();
        let c = check_downloads(&closed, None, None);
        // The superuser can make files anyway.
        if unsafe { libc::geteuid() } != 0 {
            assert_eq!(c[0].status, Status::Fail, "{}", c[0].detail);
            assert!(c[0].detail.ends_with("(reeltty cannot write to it)"), "{}", c[0].detail);
        }
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o700)).unwrap();

        assert!(free_bytes(&missing.join("not/yet")).is_some());
    }

    #[test]
    fn parses_debug_header() {
        assert_eq!(
            parse_yt_dlp_debug(HEADER),
            YtDlpInfo {
                version: Some("stable@2026.10.26".into()),
                ejs: Some("yt_dlp_ejs-0.8.0".into()),
                js_runtimes: Some("deno-2.9.8".into()),
            }
        );
    }

    #[test]
    fn missing_ejs_and_runtime() {
        let text =
            "[debug] yt-dlp version 2025.01.01\n[debug] Optional libraries: certifi-1\n[debug] JS runtimes: none\n";
        let info = parse_yt_dlp_debug(text);
        assert_eq!(info.ejs, None);
        assert_eq!(info.js_runtimes, None);
    }

    #[test]
    fn says_what_auto_picks() {
        assert_eq!(auto_theme(Some((0xef, 0xf1, 0xf5))), "auto: catppuccin-latte (the background #eff1f5 is light)");
        assert_eq!(auto_theme(Some((0x1e, 0x1e, 0x2e))), "auto: catppuccin-mocha (the background #1e1e2e is dark)");
        assert_eq!(auto_theme(None), "auto: catppuccin-mocha (the terminal did not say its background)");
        assert_eq!(check_theme(ThemeName::TokyoNight, false).detail, "tokyo-night");
        assert_eq!(check_theme(ThemeName::Auto, true).detail, "none, because NO_COLOR is set");
    }

    #[test]
    fn civil_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), Some(0));
        assert_eq!(days_from_civil(2000, 3, 1), Some(11_017));
        assert_eq!(days_from_civil(2026, 10, 29), Some(20_755));
        assert_eq!(days_from_civil(2026, 13, 1), None);
    }

    #[test]
    fn version_ages() {
        let today = days_from_civil(2026, 10, 29).unwrap();
        assert_eq!(version_age_days("stable@2026.10.26", today), Some(3));
        assert_eq!(version_age_days("nightly@2026.10.27.232410", today), Some(2));
        assert_eq!(version_age_days("2026.09.01", today), Some(58));
        assert_eq!(version_age_days("garbage", today), None);
    }
}
