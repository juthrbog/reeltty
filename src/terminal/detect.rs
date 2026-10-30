//! Finds out if the terminal can show video.
//!
//! Video needs the Kitty graphics protocol with shared memory (`t=s`), in
//! Ghostty or kitty run directly. The checks:
//!
//! 1. The environment must not show a multiplexer or SSH.
//! 2. A Kitty graphics query with a test shared memory object must get `OK`
//!    before the reply to a DA1 request (`ESC [ c`).
//!
//! Do not trust the `e=` feature query key yet. kitty 0.49 does not reply to
//! it, and Ghostty 1.3.1 replies OK without support.

use std::io::Write as _;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;

use super::shm::ShmObject;

/// What the terminal can do for video and thumbnails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Graphics {
    /// Kitty graphics with shared memory works.
    KittyShm,
    /// No video. The text says why.
    Unavailable(String),
}

impl Graphics {
    pub fn is_available(&self) -> bool {
        matches!(self, Self::KittyShm)
    }

    /// The message to show where video would be.
    pub fn message(&self) -> Option<String> {
        match self {
            Self::KittyShm => None,
            Self::Unavailable(why) => Some(format!("Video needs Ghostty or kitty run directly. Detected: {why}")),
        }
    }
}

const MULTIPLEXER_PROGRAMS: &[&str] = &["tmux", "screen", "zellij", "herdr"];
const PROBE_ID: u32 = 31;
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);
const LATE_REPLY_GRACE: Duration = Duration::from_millis(300);

/// Check 1. `env` reads one environment variable.
pub fn environment_blocker(env: impl Fn(&str) -> Option<String>) -> Option<String> {
    if env("TMUX").is_some() {
        return Some("tmux".into());
    }
    if env("ZELLIJ").is_some() {
        return Some("Zellij".into());
    }
    if env("SSH_CONNECTION").is_some() {
        return Some("SSH".into());
    }
    let program = env("TERM_PROGRAM")?;
    MULTIPLEXER_PROGRAMS.contains(&program.to_lowercase().as_str()).then_some(program)
}

/// Runs both checks. Call it before the UI starts reading input, because it
/// reads the reply from stdin.
pub fn detect() -> Graphics {
    if let Some(why) = environment_blocker(|k| std::env::var(k).ok()) {
        return Graphics::Unavailable(why);
    }
    match probe_kitty_shm() {
        Ok(true) => Graphics::KittyShm,
        Ok(false) => Graphics::Unavailable(terminal_name()),
        Err(e) => {
            tracing::warn!("graphics probe failed: {e:#}");
            Graphics::Unavailable(format!("{} (probe failed: {e})", terminal_name()))
        }
    }
}

fn terminal_name() -> String {
    std::env::var("TERM_PROGRAM").or_else(|_| std::env::var("TERM")).unwrap_or_else(|_| "an unknown terminal".into())
}

/// Check 2. Returns `Ok(false)` if the terminal answered DA1 but not the query.
///
/// It talks to `/dev/tty`, so it also works when stdout is a pipe or a file
/// (for example `reeltty doctor | less`).
fn probe_kitty_shm() -> Result<bool> {
    use std::os::fd::AsRawFd as _;

    let mut tty = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty").context("open /dev/tty")?;
    let fd = tty.as_raw_fd();

    let name = format!("/reeltty-probe-{}", std::process::id());
    let shm = ShmObject::create(&name, &[0, 0, 0])?;

    let _raw = RawMode::enter(fd)?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&name);
    write!(tty, "\x1b_Gi={PROBE_ID},s=1,v=1,a=q,t=s,f=24;{b64}\x1b\\\x1b[c")?;
    tty.flush()?;
    let reply = read_reply(fd, PROBE_TIMEOUT)?;
    drop(shm);
    Ok(reply.kitty_ok == Some(true))
}

/// Raw mode on one terminal descriptor. The old mode comes back on drop.
struct RawMode {
    fd: i32,
    saved: libc::termios,
}

impl RawMode {
    fn enter(fd: i32) -> Result<Self> {
        // SAFETY: termios is plain data, and tcgetattr fills it in.
        let mut t: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(fd, &raw mut t) } != 0 {
            return Err(std::io::Error::last_os_error()).context("tcgetattr");
        }
        let saved = t;
        unsafe { libc::cfmakeraw(&raw mut t) };
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw const t) } != 0 {
            return Err(std::io::Error::last_os_error()).context("tcsetattr");
        }
        Ok(Self { fd, saved })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        // TCSAFLUSH drops unread input, so a late reply never becomes key presses.
        unsafe { libc::tcsetattr(self.fd, libc::TCSAFLUSH, &raw const self.saved) };
    }
}

