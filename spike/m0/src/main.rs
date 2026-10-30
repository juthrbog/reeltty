//! M0 playback spike for reeltty. Throwaway code, see `README.md`.
//!
//! The path under test: yt-dlp resolves the stream URLs, libmpv decodes and
//! renders each frame with the software render API, and the frames go to the
//! terminal with the Kitty graphics protocol through POSIX shared memory.

mod diacritics;

use std::collections::VecDeque;
use std::ffi::{CStr, CString, c_char, c_void};
use std::fmt::Write as _;
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::os::fd::FromRawFd as _;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use libmpv2_sys as mpv;

const IMAGE_ID: u32 = 1;
const PLACEMENT_ID: u32 = 1;
const PLACEHOLDER: char = '\u{10EEEE}';

// ---------------------------------------------------------------------------
// Arguments
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Placement {
    /// `a=T` with a placement at the cursor. The image has its native pixel size.
    Direct,
    /// `a=T,U=1` virtual placement, shown by U+10EEEE placeholder cells.
    Unicode,
}

#[derive(Debug)]
struct Args {
    target: String,
    size: Option<(u32, u32)>,
    max_height: u32,
    max_fps: Option<u32>,
    seconds: Option<f64>,
    start: Option<f64>,
    placement: Placement,
    transient: bool,
    sync: bool,
    hwdec: String,
    sw_format: String,
    max_inflight: usize,
    headless: bool,
    log: String,
    no_ytdl: bool,
    mpv_opts: Vec<(String, String)>,
}

const USAGE: &str = "\
usage: m0-spike [options] <youtube-url | local-file>

  --size WxH            render size in pixels (default: fit the pane, max 960x540)
  --max-height N        highest video format to ask yt-dlp for (default 720)
  --max-fps N           highest frame rate to ask yt-dlp for (default: any)
  --seconds N           quit after N seconds of wall time from first frame
  --start N             start position in seconds
  --placement MODE      direct | unicode (default direct)
  --no-transient        do not send the N=1 transient hint
  --no-sync             do not wrap frames in synchronized output (mode 2026)
  --hwdec VALUE         mpv hwdec option (default auto-copy-safe)
  --sw-format FMT       software render format (default rgb0)
  --max-inflight N      shared memory objects the terminal may lag behind (default 2)
  --headless            render frames but write nothing to the terminal
  --log FILE            stats and mpv log file (default m0.log)
  --no-ytdl             play the target as a plain mpv URL or file
  --mpv KEY=VALUE       extra mpv option, can repeat (e.g. --mpv sws-scaler=bilinear)

keys: q or Ctrl-C quits, space pauses, left and right seek 5 seconds
";

fn parse_args() -> Result<Args> {
    let mut args = Args {
        target: String::new(),
        size: None,
        max_height: 720,
        max_fps: None,
        seconds: None,
        start: None,
        placement: Placement::Direct,
        transient: true,
        sync: true,
        hwdec: "auto-copy-safe".into(),
        sw_format: "rgb0".into(),
        max_inflight: 2,
        headless: false,
        log: "m0.log".into(),
        no_ytdl: false,
        mpv_opts: Vec::new(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        let mut val = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                std::process::exit(0);
            }
            "--size" => {
                let v = val()?;
                let (w, h) = v.split_once('x').context("--size must be WxH")?;
                args.size = Some((w.parse()?, h.parse()?));
            }
            "--max-height" => args.max_height = val()?.parse()?,
            "--max-fps" => args.max_fps = Some(val()?.parse()?),
            "--seconds" => args.seconds = Some(val()?.parse()?),
            "--start" => args.start = Some(val()?.parse()?),
            "--placement" => {
                args.placement = match val()?.as_str() {
                    "direct" => Placement::Direct,
                    "unicode" => Placement::Unicode,
                    other => bail!("unknown placement {other}"),
                }
            }
            "--no-transient" => args.transient = false,
            "--no-sync" => args.sync = false,
            "--hwdec" => args.hwdec = val()?,
            "--sw-format" => args.sw_format = val()?,
            "--max-inflight" => args.max_inflight = val()?.parse()?,
            "--headless" => args.headless = true,
            "--log" => args.log = val()?,
            "--no-ytdl" => args.no_ytdl = true,
            "--mpv" => {
                let v = val()?;
                let (k, v) = v.split_once('=').context("--mpv must be KEY=VALUE")?;
                args.mpv_opts.push((k.into(), v.into()));
            }
            s if s.starts_with('-') => bail!("unknown option {s}\n\n{USAGE}"),
            s => args.target = s.to_string(),
        }
    }
    if args.target.is_empty() {
        bail!("{USAGE}");
    }
    Ok(args)
}

// ---------------------------------------------------------------------------
// Log file
// ---------------------------------------------------------------------------

static LOG: Mutex<Option<File>> = Mutex::new(None);

fn log_line(s: &str) {
    if let Some(f) = LOG.lock().unwrap().as_mut() {
        let _ = writeln!(f, "{s}");
    }
}

