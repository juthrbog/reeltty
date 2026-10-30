//! A safe wrapper around the libmpv client and software render APIs.
//!
//! The rules from mpv's headers that the types enforce:
//! - Only one thread waits for events, so `Events` takes `&mut self`.
//! - Only one render call runs at a time, so `Renderer` takes `&mut self`.
//! - The render context is freed before the core, because `Renderer` holds a
//!   reference to the core.
//! - The update callback calls no mpv function.

use std::cell::Cell;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::marker::PhantomData;
use std::ptr::{self, NonNull};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use super::ffi;

#[derive(Debug, thiserror::Error)]
#[error("{what}: {message}")]
pub struct Error {
    pub what: String,
    pub code: i32,
    pub message: String,
}

pub type Result<T> = std::result::Result<T, Error>;

/// The text of an mpv error code.
pub fn error_string(code: i32) -> String {
    // SAFETY: mpv returns a static string for every code.
    unsafe { CStr::from_ptr(ffi::mpv_error_string(code)) }.to_string_lossy().into_owned()
}

fn check(code: c_int, what: impl FnOnce() -> String) -> Result<c_int> {
    if code >= 0 { Ok(code) } else { Err(Error { what: what(), code, message: error_string(code) }) }
}

fn cstring(s: &str) -> Result<CString> {
    CString::new(s).map_err(|_| Error { what: format!("{s:?}"), code: -4, message: "contains a NUL byte".into() })
}

/// # Safety
/// `p` must be null or point to a NUL-terminated string.
unsafe fn string(p: *const c_char) -> String {
    if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
}

/// The mpv core. Destroying it waits for mpv to stop.
struct Core(NonNull<ffi::mpv_handle>);

// SAFETY: the client API is thread-safe (client.h: "fully thread-safe").
unsafe impl Send for Core {}
// SAFETY: as above.
unsafe impl Sync for Core {}

impl Drop for Core {
    fn drop(&mut self) {
        // SAFETY: this is the last reference. `Events` and `Renderer` hold an
        // `Arc<Core>`, so no other call can run now.
        unsafe { ffi::mpv_terminate_destroy(self.0.as_ptr()) };
    }
}

impl Core {
    fn ptr(&self) -> *mut ffi::mpv_handle {
        self.0.as_ptr()
    }
}

/// A handle to an mpv core. Clones share the core.
#[derive(Clone)]
pub struct Mpv(Arc<Core>);

impl std::fmt::Debug for Mpv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Mpv").field(&self.0.0).finish()
    }
}

/// What an observed property sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Flag,
    Double,
}

impl Mpv {
    /// Creates and starts a core with these options, and returns it with its
    /// only event loop.
    pub fn new(options: &[(&str, &str)]) -> Result<(Self, Events)> {
        // SAFETY: no arguments.
        let handle = unsafe { ffi::mpv_create() };
        let core = NonNull::new(handle).map(|h| Arc::new(Core(h))).ok_or_else(|| Error {
            what: "mpv_create".into(),
            code: -1,
            message: "out of memory".into(),
        })?;
        for (name, value) in options {
            let (n, v) = (cstring(name)?, cstring(value)?);
            // SAFETY: the core is not initialized yet, and only this thread has it.
            check(unsafe { ffi::mpv_set_option_string(core.ptr(), n.as_ptr(), v.as_ptr()) }, || {
                format!("option {name}={value}")
            })?;
        }
        // SAFETY: as above.
        check(unsafe { ffi::mpv_initialize(core.ptr()) }, || "mpv_initialize".into())?;
        let events = Events { core: Arc::clone(&core), _not_sync: PhantomData };
        Ok((Self(core), events))
    }

    fn args(args: &[&str]) -> Result<Vec<CString>> {
        args.iter().map(|a| cstring(a)).collect()
    }

