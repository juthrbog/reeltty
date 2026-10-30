//! `reeltty doctor`: checks the programs and libraries that reeltty needs.

use std::fmt;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{self, Config, Paths};
use crate::player;
use crate::store::Store;
use crate::terminal::detect::{self, Graphics};

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
            c
        }
        Err(e) => {
            emit(check("config", Status::Fail, format!("{e:#}")));
            Config::default()
        }
    };

    emit(match Store::open(&paths.database).and_then(|s| s.schema_version()) {
        Ok(v) => check("database", Status::Ok, format!("{} (schema {v})", paths.database.display())),
        Err(e) => check("database", Status::Fail, format!("{e:#}")),
    });

    emit(check_libmpv());
    for c in check_yt_dlp(&config.yt_dlp_program(), days_since_epoch_now()) {
        emit(c);
    }
    emit(check_terminal());

    !checks.iter().any(|c| c.status == Status::Fail)
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