macro_rules! logf {
    ($($t:tt)*) => { log_line(&format!($($t)*)) };
}

// ---------------------------------------------------------------------------
// yt-dlp
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct Streams {
    video_url: String,
    audio_url: Option<String>,
    headers: Vec<(String, String)>,
    available_at: Option<u64>,
    summary: String,
}

fn resolve(args: &Args) -> Result<Streams> {
    if args.no_ytdl {
        return Ok(Streams {
            video_url: args.target.clone(),
            summary: "plain url, no yt-dlp".into(),
            ..Streams::default()
        });
    }
    let h = args.max_height;
    let fps = args
        .max_fps
        .map(|f| format!("[fps<={f}]"))
        .unwrap_or_default();
    // H.264 first, then VP9, never AV1. Opus or AAC audio.
    let selector = format!(
        "bv*[vcodec^=avc1][height<={h}]{fps}+ba[acodec=opus]/\
         bv*[vcodec^=avc1][height<={h}]{fps}+ba[acodec^=mp4a]/\
         bv*[vcodec^=vp09][height<={h}]{fps}+ba[acodec=opus]/\
         b[vcodec^=avc1][height<={h}]"
    );
    eprintln!("resolving with yt-dlp ...");
    let t0 = Instant::now();
    let out = Command::new("yt-dlp")
        .args(["-J", "--no-playlist", "-f", &selector, &args.target])
        .output()
        .context("run yt-dlp")?;
    if !out.status.success() {
        bail!("yt-dlp failed:\n{}", String::from_utf8_lossy(&out.stderr));
    }
    let resolve_time = t0.elapsed();
    let j: serde_json::Value = serde_json::from_slice(&out.stdout).context("parse yt-dlp json")?;

    let headers_of = |v: &serde_json::Value| -> Vec<(String, String)> {
        v.get("http_headers")
            .and_then(|h| h.as_object())
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    };
    let available_at =
        |v: &serde_json::Value| v.get("available_at").and_then(serde_json::Value::as_u64);
    let desc = |v: &serde_json::Value| {
        format!(
            "{} {}x{} {}fps v={} a={}",
            v["format_id"].as_str().unwrap_or("?"),
            v["width"].as_u64().unwrap_or(0),
            v["height"].as_u64().unwrap_or(0),
            v["fps"].as_f64().unwrap_or(0.0),
            v["vcodec"].as_str().unwrap_or("?"),
            v["acodec"].as_str().unwrap_or("?"),
        )
    };

    let title = j["title"].as_str().unwrap_or("?");
    let mut s = Streams::default();
    if let Some(fmts) = j.get("requested_formats").and_then(|f| f.as_array()) {
        let mut parts = Vec::new();
        for f in fmts {
            let url = f["url"].as_str().context("format without url")?.to_string();
            let is_video = f["vcodec"].as_str().is_some_and(|c| c != "none");
            if is_video {
                s.video_url = url;
                s.headers = headers_of(f);
            } else {
                s.audio_url = Some(url);
            }
            s.available_at = s.available_at.max(available_at(f));
            parts.push(desc(f));
        }
        s.summary = format!(
            "{title} | {} | yt-dlp {:.1}s",
            parts.join(" + "),
            resolve_time.as_secs_f64()
        );
    } else {
        s.video_url = j["url"]
            .as_str()
            .context("no url in yt-dlp output")?
            .to_string();
        s.headers = headers_of(&j);
        s.available_at = available_at(&j);
        s.summary = format!(
            "{title} | {} | yt-dlp {:.1}s",
            desc(&j),
            resolve_time.as_secs_f64()
        );
    }
    if s.headers.is_empty() {
        s.headers = headers_of(&j);
    }
    s.available_at = s.available_at.max(available_at(&j));
    if s.video_url.is_empty() {
        bail!("yt-dlp gave no video stream");
    }
    Ok(s)
}

fn wait_available(at: u64) {
    loop {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        if now >= at {
            break;
        }
        eprint!("\rstream available in {:>3}s ", at - now);
        thread::sleep(Duration::from_millis(500));
    }
    eprintln!();
}

// ---------------------------------------------------------------------------
// mpv
// ---------------------------------------------------------------------------

struct Mpv(*mut mpv::mpv_handle);
// SAFETY: the mpv client API is thread safe for a handle.
unsafe impl Send for Mpv {}
unsafe impl Sync for Mpv {}

fn cstr(s: &str) -> CString {
    CString::new(s).expect("no NUL in string")
}

impl Mpv {
    fn check(code: i32, what: &str) -> Result<()> {
        if code < 0 {
            bail!("mpv {what}: {}", mpv::mpv_error_str(code));
        }
        Ok(())
    }

    fn set_option(&self, name: &str, value: &str) -> Result<()> {
        let (n, v) = (cstr(name), cstr(value));
        Self::check(
            unsafe { mpv::mpv_set_option_string(self.0, n.as_ptr(), v.as_ptr()) },
            name,
        )
    }