    /// Runs a command and waits for it.
    pub fn command(&self, args: &[&str]) -> Result<()> {
        let owned = Self::args(args)?;
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|a| a.as_ptr()).chain([ptr::null()]).collect();
        // SAFETY: `ptrs` is a NULL-terminated array of valid strings.
        check(unsafe { ffi::mpv_command(self.0.ptr(), ptrs.as_mut_ptr()) }, || args.join(" "))?;
        Ok(())
    }

    /// Starts a command and does not wait. The result comes as
    /// `Event::CommandReply` with `id`.
    pub fn command_async(&self, id: u64, args: &[&str]) -> Result<()> {
        let owned = Self::args(args)?;
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|a| a.as_ptr()).chain([ptr::null()]).collect();
        // SAFETY: as in `command`. mpv copies the arguments.
        check(unsafe { ffi::mpv_command_async(self.0.ptr(), id, ptrs.as_mut_ptr()) }, || args.join(" "))?;
        Ok(())
    }

    /// Sends `Event::Property` with `id` when the property changes.
    pub fn observe(&self, id: u64, name: &str, format: Format) -> Result<()> {
        let n = cstring(name)?;
        let f = match format {
            Format::Flag => ffi::MPV_FORMAT_FLAG,
            Format::Double => ffi::MPV_FORMAT_DOUBLE,
        };
        // SAFETY: valid handle and string.
        check(unsafe { ffi::mpv_observe_property(self.0.ptr(), id, n.as_ptr(), f) }, || format!("observe {name}"))?;
        Ok(())
    }

    /// The value of a property as text, or `None` if it has no value now.
    pub fn get_string(&self, name: &str) -> Option<String> {
        let n = cstring(name).ok()?;
        // SAFETY: valid handle and string.
        let p = unsafe { ffi::mpv_get_property_string(self.0.ptr(), n.as_ptr()) };
        if p.is_null() {
            return None;
        }
        // SAFETY: mpv returned a string that we must free with `mpv_free`.
        let s = unsafe { string(p) };
        unsafe { ffi::mpv_free(p.cast()) };
        Some(s)
    }

    /// Sends log messages at `level` ("warn", "info", ...) and above as `Event::Log`.
    pub fn request_log_messages(&self, level: &str) -> Result<()> {
        let l = cstring(level)?;
        // SAFETY: valid handle and string.
        check(unsafe { ffi::mpv_request_log_messages(self.0.ptr(), l.as_ptr()) }, || "request_log_messages".into())?;
        Ok(())
    }

    /// Wakes the thread that waits in `Events::wait`.
    pub fn wakeup(&self) {
        // SAFETY: valid handle. Safe from any thread.
        unsafe { ffi::mpv_wakeup(self.0.ptr()) };
    }
}

/// Why a file stopped playing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    Eof,
    Stop,
    Quit,
    Error,
    Other(i32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    /// The property has no value now, for example `duration` before a file loads.
    None,
    Flag(bool),
    Double(f64),
}

/// An mpv event, copied out of mpv's memory.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Shutdown,
    Log { level: String, prefix: String, text: String },
    CommandReply { id: u64, error: Option<String> },
    FileLoaded,
    EndFile { reason: EndReason, error: Option<String> },
    Seek,
    PlaybackRestart,
    Property { id: u64, value: Value },
    Other(i32),
}

/// The event queue of a core. There is only one, and only one thread uses it.
pub struct Events {
    core: Arc<Core>,
    _not_sync: PhantomData<Cell<()>>,
}

impl std::fmt::Debug for Events {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Events").finish_non_exhaustive()
    }
}

impl Events {
    /// Waits up to `timeout` for the next event. `None` means the time ran
    /// out or `Mpv::wakeup` was called.
    pub fn wait(&mut self, timeout: Duration) -> Option<Event> {
        // SAFETY: only this thread waits (`&mut self`). The event stays valid
        // until the next call, and `read` copies it before that.
        let e = unsafe { &*ffi::mpv_wait_event(self.core.ptr(), timeout.as_secs_f64()) };
        // SAFETY: as above.
        unsafe { read(e) }
    }
}

/// # Safety
/// `e` must be an event that `mpv_wait_event` just returned.
unsafe fn read(e: &ffi::mpv_event) -> Option<Event> {
    let error = (e.error < 0).then(|| error_string(e.error));
    Some(match e.event_id {
        ffi::MPV_EVENT_NONE => return None,
        ffi::MPV_EVENT_SHUTDOWN => Event::Shutdown,
        ffi::MPV_EVENT_LOG_MESSAGE => {
            // SAFETY: this event carries an `mpv_event_log_message`.
            let m = unsafe { &*e.data.cast::<ffi::mpv_event_log_message>() };
            // SAFETY: mpv gives valid strings.
            unsafe {
                Event::Log {
                    level: string(m.level),
                    prefix: string(m.prefix),
                    text: string(m.text).trim_end().to_string(),
                }
            }
        }
        ffi::MPV_EVENT_COMMAND_REPLY => Event::CommandReply { id: e.reply_userdata, error },
        ffi::MPV_EVENT_FILE_LOADED => Event::FileLoaded,
        ffi::MPV_EVENT_END_FILE => {
            // SAFETY: this event carries an `mpv_event_end_file`.
            let end = unsafe { &*e.data.cast::<ffi::mpv_event_end_file>() };
            let reason = match end.reason {
                ffi::MPV_END_FILE_REASON_EOF => EndReason::Eof,
                ffi::MPV_END_FILE_REASON_STOP => EndReason::Stop,
                ffi::MPV_END_FILE_REASON_QUIT => EndReason::Quit,
                ffi::MPV_END_FILE_REASON_ERROR => EndReason::Error,
                n => EndReason::Other(n),
            };
            Event::EndFile { reason, error: (end.error < 0).then(|| error_string(end.error)) }
        }
        ffi::MPV_EVENT_SEEK => Event::Seek,
        ffi::MPV_EVENT_PLAYBACK_RESTART => Event::PlaybackRestart,
        ffi::MPV_EVENT_PROPERTY_CHANGE => {
            // SAFETY: this event carries an `mpv_event_property`.
            let p = unsafe { &*e.data.cast::<ffi::mpv_event_property>() };
            let value = match p.format {
                // SAFETY: the format says what `data` points to.
                ffi::MPV_FORMAT_FLAG => Value::Flag(unsafe { *p.data.cast::<c_int>() } != 0),
                ffi::MPV_FORMAT_DOUBLE => Value::Double(unsafe { *p.data.cast::<f64>() }),
                _ => Value::None,
            };
            Event::Property { id: e.reply_userdata, value }
        }
        n => Event::Other(n),
    })
}

