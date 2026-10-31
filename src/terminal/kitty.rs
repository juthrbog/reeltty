//! Kitty graphics for thumbnails. Each image goes to the terminal once,
//! through shared memory, with a virtual placement (`U=1`). The UI then shows
//! it with Unicode placeholder cells, so ratatui draws images as ordinary text.
//!
//! See <https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders>.

use std::collections::{HashSet, VecDeque};
use std::io::Write;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use base64::Engine as _;

use super::diacritics::DIACRITICS;
use super::shm;
use crate::thumbnails::Picture;

/// The character that the terminal replaces with a part of an image.
pub const PLACEHOLDER: char = '\u{10EEEE}';

/// The largest image ID. The ID rides in a 24-bit foreground color.
pub const MAX_ID: u32 = 0xFF_FFFF;

/// The image ID of the video. Thumbnails use IDs from 256.
pub const VIDEO_ID: u32 = 1;

/// The most rows or columns of an image that placeholders can address.
pub const MAX_CELLS: u16 = 297;

/// Sends the RGBA pixels in the shared memory object `shm_name` as image `id`,
/// with a virtual placement of `cols` x `rows` cells. `q=2` stops all replies,
/// so nothing comes back on stdin.
pub fn transmit(id: u32, shm_name: &str, picture: &Picture) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(shm_name);
    let Picture { width, height, cols, rows, .. } = picture;
    format!("\x1b_Ga=T,U=1,q=2,f=32,t=s,i={id},s={width},v={height},c={cols},r={rows};{b64}\x1b\\")
}

/// Sends one video frame of `width` x `height` RGBA pixels in the shared
/// memory object `shm_name`, shown in `cols` x `rows` cells. The same image
/// ID and placement each time replace the last frame. `N=1` tells the
/// terminal that the image changes often, and synchronized output keeps the
/// terminal from painting half a frame.
pub fn video_frame(shm_name: &str, width: u16, height: u16, cols: u16, rows: u16) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(shm_name);
    format!(
        "\x1b[?2026h\x1b_Ga=T,U=1,q=2,f=32,t=s,i={VIDEO_ID},p=1,s={width},v={height},c={cols},r={rows},N=1;{b64}\x1b\\\x1b[?2026l"
    )
}

/// Deletes image `id` and frees its data in the terminal.
pub fn delete(id: u32) -> String {
    format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\")
}

/// The text of the placeholder cell at `row` and `col` of an image, or `None`
/// past the diacritics table. Every cell names its row and column, so it does
/// not matter in which order ratatui writes the cells.
pub fn cell(row: u16, col: u16) -> Option<String> {
    let r = *DIACRITICS.get(usize::from(row))?;
    let c = *DIACRITICS.get(usize::from(col))?;
    Some([PLACEHOLDER, r, c].into_iter().collect())
}

/// The foreground color that names image `id`.
pub fn id_color(id: u32) -> (u8, u8, u8) {
    let [_, r, g, b] = id.to_be_bytes();
    (r, g, b)
}

/// The images that the terminal holds.
#[derive(Debug, Default)]
pub struct Uploads {
    /// Shared with the panic hook, which must delete the images before the
    /// alternate screen goes away.
    held: Arc<Mutex<Held>>,
    counter: u64,
}

#[derive(Debug, Default)]
struct Held {
    sent: HashSet<u32>,
    /// Shared memory objects that the terminal did not read yet.
    unread: Vec<String>,
}

impl Held {
    fn clear(&mut self, out: &mut String) {
        for id in self.sent.drain() {
            out.push_str(&delete(id));
        }
        for name in self.unread.drain(..) {
            shm::unlink(&name);
        }
    }
}