    fn command(&self, args: &[&str]) -> Result<()> {
        let owned: Vec<CString> = args.iter().map(|a| cstr(a)).collect();
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        Self::check(
            unsafe { mpv::mpv_command(self.0, ptrs.as_mut_ptr()) },
            args[0],
        )
    }

    fn get(&self, name: &str) -> Option<String> {
        let n = cstr(name);
        unsafe {
            let p = mpv::mpv_get_property_string(self.0, n.as_ptr());
            if p.is_null() {
                return None;
            }
            let s = CStr::from_ptr(p).to_string_lossy().into_owned();
            mpv::mpv_free(p.cast());
            Some(s)
        }
    }

    fn get_f64(&self, name: &str) -> Option<f64> {
        self.get(name)?.parse().ok()
    }
}

fn create_mpv(args: &Args, streams: &Streams) -> Result<Mpv> {
    let h = unsafe { mpv::mpv_create() };
    if h.is_null() {
        bail!("mpv_create failed");
    }
    let m = Mpv(h);
    m.set_option("vo", "libmpv")?;
    m.set_option("hwdec", &args.hwdec)?;
    m.set_option("sws-fast", "yes")?;
    m.set_option("ytdl", "no")?;
    m.set_option("terminal", "no")?;
    m.set_option("input-default-bindings", "no")?;
    m.set_option("osc", "no")?;
    m.set_option("cache", "yes")?;
    m.set_option("demuxer-max-bytes", "64MiB")?;
    for (k, v) in &args.mpv_opts {
        m.set_option(k, v)?;
    }
    if let Some(s) = args.start {
        m.set_option("start", &s.to_string())?;
    }
    Mpv::check(unsafe { mpv::mpv_initialize(h) }, "initialize")?;
    let lvl = cstr("info");
    unsafe { mpv::mpv_request_log_messages(h, lvl.as_ptr()) };

    for (k, v) in &streams.headers {
        if k.eq_ignore_ascii_case("user-agent") {
            m.command(&["set", "user-agent", v])?;
        } else {
            // `append` adds one item and does not split on commas.
            m.command(&[
                "change-list",
                "http-header-fields",
                "append",
                &format!("{k}: {v}"),
            ])?;
        }
    }
    if let Some(a) = &streams.audio_url {
        m.command(&["change-list", "audio-files", "append", a])?;
    }
    Ok(m)
}

// ---------------------------------------------------------------------------
// Terminal
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Winsize {
    cols: u16,
    rows: u16,
    xpix: u16,
    ypix: u16,
}

fn winsize() -> Result<Winsize> {
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) } != 0 {
        bail!("TIOCGWINSZ failed");
    }
    Ok(Winsize {
        cols: ws.ws_col,
        rows: ws.ws_row,
        xpix: ws.ws_xpixel,
        ypix: ws.ws_ypixel,
    })
}

/// Puts the terminal in raw mode and the alternate screen, and undoes both on drop.
struct TermGuard {
    saved: libc::termios,
}

impl TermGuard {
    fn enter() -> Result<Self> {
        let mut t: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut t) } != 0 {
            bail!("tcgetattr failed: stdin is not a terminal");
        }
        let saved = t;
        unsafe { libc::cfmakeraw(&mut t) };
        unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &t) };
        // ?7l: no auto-wrap, so text on the last row can never scroll the screen.
        out(b"\x1b[?1049h\x1b[?25l\x1b[?7l\x1b[2J");
        Ok(Self { saved })
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        out(
            format!("\x1b_Ga=d,d=I,i={IMAGE_ID},q=2\x1b\\\x1b[0m\x1b[?7h\x1b[?25h\x1b[?1049l")
                .as_bytes(),
        );
        unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.saved) };
    }
}

/// Writes one buffer to stdout under the stdout lock, so that sequences
/// from different threads never interleave.
fn out(buf: &[u8]) {
    let mut o = std::io::stdout().lock();
    let _ = o.write_all(buf);
    let _ = o.flush();
}

/// Picks a render size that fits in the pane above the status row, at most 960x540,
/// with a 16:9 shape. Width is a multiple of 16, so the stride is a multiple of 64.
fn fit_size(ws: Winsize) -> (u32, u32) {
    let cell_h = if ws.rows > 0 {
        u32::from(ws.ypix) / u32::from(ws.rows)
    } else {
        0
    };
    let avail_w = u32::from(ws.xpix).min(960);
    let avail_h = u32::from(ws.ypix).saturating_sub(cell_h).min(540);
    let (mut w, mut h) = (avail_w, avail_w * 9 / 16);
    if h > avail_h {
        h = avail_h;
        w = h * 16 / 9;
    }
    ((w / 16 * 16).max(16), (h / 2 * 2).max(2))
}

// ---------------------------------------------------------------------------
// Frame writer
// ---------------------------------------------------------------------------

struct AlignedBuf {
    ptr: *mut u8,
    layout: std::alloc::Layout,
}

