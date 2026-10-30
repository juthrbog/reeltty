//! Kitty graphics for thumbnails. Each image goes to the terminal once,
//! through shared memory, with a virtual placement (`U=1`). The UI then shows
//! it with Unicode placeholder cells, so ratatui draws images as ordinary text.
//!
//! See <https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders>.

use std::collections::HashSet;

use base64::Engine as _;

use super::diacritics::DIACRITICS;
use super::shm;
use crate::thumbnails::Picture;

/// The character that the terminal replaces with a part of an image.
pub const PLACEHOLDER: char = '\u{10EEEE}';

/// The largest image ID. The ID rides in a 24-bit foreground color.
pub const MAX_ID: u32 = 0xFF_FFFF;

/// Sends the RGBA pixels in the shared memory object `shm_name` as image `id`,
/// with a virtual placement of `cols` x `rows` cells. `q=2` stops all replies,
/// so nothing comes back on stdin.
pub fn transmit(id: u32, shm_name: &str, picture: &Picture) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(shm_name);
    let Picture { width, height, cols, rows, .. } = picture;
    format!("\x1b_Ga=T,U=1,q=2,f=32,t=s,i={id},s={width},v={height},c={cols},r={rows};{b64}\x1b\\")
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
    sent: HashSet<u32>,
    /// Shared memory objects that the terminal did not read yet.
    unread: Vec<String>,
    counter: u64,
}

impl Uploads {
    /// Sends the images in `wanted` that the terminal does not have, and
    /// deletes the ones it has that are not in `wanted`. The escapes go into `out`.
    pub fn sync<'a>(&mut self, wanted: impl IntoIterator<Item = (u32, &'a Picture)>, out: &mut String) {
        self.unread.retain(|name| shm::exists(name));
        let mut keep = HashSet::new();
        for (id, picture) in wanted {
            keep.insert(id);
            if self.sent.contains(&id) {
                continue;
            }
            self.counter += 1;
            let name = format!("/reeltty-{}-{}", std::process::id(), self.counter);
            match shm::write(&name, &picture.rgba) {
                Ok(()) => {
                    out.push_str(&transmit(id, &name, picture));
                    self.unread.push(name);
                    self.sent.insert(id);
                }
                Err(e) => tracing::warn!("cannot send thumbnail {id}: {e:#}"),
            }
        }
        self.sent.retain(|id| {
            let k = keep.contains(id);
            if !k {
                out.push_str(&delete(*id));
            }
            k
        });
    }

    /// Deletes every image and removes the objects that the terminal did not read.
    pub fn clear(&mut self, out: &mut String) {
        for id in self.sent.drain() {
            out.push_str(&delete(id));
        }
        for name in self.unread.drain(..) {
            shm::unlink(&name);
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
    }

    #[test]
    fn sends_each_image_once_and_deletes_old_ones() {
        let p = picture();
        let mut u = Uploads::default();
        let mut out = String::new();
        u.sync([(300, &p), (301, &p)], &mut out);
        assert_eq!(out.matches("a=T").count(), 2);
        let names = u.unread.clone();
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