impl Uploads {
    /// Sends the images in `wanted` that the terminal does not have, and
    /// deletes the ones it has that are not in `wanted`. The escapes go into `out`.
    pub fn sync<'a>(&mut self, wanted: impl IntoIterator<Item = (u32, &'a Picture)>, out: &mut String) {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        held.unread.retain(|name| shm::exists(name));
        let mut keep = HashSet::new();
        for (id, picture) in wanted {
            keep.insert(id);
            if held.sent.contains(&id) {
                continue;
            }
            self.counter += 1;
            let name = format!("/reeltty-{}-{}", std::process::id(), self.counter);
            match shm::write(&name, &picture.rgba) {
                Ok(()) => {
                    out.push_str(&transmit(id, &name, picture));
                    held.unread.push(name);
                    held.sent.insert(id);
                }
                Err(e) => tracing::warn!("cannot send thumbnail {id}: {e:#}"),
            }
        }
        held.sent.retain(|id| {
            let k = keep.contains(id);
            if !k {
                out.push_str(&delete(*id));
            }
            k
        });
    }

    /// Deletes every image and removes the objects that the terminal did not read.
    pub fn clear(&mut self, out: &mut String) {
        self.held.lock().unwrap_or_else(PoisonError::into_inner).clear(out);
    }

    /// A function for a panic hook that deletes every image, like `clear`,
    /// and writes the escapes to stdout. It does nothing if `sync` holds the
    /// lock, so it can never deadlock.
    pub fn panic_cleanup(&self) -> impl Fn() + Send + Sync + 'static {
        let held = Arc::clone(&self.held);
        move || {
            let Ok(mut held) = held.try_lock() else { return };
            let mut out = String::new();
            held.clear(&mut out);
            let mut stdout = std::io::stdout().lock();
            let _ = stdout.write_all(out.as_bytes()).and_then(|()| stdout.flush());
        }
    }
}

/// What `VideoSink::present` did with a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presented {
    Shown,
    /// The terminal has not read the last frames yet, or the screen does
    /// not show cells of this size yet, so this one was dropped.
    Skipped,
    /// The screen shows no video now, so this one was dropped.
    Hidden,
    /// The video was closed, or another one started.
    Closed,
}

/// What the screen shows of the video.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Drawn {
    /// The frontend has not said, so every frame goes.
    Unknown,
    Hidden,
    Cells(u16, u16),
}

/// The terminal has not read this many frames yet: drop the next one.
const MAX_UNREAD_FRAMES: usize = 2;

/// Where video frames go. The render thread sends frames, and the main
/// thread opens and closes the video. Closing deletes the image, and no frame
/// can come after that.
pub struct VideoSink {
    state: Mutex<SinkState>,
}

struct SinkState {
    out: Box<dyn Write + Send>,
    /// The session that may send frames, or `None` when closed.
    session: Option<u64>,
    /// Frames that the terminal did not read yet, with their session.
    unread: VecDeque<(u64, String)>,
    counter: u64,
    drawn: Drawn,
    /// The cells of the frame that the terminal has now.
    placed: Option<(u16, u16)>,
}

impl std::fmt::Debug for VideoSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoSink").finish_non_exhaustive()
    }
}