impl AlignedBuf {
    fn new(len: usize) -> Self {
        let layout = std::alloc::Layout::from_size_align(len, 64).unwrap();
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        assert!(!ptr.is_null());
        Self { ptr, layout }
    }
    fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.layout.size()) }
    }
}

impl Drop for AlignedBuf {
    fn drop(&mut self) {
        unsafe { std::alloc::dealloc(self.ptr, self.layout) };
    }
}

struct Writer {
    w: u32,
    h: u32,
    cols: u16,
    term_cols: u16,
    rows: u16,
    placement: Placement,
    transient: bool,
    sync: bool,
    max_inflight: usize,
    inflight: VecDeque<String>,
    counter: u64,
    placeholders_drawn: bool,
}

impl Writer {
    /// Forgets objects that the terminal already read and unlinked.
    fn reap(&mut self) {
        self.inflight
            .retain(|n| std::path::Path::new(&format!("/dev/shm{n}")).exists());
    }

    fn backpressured(&mut self) -> bool {
        self.reap();
        self.inflight.len() >= self.max_inflight
    }

    fn shm_write(&mut self, bytes: &[u8]) -> Result<String> {
        self.counter += 1;
        let name = format!("/reeltty-m0-{}-{}", std::process::id(), self.counter);
        let c = cstr(&name);
        let fd = unsafe {
            libc::shm_open(
                c.as_ptr(),
                libc::O_CREAT | libc::O_EXCL | libc::O_RDWR,
                0o600,
            )
        };
        if fd < 0 {
            bail!("shm_open {name}: {}", std::io::Error::last_os_error());
        }
        // Linux: write(2) sets the size and allocates the tmpfs pages, no mmap needed.
        let mut f = unsafe { File::from_raw_fd(fd) };
        f.write_all(bytes)?;
        self.inflight.push_back(name.clone());
        Ok(name)
    }

    fn frame_escape(&self, name: &str, status: Option<&str>) -> Vec<u8> {
        let mut s = String::with_capacity(256);
        if self.sync {
            s.push_str("\x1b[?2026h");
        }
        let b64 = base64::engine::general_purpose::STANDARD.encode(name);
        let n = if self.transient { ",N=1" } else { "" };
        let (w, h) = (self.w, self.h);
        match self.placement {
            Placement::Direct => {
                // C=1: the cursor does not move, so nothing scrolls.
                let _ = write!(
                    s,
                    "\x1b[1;1H\x1b_Ga=T,f=32,t=s,s={w},v={h},i={IMAGE_ID},p={PLACEMENT_ID},C=1,q=2{n};{b64}\x1b\\"
                );
            }
            Placement::Unicode => {
                let (c, r) = (self.cols, self.rows);
                let _ = write!(
                    s,
                    "\x1b_Ga=T,U=1,f=32,t=s,s={w},v={h},i={IMAGE_ID},p={PLACEMENT_ID},c={c},r={r},q=2{n};{b64}\x1b\\"
                );
                if !self.placeholders_drawn {
                    s.push_str(&self.placeholders());
                }
            }
        }
        if let Some(st) = status {
            let st: String = st.chars().take(usize::from(self.term_cols)).collect();
            let _ = write!(s, "\x1b[{};1H\x1b[0m\x1b[2K{st}", self.rows + 1);
        }
        if self.sync {
            s.push_str("\x1b[?2026l");
        }
        s.into_bytes()
    }

    fn placeholders(&self) -> String {
        let mut s = String::new();
        // The image ID rides in the 24-bit foreground color.
        let _ = write!(
            s,
            "\x1b[38;2;{};{};{}m",
            (IMAGE_ID >> 16) & 255,
            (IMAGE_ID >> 8) & 255,
            IMAGE_ID & 255
        );
        let max = diacritics::DIACRITICS.len();
        for y in 0..usize::from(self.rows).min(max) {
            let _ = write!(s, "\x1b[{};1H", y + 1);
            for x in 0..usize::from(self.cols).min(max) {
                s.push(PLACEHOLDER);
                if x == 0 {
                    s.push(diacritics::DIACRITICS[y]);
                    s.push(diacritics::DIACRITICS[0]);
                }
            }
        }
        s.push_str("\x1b[39m");
        s
    }

    fn cleanup(&mut self) {
        // Give the terminal a moment to read the last frames, then unlink the rest.
        for _ in 0..20 {
            self.reap();
            if self.inflight.is_empty() {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        for n in self.inflight.drain(..) {
            let c = cstr(&n);
            unsafe { libc::shm_unlink(c.as_ptr()) };
        }
    }
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Counters {
    rendered: AtomicU64,
    presented: AtomicU64,
    dropped_backpressure: AtomicU64,
    render_ns: AtomicU64,
    present_ns: AtomicU64,
}

fn cpu_self_secs() -> f64 {
    let mut ru: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut ru) };
    let tv = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
    tv(ru.ru_utime) + tv(ru.ru_stime)
}

/// Reads `(comm, ppid, utime + stime in seconds)` from /proc.
fn proc_stat(pid: i32) -> Option<(String, i32, f64)> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let open = s.find('(')?;
    let close = s.rfind(')')?;
    let comm = s[open + 1..close].to_string();
    let f: Vec<&str> = s[close + 2..].split(' ').collect();
    let ppid = f.get(1)?.parse().ok()?;
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    let t = f.get(11)?.parse::<f64>().ok()? + f.get(12)?.parse::<f64>().ok()?;
    Some((comm, ppid, t / ticks))
}