fn read_reply(fd: i32, timeout: Duration) -> Result<ProbeReply> {
    let deadline = Instant::now() + timeout;
    let mut buf = Vec::new();
    loop {
        let reply = parse_reply(&buf);
        if reply.da1 {
            return Ok(reply);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            // Wait a little longer for a slow reply, so that it does not reach
            // the UI as key presses. RawMode's drop discards what is left.
            drain_until_da1(fd, buf, LATE_REPLY_GRACE);
            bail!("no reply from the terminal");
        }
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        let ms = i32::try_from(left.as_millis()).unwrap_or(i32::MAX).max(1);
        if unsafe { libc::poll(&raw mut pfd, 1, ms) } <= 0 {
            continue;
        }
        let mut chunk = [0u8; 256];
        let n = unsafe { libc::read(fd, chunk.as_mut_ptr().cast(), chunk.len()) };
        if n <= 0 {
            bail!("read from the terminal failed");
        }
        buf.extend_from_slice(&chunk[..n.cast_unsigned()]);
    }
}

fn drain_until_da1(fd: i32, mut buf: Vec<u8>, window: Duration) {
    let deadline = Instant::now() + window;
    while !parse_reply(&buf).da1 {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        let ms = i32::try_from(left.as_millis()).unwrap_or(i32::MAX).max(1);
        if unsafe { libc::poll(&raw mut pfd, 1, ms) } <= 0 {
            continue;
        }
        let mut chunk = [0u8; 256];
        let n = unsafe { libc::read(fd, chunk.as_mut_ptr().cast(), chunk.len()) };
        if n <= 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n.cast_unsigned()]);
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ProbeReply {
    /// `Some(true)` for `OK`, `Some(false)` for an error reply, `None` for no reply.
    kitty_ok: Option<bool>,
    da1: bool,
}

fn parse_reply(buf: &[u8]) -> ProbeReply {
    let mut reply = ProbeReply::default();
    let s = String::from_utf8_lossy(buf);
    let mut rest = s.as_ref();
    while let Some(start) = rest.find("\x1b_G") {
        let body = &rest[start + 3..];
        let Some(end) = body.find("\x1b\\") else { break };
        let apc = &body[..end];
        if let Some((keys, msg)) = apc.split_once(';')
            && keys.split(',').any(|kv| kv == format!("i={PROBE_ID}"))
        {
            reply.kitty_ok = Some(msg == "OK");
        }
        rest = &body[end + 2..];
    }
    // DA1 reply: ESC [ ? <params> c
    if let Some(i) = s.find("\x1b[?") {
        reply.da1 = s[i + 3..].bytes().any(|b| b == b'c');
    }
    reply
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        |k| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| (*v).to_string())
    }

    #[test]
    fn environment_finds_multiplexers() {
        assert_eq!(environment_blocker(env(&[])), None);
        assert_eq!(environment_blocker(env(&[("TERM_PROGRAM", "ghostty")])), None);
        assert_eq!(environment_blocker(env(&[("TERM_PROGRAM", "herdr")])), Some("herdr".into()));
        assert_eq!(environment_blocker(env(&[("TMUX", "/tmp/x,1,0")])), Some("tmux".into()));
        assert_eq!(environment_blocker(env(&[("ZELLIJ", "0")])), Some("Zellij".into()));
        assert_eq!(environment_blocker(env(&[("SSH_CONNECTION", "a b c d")])), Some("SSH".into()));
    }

    #[test]
    fn parses_ok_then_da1() {
        let r = parse_reply(b"\x1b_Gi=31;OK\x1b\\\x1b[?62;22;52c");
        assert_eq!(r, ProbeReply { kitty_ok: Some(true), da1: true });
    }

    #[test]
    fn parses_error_reply() {
        let r = parse_reply(b"\x1b_Gi=31;EBADF:could not open\x1b\\\x1b[?62c");
        assert_eq!(r, ProbeReply { kitty_ok: Some(false), da1: true });
    }

    #[test]
    fn da1_alone_means_no_support() {
        assert_eq!(parse_reply(b"\x1b[?1;2c"), ProbeReply { kitty_ok: None, da1: true });
    }

    #[test]
    fn partial_reply_waits() {
        assert_eq!(parse_reply(b"\x1b_Gi=31;O"), ProbeReply::default());
        assert_eq!(parse_reply(b"\x1b[?62;2"), ProbeReply::default());
    }

    #[test]
    fn ignores_other_image_ids() {
        let r = parse_reply(b"\x1b_Gi=7;OK\x1b\\\x1b[?62c");
        assert_eq!(r.kitty_ok, None);
    }

    #[test]
    fn message_names_the_blocker() {
        let g = Graphics::Unavailable("herdr".into());
        assert_eq!(g.message().unwrap(), "Video needs Ghostty or kitty run directly. Detected: herdr");
        assert!(Graphics::KittyShm.message().is_none());
    }
}