impl VideoSink {
    /// Writes the escapes to `out`, which must be the terminal (`stdout`)
    /// outside tests.
    pub fn new(out: Box<dyn Write + Send>) -> Self {
        Self {
            state: Mutex::new(SinkState {
                out,
                session: None,
                unread: VecDeque::new(),
                counter: 0,
                drawn: Drawn::Unknown,
                placed: None,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, SinkState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Lets `session` send frames.
    pub fn open(&self, session: u64) {
        self.lock().session = Some(session);
    }

    /// Says which video cells the screen shows, before the frontend draws
    /// them. A frame for other cells waits: in the wrong grid of
    /// placeholders, it shows cropped or stretched. When the screen shows
    /// another size, the frame that the terminal has is deleted for the same
    /// reason, and the screen is blank until the next frame.
    pub fn set_drawn(&self, cells: Option<(u16, u16)>) {
        let mut s = self.lock();
        s.drawn = cells.map_or(Drawn::Hidden, |(cols, rows)| Drawn::Cells(cols, rows));
        if cells.is_some() && s.placed.is_some_and(|placed| Some(placed) != cells) {
            s.placed = None;
            let delete = delete(VIDEO_ID);
            if let Err(e) = s.out.write_all(delete.as_bytes()).and_then(|()| s.out.flush()) {
                tracing::warn!("cannot delete the video image: {e}");
            }
        }
    }

    /// Sends a frame of `session`, if it is still open, the screen shows
    /// video cells of its size, and the terminal keeps up.
    pub fn present(&self, session: u64, rgba: &[u8], size: (u16, u16), cells: (u16, u16)) -> anyhow::Result<Presented> {
        let mut s = self.lock();
        if s.session != Some(session) {
            return Ok(Presented::Closed);
        }
        match s.drawn {
            Drawn::Hidden => return Ok(Presented::Hidden),
            Drawn::Cells(cols, rows) if (cols, rows) != cells => return Ok(Presented::Skipped),
            Drawn::Cells(..) | Drawn::Unknown => {}
        }
        s.unread.retain(|(_, name)| shm::exists(name));
        if s.unread.iter().filter(|(id, _)| *id == session).count() >= MAX_UNREAD_FRAMES {
            return Ok(Presented::Skipped);
        }
        s.counter += 1;
        let name = format!("/reeltty-v-{}-{session}-{}", std::process::id(), s.counter);
        shm::write(&name, rgba)?;
        s.unread.push_back((session, name.clone()));
        let escape = video_frame(&name, size.0, size.1, cells.0, cells.1);
        s.out.write_all(escape.as_bytes()).and_then(|()| s.out.flush())?;
        s.placed = Some(cells);
        Ok(Presented::Shown)
    }

    /// Stops all frames and deletes the video image.
    pub fn close(&self) {
        let mut s = self.lock();
        s.placed = None;
        if s.session.take().is_some() {
            let delete = delete(VIDEO_ID);
            if let Err(e) = s.out.write_all(delete.as_bytes()).and_then(|()| s.out.flush()) {
                tracing::warn!("cannot delete the video image: {e}");
            }
        }
    }

    /// Waits up to `wait` for the terminal to read the frames of `session`,
    /// then removes the ones it did not read.
    pub fn reap(&self, session: u64, wait: Duration) {
        let end = Instant::now() + wait;
        loop {
            let mut s = self.lock();
            s.unread.retain(|(_, name)| shm::exists(name));
            if !s.unread.iter().any(|(id, _)| *id == session) {
                return;
            }
            if Instant::now() >= end {
                s.unread.retain(|(id, name)| {
                    if *id == session {
                        shm::unlink(name);
                    }
                    *id != session
                });
                return;
            }
            drop(s);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// A function for a panic hook, like `Uploads::panic_cleanup`: it
    /// deletes the video image and removes the unread frames. It does
    /// nothing if another thread holds the lock, so it can never deadlock.
    pub fn panic_cleanup(self: &Arc<Self>) -> impl Fn() + Send + Sync + 'static {
        let sink = Arc::downgrade(self);
        move || {
            let Some(sink) = sink.upgrade() else { return };
            let Ok(mut s) = sink.state.try_lock() else { return };
            if s.session.take().is_some() {
                let delete = delete(VIDEO_ID);
                let _ = s.out.write_all(delete.as_bytes()).and_then(|()| s.out.flush());
            }
            for (_, name) in s.unread.drain(..) {
                shm::unlink(&name);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture() -> Picture {
        Picture { width: 2, height: 1, cols: 2, rows: 1, rgba: vec![1; 8] }
    }

    #[test]
    fn escapes() {
        assert_eq!(transmit(300, "/x", &picture()), "\x1b_Ga=T,U=1,q=2,f=32,t=s,i=300,s=2,v=1,c=2,r=1;L3g=\x1b\\");
        assert_eq!(delete(7), "\x1b_Ga=d,d=I,i=7,q=2\x1b\\");
        assert_eq!(id_color(0x01_02_03), (1, 2, 3));
        assert_eq!(cell(0, 1).unwrap(), "\u{10EEEE}\u{305}\u{30D}");
        assert_eq!(cell(0, 297), None);
        assert_eq!(DIACRITICS.len(), usize::from(MAX_CELLS));
    }

    /// A writer that tests can read.
    #[derive(Clone, Default)]
    struct Shared(Arc<Mutex<Vec<u8>>>);

    impl Write for Shared {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Shared {
        fn take(&self) -> String {
            String::from_utf8(std::mem::take(&mut *self.0.lock().unwrap())).unwrap()
        }
    }

    #[test]
    fn video_frame_escape() {
        assert_eq!(
            video_frame("/x", 32, 18, 4, 2),
            "\x1b[?2026h\x1b_Ga=T,U=1,q=2,f=32,t=s,i=1,p=1,s=32,v=18,c=4,r=2,N=1;L3g=\x1b\\\x1b[?2026l"
        );
    }

    #[test]
    fn video_sink_drops_frames_the_terminal_did_not_read() {
        let out = Shared::default();
        let sink = Arc::new(VideoSink::new(Box::new(out.clone())));
        let rgba = [0u8; 16];
        let present = |session| sink.present(session, &rgba, (2, 2), (1, 1)).unwrap();
        assert_eq!(present(5), Presented::Closed);
        sink.open(5);
        assert_eq!(present(5), Presented::Shown);
        assert_eq!(present(5), Presented::Shown);
        // Nobody reads the objects in this test.
        assert_eq!(present(5), Presented::Skipped);
        assert_eq!(out.take().matches("a=T").count(), 2);
        assert_eq!(present(4), Presented::Closed);

        let names: Vec<String> = sink.lock().unread.iter().map(|(_, n)| n.clone()).collect();
        // The terminal reads one.
        shm::unlink(&names[0]);
        assert_eq!(present(5), Presented::Shown);

        sink.close();
        assert_eq!(out.take(), format!("{}{}", video_frame(&sink.lock().unread[1].1, 2, 2, 1, 1), delete(VIDEO_ID)));
        assert_eq!(present(5), Presented::Closed);
        sink.close();
        assert_eq!(out.take(), "", "close deletes once");
        sink.reap(5, Duration::from_millis(20));
        assert!(sink.lock().unread.is_empty());
        assert!(names.iter().all(|n| !shm::exists(n)));
    }

    #[test]
    fn video_sink_sends_only_frames_for_the_cells_on_screen() {
        let out = Shared::default();
        let sink = Arc::new(VideoSink::new(Box::new(out.clone())));
        let present = |cells| sink.present(1, &[0; 4], (1, 1), cells).unwrap();
        let read = |sink: &VideoSink| {
            for (_, name) in sink.lock().unread.drain(..) {
                shm::unlink(&name);
            }
        };
        sink.open(1);
        sink.set_drawn(Some((4, 2)));
        assert_eq!(present((4, 2)), Presented::Shown);
        read(&sink);
        out.take();
        // The app shrinks the video, and the render thread is first.
        assert_eq!(present((2, 1)), Presented::Skipped);
        assert_eq!(out.take(), "");
        // The screen draws the small cells: the big frame goes.
        sink.set_drawn(Some((2, 1)));
        assert_eq!(out.take(), delete(VIDEO_ID));
        assert_eq!(present((4, 2)), Presented::Skipped, "a frame of the old size");
        assert_eq!(present((2, 1)), Presented::Shown);
        read(&sink);
        // The same size again deletes nothing.
        out.take();
        sink.set_drawn(Some((2, 1)));
        assert_eq!(out.take(), "");
        // No video on screen.
        sink.set_drawn(None);
        assert_eq!(present((2, 1)), Presented::Hidden);
        assert_eq!(out.take(), "");
        sink.close();
    }

    #[test]
    fn video_sink_panic_cleanup() {
        let out = Shared::default();
        let sink = Arc::new(VideoSink::new(Box::new(out.clone())));
        sink.open(1);
        sink.present(1, &[0; 4], (1, 1), (1, 1)).unwrap();
        let name = sink.lock().unread[0].1.clone();
        out.take();
        sink.panic_cleanup()();
        assert_eq!(out.take(), delete(VIDEO_ID));
        assert!(!shm::exists(&name));
    }

    #[test]
    fn sends_each_image_once_and_deletes_old_ones() {
        let p = picture();
        let mut u = Uploads::default();
        let mut out = String::new();
        u.sync([(300, &p), (301, &p)], &mut out);
        assert_eq!(out.matches("a=T").count(), 2);
        let names = u.held.lock().unwrap().unread.clone();
        out.clear();
        u.sync([(301, &p)], &mut out);
        assert_eq!(out, delete(300));
        out.clear();
        u.clear(&mut out);
        assert_eq!(out, delete(301));
        // No terminal read the objects, so clear removed them.
        assert!(names.iter().all(|n| !shm::exists(n)));
    }
}