/// Walks up the process tree to the terminal emulator.
fn find_terminal() -> Option<(i32, String)> {
    let mut pid = std::process::id() as i32;
    for _ in 0..16 {
        let (comm, ppid, _) = proc_stat(pid)?;
        let c = comm.to_lowercase();
        if c.contains("ghostty") || c.contains("kitty") {
            return Some((pid, comm));
        }
        if ppid <= 1 {
            return None;
        }
        pid = ppid;
    }
    None
}

fn percentile(sorted: &[f32], p: f64) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[i]
}

// ---------------------------------------------------------------------------
// Render thread
// ---------------------------------------------------------------------------

struct Wake {
    flag: Mutex<bool>,
    cv: Condvar,
}

unsafe extern "C" fn on_update(ctx: *mut c_void) {
    // SAFETY: ctx is the `Wake` that `main` keeps alive until the render context is freed.
    let wake = unsafe { &*ctx.cast::<Wake>() };
    *wake
        .flag
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
    wake.cv.notify_one();
}

struct Shared {
    quit: AtomicBool,
    status: Mutex<Option<String>>,
    counters: Counters,
    intervals_ms: Mutex<Vec<f32>>,
    first_frame: Mutex<Option<Instant>>,
}

struct RenderCtx(*mut mpv::mpv_render_context);
unsafe impl Send for RenderCtx {}

fn render_loop(
    ctx: RenderCtx,
    wake: Arc<Wake>,
    shared: Arc<Shared>,
    mut writer: Option<Writer>,
    (w, h): (u32, u32),
    sw_format: String,
) {
    let stride = w as usize * 4;
    let mut buf = AlignedBuf::new(stride * h as usize);
    let fmt = cstr(&sw_format);
    let mut last_present: Option<Instant> = None;

    while !shared.quit.load(Ordering::Relaxed) {
        {
            let mut f = wake.flag.lock().unwrap();
            if !*f {
                f = wake
                    .cv
                    .wait_timeout(f, Duration::from_millis(100))
                    .unwrap()
                    .0;
            }
            if !*f {
                continue;
            }
            *f = false;
        }
        let flags = unsafe { mpv::mpv_render_context_update(ctx.0) };
        if flags & u64::from(mpv::mpv_render_update_flag_MPV_RENDER_UPDATE_FRAME) == 0 {
            continue;
        }

        let mut size: [i32; 2] = [w as i32, h as i32];
        let mut stride_v: usize = stride;
        let mut params = [
            mpv::mpv_render_param {
                type_: mpv::mpv_render_param_type_MPV_RENDER_PARAM_SW_SIZE,
                data: size.as_mut_ptr().cast(),
            },
            mpv::mpv_render_param {
                type_: mpv::mpv_render_param_type_MPV_RENDER_PARAM_SW_FORMAT,
                data: fmt.as_ptr().cast_mut().cast(),
            },
            mpv::mpv_render_param {
                type_: mpv::mpv_render_param_type_MPV_RENDER_PARAM_SW_STRIDE,
                data: (&raw mut stride_v).cast(),
            },
            mpv::mpv_render_param {
                type_: mpv::mpv_render_param_type_MPV_RENDER_PARAM_SW_POINTER,
                data: buf.ptr.cast(),
            },
            mpv::mpv_render_param {
                type_: mpv::mpv_render_param_type_MPV_RENDER_PARAM_INVALID,
                data: std::ptr::null_mut(),
            },
        ];
        let t0 = Instant::now();
        let rc = unsafe { mpv::mpv_render_context_render(ctx.0, params.as_mut_ptr()) };
        let render_dt = t0.elapsed();
        if rc < 0 {
            logf!("render error: {}", mpv::mpv_error_str(rc));
            continue;
        }
        let c = &shared.counters;
        c.rendered.fetch_add(1, Ordering::Relaxed);
        c.render_ns
            .fetch_add(render_dt.as_nanos() as u64, Ordering::Relaxed);

        if let Some(wr) = writer.as_mut() {
            let t1 = Instant::now();
            if wr.backpressured() {
                c.dropped_backpressure.fetch_add(1, Ordering::Relaxed);
            } else {
                let bytes = buf.as_mut_slice();
                // rgb0 leaves the 4th byte undefined. Kitty f=32 reads it as alpha.
                if sw_format.ends_with('0') {
                    let alpha = if sw_format.starts_with('0') { 0 } else { 3 };
                    for px in bytes.chunks_exact_mut(4) {
                        px[alpha] = 255;
                    }
                }
                match wr.shm_write(bytes) {
                    Ok(name) => {
                        let status = shared.status.lock().unwrap().take();
                        out(&wr.frame_escape(&name, status.as_deref()));
                        wr.placeholders_drawn = true;
                        c.presented.fetch_add(1, Ordering::Relaxed);
                        let now = Instant::now();
                        if let Some(lp) = last_present {
                            shared
                                .intervals_ms
                                .lock()
                                .unwrap()
                                .push((now - lp).as_secs_f32() * 1000.0);
                        } else {
                            *shared.first_frame.lock().unwrap() = Some(now);
                        }
                        last_present = Some(now);
                    }
                    Err(e) => logf!("shm write: {e:#}"),
                }
            }
            c.present_ns
                .fetch_add(t1.elapsed().as_nanos() as u64, Ordering::Relaxed);
        } else {
            let now = Instant::now();
            if let Some(lp) = last_present {
                shared
                    .intervals_ms
                    .lock()
                    .unwrap()
                    .push((now - lp).as_secs_f32() * 1000.0);
            } else {
                *shared.first_frame.lock().unwrap() = Some(now);
            }
            last_present = Some(now);
        }
        unsafe { mpv::mpv_render_context_report_swap(ctx.0) };
    }

    if let Some(wr) = writer.as_mut() {
        wr.cleanup();
    }
    unsafe { mpv::mpv_render_context_free(ctx.0) };
}