/// Set by mpv's update callback. Other threads can set it too, to wake the
/// render thread.
#[derive(Debug, Default)]
pub struct Signal {
    set: Mutex<bool>,
    cv: Condvar,
}

impl Signal {
    pub fn notify(&self) {
        // Never panic here: this runs inside a C callback.
        *self.set.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.cv.notify_one();
    }

    /// Waits up to `timeout` for `notify`, and clears it. Returns whether it was set.
    pub fn wait(&self, timeout: Duration) -> bool {
        let set = self.set.lock().unwrap_or_else(PoisonError::into_inner);
        let (mut set, _) = self.cv.wait_timeout_while(set, timeout, |s| !*s).unwrap_or_else(PoisonError::into_inner);
        std::mem::take(&mut *set)
    }
}

unsafe extern "C" fn on_update(ctx: *mut c_void) {
    // SAFETY: `ctx` is the `Signal` that the `Renderer` keeps alive until
    // after `mpv_render_context_free`.
    let signal = unsafe { &*ctx.cast::<Signal>() };
    signal.notify();
}

/// The software renderer of a core. Create it before `loadfile`.
pub struct Renderer {
    ctx: NonNull<ffi::mpv_render_context>,
    signal: Arc<Signal>,
    // Keeps the core alive until the context is freed.
    _core: Arc<Core>,
    _not_sync: PhantomData<Cell<()>>,
}

// SAFETY: the render API can be used from any thread, one call at a time.
// `&mut self` on every call and `!Sync` make sure of that.
unsafe impl Send for Renderer {}

impl std::fmt::Debug for Renderer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Renderer").finish_non_exhaustive()
    }
}

impl Renderer {
    pub fn new(mpv: &Mpv) -> Result<Self> {
        let mut params = [
            ffi::mpv_render_param {
                type_: ffi::MPV_RENDER_PARAM_API_TYPE,
                data: ffi::MPV_RENDER_API_TYPE_SW.as_ptr().cast_mut().cast(),
            },
            ffi::mpv_render_param { type_: ffi::MPV_RENDER_PARAM_INVALID, data: ptr::null_mut() },
        ];
        let mut ctx = ptr::null_mut();
        // SAFETY: valid handle and a terminated parameter list.
        check(unsafe { ffi::mpv_render_context_create(&raw mut ctx, mpv.0.ptr(), params.as_mut_ptr()) }, || {
            "mpv_render_context_create".into()
        })?;
        let ctx = NonNull::new(ctx).ok_or_else(|| Error {
            what: "mpv_render_context_create".into(),
            code: -1,
            message: "no context".into(),
        })?;
        let signal = Arc::new(Signal::default());
        // SAFETY: the signal outlives the context (see `Drop`), and the
        // callback only sets it.
        unsafe {
            ffi::mpv_render_context_set_update_callback(
                ctx.as_ptr(),
                Some(on_update),
                Arc::as_ptr(&signal).cast_mut().cast(),
            );
        }
        Ok(Self { ctx, signal, _core: Arc::clone(&mpv.0), _not_sync: PhantomData })
    }

    /// The signal that mpv sets when there is something to render.
    pub fn signal(&self) -> Arc<Signal> {
        Arc::clone(&self.signal)
    }

    /// Whether a new video frame is ready.
    pub fn has_frame(&mut self) -> bool {
        // SAFETY: valid context, one call at a time.
        unsafe { ffi::mpv_render_context_update(self.ctx.as_ptr()) & ffi::MPV_RENDER_UPDATE_FRAME != 0 }
    }