// ---------------------------------------------------------------------------
// Event thread
// ---------------------------------------------------------------------------

fn event_loop(m: Arc<Mpv>, shared: Arc<Shared>, ended: Arc<AtomicBool>) {
    while !shared.quit.load(Ordering::Relaxed) {
        let ev = unsafe { &*mpv::mpv_wait_event(m.0, 0.25) };
        match ev.event_id {
            mpv::mpv_event_id_MPV_EVENT_LOG_MESSAGE => {
                let msg = unsafe { &*ev.data.cast::<mpv::mpv_event_log_message>() };
                let txt = |p: *const c_char| unsafe { CStr::from_ptr(p) }.to_string_lossy();
                let line = format!(
                    "mpv [{}] {}: {}",
                    txt(msg.level),
                    txt(msg.prefix),
                    txt(msg.text)
                );
                log_line(line.trim_end());
            }
            mpv::mpv_event_id_MPV_EVENT_END_FILE => {
                let ef = unsafe { &*ev.data.cast::<mpv::mpv_event_end_file>() };
                logf!(
                    "end-file reason={} error={}",
                    ef.reason,
                    mpv::mpv_error_str(ef.error)
                );
                ended.store(true, Ordering::Relaxed);
            }
            mpv::mpv_event_id_MPV_EVENT_SHUTDOWN => {
                ended.store(true, Ordering::Relaxed);
                break;
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

#[allow(clippy::too_many_lines)]
fn run() -> Result<()> {
    let args = parse_args()?;
    *LOG.lock().unwrap() =
        Some(File::create(&args.log).with_context(|| format!("create {}", args.log))?);
    logf!("args: {args:?}");

    for var in ["TMUX", "ZELLIJ", "SSH_CONNECTION"] {
        if std::env::var_os(var).is_some() && !args.headless {
            eprintln!("warning: {var} is set; video needs Ghostty or kitty run directly");
        }
    }
    if let Ok(tp) = std::env::var("TERM_PROGRAM") {
        logf!("TERM_PROGRAM={tp}");
    }

    let ws = winsize().unwrap_or(Winsize {
        cols: 120,
        rows: 40,
        xpix: 0,
        ypix: 0,
    });
    logf!("winsize: {ws:?}");
    let (w, h) = match args.size {
        Some((w, h)) if w == 0 || h == 0 => bail!("--size must not be zero"),
        Some(s) => s,
        None if ws.xpix == 0 || ws.ypix == 0 => (960, 540),
        None => fit_size(ws),
    };
    if w % 16 != 0 {
        eprintln!("note: width {w} is not a multiple of 16, so the stride is not 64-byte aligned");
    }
    let cell_w = if ws.cols > 0 && ws.xpix > 0 {
        f64::from(ws.xpix) / f64::from(ws.cols)
    } else {
        8.0
    };
    let cell_h = if ws.rows > 0 && ws.ypix > 0 {
        f64::from(ws.ypix) / f64::from(ws.rows)
    } else {
        16.0
    };
    let cols = ((f64::from(w) / cell_w).ceil() as u16).min(ws.cols.max(1));
    let rows = ((f64::from(h) / cell_h).ceil() as u16).min(ws.rows.saturating_sub(1).max(1));
    logf!("render size {w}x{h}, {cols}x{rows} cells");

    let streams = resolve(&args)?;
    logf!("streams: {}", streams.summary);
    eprintln!("{}", streams.summary);
    if let Some(at) = streams.available_at {
        wait_available(at);
    }

    let m = Arc::new(create_mpv(&args, &streams)?);

    // Render context.
    let api = cstr("sw");
    let mut params = [
        mpv::mpv_render_param {
            type_: mpv::mpv_render_param_type_MPV_RENDER_PARAM_API_TYPE,
            data: api.as_ptr().cast_mut().cast(),
        },
        mpv::mpv_render_param {
            type_: mpv::mpv_render_param_type_MPV_RENDER_PARAM_INVALID,
            data: std::ptr::null_mut(),
        },
    ];
    let mut rctx: *mut mpv::mpv_render_context = std::ptr::null_mut();
    Mpv::check(
        unsafe { mpv::mpv_render_context_create(&mut rctx, m.0, params.as_mut_ptr()) },
        "render_context_create",
    )?;
    let wake = Arc::new(Wake {
        flag: Mutex::new(false),
        cv: Condvar::new(),
    });
    unsafe {
        mpv::mpv_render_context_set_update_callback(
            rctx,
            Some(on_update),
            Arc::as_ptr(&wake).cast_mut().cast(),
        );
    }

    let shared = Arc::new(Shared {
        quit: AtomicBool::new(false),
        status: Mutex::new(None),
        counters: Counters::default(),
        intervals_ms: Mutex::new(Vec::new()),
        first_frame: Mutex::new(None),
    });
    let ended = Arc::new(AtomicBool::new(false));

    let guard = if args.headless {
        None
    } else {
        Some(TermGuard::enter()?)
    };
    let writer = (!args.headless).then(|| Writer {
        w,
        h,
        cols,
        term_cols: ws.cols.max(1),
        rows,
        placement: args.placement,
        transient: args.transient,
        sync: args.sync,
        max_inflight: args.max_inflight.max(1),
        inflight: VecDeque::new(),
        counter: 0,
        placeholders_drawn: false,
    });

    let render_thread = {
        let (wake, shared, fmt) = (wake.clone(), shared.clone(), args.sw_format.clone());
        let ctx = RenderCtx(rctx);
        thread::Builder::new()
            .name("render".into())
            .spawn(move || render_loop(ctx, wake, shared, writer, (w, h), fmt))?
    };
    let event_thread = {
        let (m, shared, ended) = (m.clone(), shared.clone(), ended.clone());
        thread::Builder::new()
            .name("events".into())
            .spawn(move || event_loop(m, shared, ended))?
    };

    m.command(&["loadfile", &streams.video_url])?;

    // Main loop: keys and stats once a second.
    let term_proc = find_terminal();
    logf!("terminal process: {term_proc:?}");
    let t_start = Instant::now();
    let cpu0 = cpu_self_secs();
    let term0 = term_proc
        .as_ref()
        .and_then(|(p, _)| proc_stat(*p))
        .map(|s| s.2);
    let mut last = (Instant::now(), cpu0, term0, 0u64, 0u64, 0u64);
    let mut samples: Vec<(f64, f64, f64)> = Vec::new(); // (fps, self cores, terminal cores)
    let mut max_abs_avsync: f64 = 0.0;
    let mut keybuf = [0u8; 16];

    loop {
        if ended.load(Ordering::Relaxed) {
            break;
        }
        if let (Some(secs), Some(ff)) = (args.seconds, *shared.first_frame.lock().unwrap()) {
            if ff.elapsed().as_secs_f64() >= secs {
                break;
            }
        }

        // Keys (only with a terminal).
        if guard.is_some() {
            let mut pfd = libc::pollfd {
                fd: libc::STDIN_FILENO,
                events: libc::POLLIN,
                revents: 0,
            };
            if unsafe { libc::poll(&mut pfd, 1, 100) } > 0 {
                let n = std::io::stdin().read(&mut keybuf).unwrap_or(0);
                match &keybuf[..n] {
                    b"q" | b"\x03" => break,
                    b" " => {
                        let _ = m.command(&["cycle", "pause"]);
                    }
                    b"\x1b[C" => {
                        let _ = m.command(&["seek", "5"]);
                    }
                    b"\x1b[D" => {
                        let _ = m.command(&["seek", "-5"]);
                    }
                    _ => {}
                }
            }
        } else {
            thread::sleep(Duration::from_millis(100));
        }

        let now = Instant::now();
        let dt = (now - last.0).as_secs_f64();
        if dt < 1.0 {
            continue;
        }
        let c = &shared.counters;
        let (r, p, d) = (
            c.rendered.load(Ordering::Relaxed),
            c.presented.load(Ordering::Relaxed),
            c.dropped_backpressure.load(Ordering::Relaxed),
        );
        let cpu = cpu_self_secs();
        let term = term_proc
            .as_ref()
            .and_then(|(pid, _)| proc_stat(*pid))
            .map(|s| s.2);
        let fps = (if writer_presenting(&args) {
            p - last.4
        } else {
            r - last.3
        }) as f64
            / dt;
        let self_cores = (cpu - last.1) / dt;
        let term_cores = match (term, last.2) {
            (Some(a), Some(b)) => (a - b) / dt,
            _ => f64::NAN,
        };
        let avsync = m.get_f64("avsync").unwrap_or(f64::NAN);
        let paused = m.get("pause").as_deref() == Some("yes");
        if !paused && shared.first_frame.lock().unwrap().is_some() {
            samples.push((fps, self_cores, term_cores));
            if avsync.is_finite() {
                max_abs_avsync = max_abs_avsync.max(avsync.abs());
            }
        }
        let line = format!(
            "fps {fps:5.1} | cpu self {self_cores:4.2} term {term_cores:4.2} cores | avsync {:+6.1}ms | drop mpv {} dec {} bp {d} | {}x{} hwdec={} | t={}",
            avsync * 1000.0,
            m.get("frame-drop-count").unwrap_or_default(),
            m.get("decoder-frame-drop-count").unwrap_or_default(),
            w,
            h,
            m.get("hwdec-current").unwrap_or_default(),
            m.get("time-pos/full")
                .or_else(|| m.get("time-pos"))
                .unwrap_or_default(),
        );
        logf!("{line}");
        if guard.is_some() {
            *shared.status.lock().unwrap() = Some(line);
        } else {
            eprintln!("{line}");
        }
        last = (now, cpu, term, r, p, d);
    }

    // Shutdown order: stop the render thread (it frees the render context),
    // then the event thread, then the mpv core.
    let total_wall = t_start.elapsed().as_secs_f64();
    let mpv_drops = m.get("frame-drop-count").unwrap_or_default();
    let dec_drops = m.get("decoder-frame-drop-count").unwrap_or_default();
    let vfps = m.get("container-fps").unwrap_or_default();
    let hwdec = m.get("hwdec-current").unwrap_or_default();
    shared.quit.store(true, Ordering::Relaxed);
    wake.cv.notify_one();
    let _ = render_thread.join();
    unsafe { mpv::mpv_wakeup(m.0) };
    let _ = event_thread.join();
    drop(guard);
    unsafe { mpv::mpv_terminate_destroy(m.0) };

    // Summary.
    let mut iv = shared.intervals_ms.lock().unwrap().clone();
    iv.sort_by(f32::total_cmp);
    let avg = |f: fn(&(f64, f64, f64)) -> f64| {
        let v: Vec<f64> = samples.iter().map(f).filter(|x| x.is_finite()).collect();
        if v.is_empty() {
            f64::NAN
        } else {
            v.iter().sum::<f64>() / v.len() as f64
        }
    };
    let c = &shared.counters;
    let rendered = c.rendered.load(Ordering::Relaxed).max(1);
    let mut summary = String::new();
    let _ = writeln!(summary, "=== M0 summary ===");
    let _ = writeln!(summary, "stream:       {}", streams.summary);
    let _ = writeln!(
        summary,
        "terminal:     {} (TERM_PROGRAM={})",
        term_proc.map(|t| t.1).unwrap_or_else(|| "?".into()),
        std::env::var("TERM_PROGRAM").unwrap_or_default()
    );
    let _ = writeln!(
        summary,
        "render size:  {w}x{h} ({cols}x{rows} cells), placement {:?}, sync {}, N=1 {}",
        args.placement, args.sync, args.transient
    );
    let _ = writeln!(summary, "hwdec:        {hwdec}, container fps {vfps}");
    let _ = writeln!(
        summary,
        "wall:         {total_wall:.1}s, {} samples",
        samples.len()
    );
    let _ = writeln!(summary, "fps:          avg {:.1}", avg(|s| s.0));
    let _ = writeln!(
        summary,
        "cpu (cores):  self avg {:.2}, terminal avg {:.2}",
        avg(|s| s.1),
        avg(|s| s.2)
    );
    let _ = writeln!(
        summary,
        "frame gap ms: p50 {:.1} p95 {:.1} p99 {:.1} max {:.1}",
        percentile(&iv, 0.5),
        percentile(&iv, 0.95),
        percentile(&iv, 0.99),
        iv.last().copied().unwrap_or(0.0)
    );
    let _ = writeln!(
        summary,
        "avsync:       max |avsync| {:.1}ms",
        max_abs_avsync * 1000.0
    );
    let _ = writeln!(
        summary,
        "frames:       rendered {} presented {} dropped(backpressure) {} | mpv drops {mpv_drops} decoder drops {dec_drops}",
        c.rendered.load(Ordering::Relaxed),
        c.presented.load(Ordering::Relaxed),
        c.dropped_backpressure.load(Ordering::Relaxed),
    );
    let _ = writeln!(
        summary,
        "per frame:    render call incl. pacing wait {:.2}ms, shm+write {:.2}ms",
        c.render_ns.load(Ordering::Relaxed) as f64 / rendered as f64 / 1e6,
        c.present_ns.load(Ordering::Relaxed) as f64 / rendered as f64 / 1e6,
    );
    print!("{summary}");
    log_line(&summary);
    Ok(())
}

fn writer_presenting(args: &Args) -> bool {
    !args.headless
}