    /// Renders the current frame into `frame`. mpv scales it to fit and
    /// waits until it is time to show it. Without a new frame, it renders the
    /// last one again.
    pub fn render(&mut self, frame: &mut Frame) -> Result<()> {
        let mut size = [c_int::from(frame.width), c_int::from(frame.height)];
        let mut stride = frame.stride();
        let mut params = [
            ffi::mpv_render_param { type_: ffi::MPV_RENDER_PARAM_SW_SIZE, data: size.as_mut_ptr().cast() },
            ffi::mpv_render_param { type_: ffi::MPV_RENDER_PARAM_SW_FORMAT, data: c"rgb0".as_ptr().cast_mut().cast() },
            ffi::mpv_render_param { type_: ffi::MPV_RENDER_PARAM_SW_STRIDE, data: (&raw mut stride).cast() },
            ffi::mpv_render_param {
                type_: ffi::MPV_RENDER_PARAM_SW_POINTER,
                data: frame.bytes_mut().as_mut_ptr().cast(),
            },
            ffi::mpv_render_param { type_: ffi::MPV_RENDER_PARAM_INVALID, data: ptr::null_mut() },
        ];
        // SAFETY: the buffer holds `stride * height` bytes, aligned to 64.
        check(unsafe { ffi::mpv_render_context_render(self.ctx.as_ptr(), params.as_mut_ptr()) }, || {
            "mpv_render_context_render".into()
        })?;
        Ok(())
    }

    /// Tells mpv that the frame was shown, for its timing.
    pub fn report_swap(&mut self) {
        // SAFETY: valid context, one call at a time.
        unsafe { ffi::mpv_render_context_report_swap(self.ctx.as_ptr()) };
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        // SAFETY: the core is still alive (`_core`). After this returns, mpv
        // calls the update callback no more, so the signal can go.
        unsafe { ffi::mpv_render_context_free(self.ctx.as_ptr()) };
    }
}

#[repr(C, align(64))]
#[derive(Clone, Copy)]
struct Block([u8; 64]);

/// An RGBA frame buffer aligned to 64 bytes, as mpv's software renderer
/// wants. Its width is a multiple of 16 pixels, so its rows have no padding.
pub struct Frame {
    width: u16,
    height: u16,
    blocks: Vec<Block>,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Frame({}x{})", self.width, self.height)
    }
}

impl Frame {
    /// A frame of at least 16 x 1 pixels, with the width rounded down to a
    /// multiple of 16.
    pub fn new(width: u16, height: u16) -> Self {
        let width = (width / 16).max(1) * 16;
        let height = height.max(1);
        let len = usize::from(width) * usize::from(height) * 4 / 64;
        Self { width, height, blocks: vec![Block([0; 64]); len] }
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    fn stride(&self) -> usize {
        usize::from(self.width) * 4
    }

    pub fn bytes(&self) -> &[u8] {
        // SAFETY: `Block` is 64 plain bytes with no padding.
        unsafe { std::slice::from_raw_parts(self.blocks.as_ptr().cast(), self.blocks.len() * 64) }
    }

    fn bytes_mut(&mut self) -> &mut [u8] {
        // SAFETY: as in `bytes`.
        unsafe { std::slice::from_raw_parts_mut(self.blocks.as_mut_ptr().cast(), self.blocks.len() * 64) }
    }

    /// Sets every alpha byte to 255. mpv's `rgb0` leaves it undefined, and
    /// Kitty reads it as alpha.
    pub fn set_opaque(&mut self) {
        for px in self.bytes_mut().as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_have_whole_blocks() {
        let mut f = Frame::new(170, 9);
        assert_eq!((f.width(), f.height()), (160, 9));
        assert_eq!(f.bytes().len(), 160 * 9 * 4);
        assert_eq!(f.bytes().as_ptr() as usize % 64, 0);
        f.set_opaque();
        assert!(f.bytes().chunks(4).all(|p| p == [0, 0, 0, 255]));
        assert_eq!(Frame::new(3, 0).bytes().len(), 16 * 4);
    }

    #[test]
    fn signal_wakes_once() {
        let s = Signal::default();
        assert!(!s.wait(Duration::from_millis(1)));
        s.notify();
        assert!(s.wait(Duration::from_millis(1)));
        assert!(!s.wait(Duration::from_millis(1)));
    }

    #[test]
    fn bad_options_are_errors() {
        let e = Mpv::new(&[("no-such-option", "1")]).unwrap_err();
        assert_eq!(e.what, "option no-such-option=1");
        assert_eq!(e.code, -5);
    }
}
