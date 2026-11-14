//! Thumbnails: pick a size, download it, keep it in a disk cache, and fit it
//! into the cells beside a list item.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, Result, bail};
use image::imageops::FilterType;
use image::{ImageReader, Limits, RgbaImage};
use tokio::sync::Semaphore;

use crate::source::innertube::client::{USER_AGENT, error_chain};
use crate::source::model::Thumbnail;

/// The cache stops at this size. Startup removes the oldest files past it.
pub const MAX_CACHE_BYTES: u64 = 100 * 1024 * 1024;
/// Downloads that run at the same time.
const DOWNLOADS: usize = 6;
/// A thumbnail is some tens of KB. Anything this big is not one.
const MAX_DOWNLOAD_BYTES: usize = 4 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(15);
/// YouTube serves thumbnails and avatars from these hosts and their subdomains.
const HOSTS: &[&str] = &["ytimg.com", "ggpht.com", "googleusercontent.com"];

/// Where a thumbnail goes on screen: its size in cells and in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub cols: u16,
    pub rows: u16,
    pub width: u32,
    pub height: u32,
}

impl Slot {
    /// A 16:9 slot `rows` cells high, for cells of `cell` pixels (width, height).
    pub fn new(rows: u16, cell: (u16, u16)) -> Self {
        let (cw, ch) = (u32::from(cell.0.max(1)), u32::from(cell.1.max(1)));
        let height = u32::from(rows) * ch;
        let cols = (height * 16).div_ceil(9 * cw).clamp(4, 40);
        let cols = u16::try_from(cols).unwrap_or(40);
        Self { cols, rows, width: u32::from(cols) * cw, height }
    }
}

/// RGBA pixels that fill a slot exactly. The image is centered, and the
/// space around it is transparent.
#[derive(Clone, PartialEq, Eq)]
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub cols: u16,
    pub rows: u16,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Picture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Picture({}x{} in {}x{} cells)", self.width, self.height, self.cols, self.rows)
    }
}

/// The URL of the smallest thumbnail at least `width` pixels wide, or of the
/// largest one if none is wide enough. Thumbnails of unknown size come last.
pub fn pick(thumbnails: &[Thumbnail], width: u32) -> Option<&str> {
    thumbnails
        .iter()
        .filter(|t| allowed(&t.url))
        .min_by_key(|t| if t.width >= width { (0, t.width) } else { (1, u32::MAX - t.width) })
        .map(|t| t.url.as_str())
}

/// Only `https:` URLs on YouTube's image hosts are downloaded.
/// The URL is parsed the same way reqwest parses it, so the host checked is
/// the host contacted.
pub fn allowed(url: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(url) else { return false };
    if url.scheme() != "https" || url.port().is_some() || !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    // Lowercase and IDNA-normalized. An IP address never matches a host name.
    let Some(host) = url.host_str() else { return false };
    HOSTS.iter().any(|h| host == *h || host.strip_suffix(h).is_some_and(|sub| sub.ends_with('.')))
}

/// Decodes an image file and fits it into `slot`.
pub fn fit(bytes: &[u8], slot: Slot) -> Result<Picture> {
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().context("decode")?;
    let fitted = image.resize(slot.width, slot.height, FilterType::Triangle).to_rgba8();
    let mut canvas = RgbaImage::new(slot.width, slot.height);
    let x = (slot.width - fitted.width().min(slot.width)) / 2;
    let y = (slot.height - fitted.height().min(slot.height)) / 2;
    image::imageops::overlay(&mut canvas, &fitted, i64::from(x), i64::from(y));
    Ok(Picture { width: slot.width, height: slot.height, cols: slot.cols, rows: slot.rows, rgba: canvas.into_raw() })
}

/// The main color of an RGBA image, or `None` if it is mostly grey, black,
/// or white. Colors are grouped by hue, each weighted by how colorful it
/// is, and the result is the average of the largest group.
pub fn main_color(rgba: &[u8]) -> Option<(u8, u8, u8)> {
    /// Less colorful pixels count as grey.
    const MIN_CHROMA: u8 = 48;
    const BUCKETS: usize = 12;
    let mut weight = [0u64; BUCKETS];
    let mut sums = [[0u64; 3]; BUCKETS];
    let mut pixels = 0u64;
    for &[red, green, blue, alpha] in rgba.as_chunks::<4>().0 {
        if alpha < 128 {
            continue;
        }
        pixels += 1;
        let (max, min) = (red.max(green).max(blue), red.min(green).min(blue));
        let chroma = max - min;
        if chroma < MIN_CHROMA {
            continue;
        }
        let bucket = hue(red, green, blue) * BUCKETS / 360;
        weight[bucket] += u64::from(chroma);
        for (sum, c) in sums[bucket].iter_mut().zip([red, green, blue]) {
            *sum += u64::from(chroma) * u64::from(c);
        }
    }
    let (best, &w) = weight.iter().enumerate().max_by_key(|(_, w)| **w)?;
    // At least a twentieth of the picture, at full chroma.
    if w == 0 || w * 20 < pixels * 255 / 4 {
        return None;
    }
    let avg = |i: usize| u8::try_from(sums[best][i] / w).unwrap_or(u8::MAX);
    Some((avg(0), avg(1), avg(2)))
}

/// The hue of a color in degrees, 0 to 359. Grey gives 0.
#[expect(clippy::many_single_char_names, reason = "the usual names of the hue formula")]
fn hue(r: u8, g: u8, b: u8) -> usize {
    let (r, g, b) = (i32::from(r), i32::from(g), i32::from(b));
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let c = max - min;
    if c == 0 {
        return 0;
    }
    let h = if max == r {
        60 * (g - b) / c
    } else if max == g {
        60 * (b - r) / c + 120
    } else {
        60 * (r - g) / c + 240
    };
    usize::try_from(h.rem_euclid(360)).unwrap_or(0)
}

/// The cache file for `url`. FNV-1a, because its value never changes between builds.
pub fn cache_file(dir: &Path, url: &str) -> PathBuf {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in url.bytes() {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    dir.join(format!("{hash:016x}"))
}

/// Removes the oldest files in `dir` until the total is below 80 % of `max_bytes`.
/// Returns the number of files removed.
pub fn prune(dir: &Path, max_bytes: u64) -> Result<usize> {
    let mut files = Vec::new();
    let mut total = 0;
    for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        let meta = entry.metadata()?;
        if meta.is_file() {
            total += meta.len();
            files.push((meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len(), entry.path()));
        }
    }
    if total <= max_bytes {
        return Ok(0);
    }
    files.sort();
    let target = max_bytes / 10 * 8;
    let mut removed = 0;
    for (_, len, path) in files {
        if total <= target {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total -= len;
            removed += 1;
        }
    }
    Ok(removed)
}

/// Downloads thumbnails, with a disk cache.
#[derive(Debug, Clone)]
pub struct Fetcher {
    http: reqwest::Client,
    dir: PathBuf,
    downloads: Arc<Semaphore>,
}

impl Fetcher {
    pub fn new(dir: PathBuf) -> Result<Self> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(TIMEOUT)
            .build()
            .map_err(|e| anyhow::anyhow!("cannot build the HTTP client: {}", error_chain(&e)))?;
        Ok(Self { http, dir, downloads: Arc::new(Semaphore::new(DOWNLOADS)) })
    }

    /// The thumbnail at `url`, fitted into `slot`.
    pub async fn load(&self, url: &str, slot: Slot) -> Result<Picture> {
        if !allowed(url) {
            bail!("not a YouTube image URL: {url}");
        }
        let path = cache_file(&self.dir, url);
        let cached = {
            let path = path.clone();
            tokio::task::spawn_blocking(move || read_cached(&path)).await?
        };
        let bytes = if let Some(bytes) = cached {
            bytes
        } else {
            let bytes = self.download(url).await?;
            let (dir, copy) = (self.dir.clone(), bytes.clone());
            tokio::task::spawn_blocking(move || {
                if let Err(e) = write_cached(&dir, &path, &copy) {
                    tracing::warn!("cannot cache a thumbnail: {e:#}");
                }
            });
            bytes
        };
        tokio::task::spawn_blocking(move || fit(&bytes, slot)).await?
    }

    async fn download(&self, url: &str) -> Result<Vec<u8>> {
        let _permit = self.downloads.acquire().await?;
        let network = |e: reqwest::Error| anyhow::anyhow!("{}", error_chain(&e));
        let mut response =
            self.http.get(url).send().await.and_then(reqwest::Response::error_for_status).map_err(network)?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(network)? {
            bytes.extend_from_slice(&chunk);
            if bytes.len() > MAX_DOWNLOAD_BYTES {
                bail!("thumbnail bigger than {MAX_DOWNLOAD_BYTES} bytes");
            }
        }
        Ok(bytes)
    }
}

/// Reads a cache file and marks it as used, so that pruning keeps it longer.
fn read_cached(path: &Path) -> Option<Vec<u8>> {
    let bytes = std::fs::read(path).ok()?;
    if let Ok(f) = std::fs::File::options().write(true).open(path) {
        let _ = f.set_modified(SystemTime::now());
    }
    Some(bytes)
}

/// Writes to a temporary file first, so that a reader never sees half a file.
fn write_cached(dir: &Path, path: &Path, bytes: &[u8]) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("rename to {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thumb(url: &str, width: u32) -> Thumbnail {
        Thumbnail { url: url.into(), width, height: width * 9 / 16 }
    }

    #[test]
    fn slot_is_16_by_9() {
        assert_eq!(Slot::new(3, (10, 20)), Slot { cols: 11, rows: 3, width: 110, height: 60 });
        assert_eq!(Slot::new(3, (0, 0)).cols, 6);
        assert_eq!(Slot::new(3, (1, 100)).cols, 40);
    }

    #[test]
    fn picks_the_smallest_wide_enough() {
        let a = "https://i.ytimg.com/a.jpg";
        let b = "https://i.ytimg.com/b.jpg";
        let c = "https://i.ytimg.com/c.jpg";
        let thumbs = [thumb(c, 336), thumb(a, 168), thumb(b, 246)];
        assert_eq!(pick(&thumbs, 200), Some(b));
        assert_eq!(pick(&thumbs, 100), Some(a));
        assert_eq!(pick(&thumbs, 400), Some(c));
        assert_eq!(pick(&[thumb(a, 0), thumb(b, 120)], 200), Some(b));
        assert_eq!(pick(&[thumb("https://evil.example/x.jpg", 400)], 200), None);
        assert_eq!(pick(&[], 200), None);
    }

    #[test]
    fn allows_only_youtube_image_hosts() {
        assert!(allowed("https://i.ytimg.com/vi/x/hq.jpg?sqp=1"));
        assert!(allowed("https://yt3.ggpht.com/abc=s88"));
        assert!(allowed("https://yt3.googleusercontent.com/abc"));
        assert!(!allowed("http://i.ytimg.com/vi/x.jpg"));
        assert!(!allowed("https://notytimg.com/x.jpg"));
        assert!(!allowed("https://ytimg.com.evil.example/x.jpg"));
        assert!(!allowed("https://i.ytimg.com:8443/x.jpg"));
        assert!(!allowed("https://user@i.ytimg.com/x.jpg"));
        // The url crate ends the host at a backslash, so this goes to evil.example.
        assert!(!allowed("https://evil.example\\.ytimg.com/x.jpg"));
        assert!(allowed("https://I.YTIMG.COM/x.jpg"), "hosts are compared in lowercase");
    }

    #[test]
    fn fits_and_centers() {
        let mut png = Vec::new();
        RgbaImage::from_pixel(40, 40, image::Rgba([255, 0, 0, 255]))
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let slot = Slot { cols: 4, rows: 2, width: 40, height: 20 };
        let p = fit(&png, slot).unwrap();
        assert_eq!((p.width, p.height, p.rgba.len()), (40, 20, 40 * 20 * 4));
        let px = |x: usize, y: usize| &p.rgba[(y * 40 + x) * 4..][..4];
        assert_eq!(px(20, 10), [255, 0, 0, 255]);
        assert_eq!(px(0, 10)[3], 0, "the sides are transparent");
        assert!(fit(b"not an image", slot).is_err());
    }

    #[test]
    fn finds_the_main_color() {
        let image = |pixels: &[([u8; 4], usize)]| {
            pixels.iter().flat_map(|(px, n)| std::iter::repeat_n(*px, *n)).flatten().collect::<Vec<u8>>()
        };
        // Mostly dark grey with a red sign: red wins, because grey does not count.
        let red = image(&[([30, 30, 30, 255], 700), ([220, 30, 40, 255], 200), ([40, 60, 200, 255], 100)]);
        assert_eq!(main_color(&red), Some((220, 30, 40)));
        // Shades of one hue are averaged, weighted by how colorful they are.
        let greens = image(&[([0, 200, 0, 255], 100), ([100, 200, 100, 255], 100)]);
        assert_eq!(main_color(&greens), Some((33, 200, 33)));
        // Grey, black, white, or a little color in a big grey picture: none.
        assert_eq!(main_color(&image(&[([128, 128, 128, 255], 100), ([255, 255, 255, 255], 100)])), None);
        assert_eq!(main_color(&image(&[([20, 20, 20, 255], 990), ([255, 0, 0, 255], 10)])), None);
        // Transparent sides do not count.
        let sides = image(&[([0, 0, 0, 0], 900), ([0, 120, 255, 255], 100)]);
        assert_eq!(main_color(&sides), Some((0, 120, 255)));
        assert_eq!(main_color(&[]), None);
    }

    #[test]
    fn hues() {
        assert_eq!(hue(255, 0, 0), 0);
        assert_eq!(hue(255, 255, 0), 60);
        assert_eq!(hue(0, 255, 0), 120);
        assert_eq!(hue(0, 0, 255), 240);
        assert_eq!(hue(255, 0, 128), 330);
        assert_eq!(hue(9, 9, 9), 0);
    }

    #[test]
    fn cache_names_are_stable() {
        let dir = Path::new("/c");
        assert_eq!(cache_file(dir, ""), Path::new("/c/cbf29ce484222325"));
        assert_ne!(cache_file(dir, "a"), cache_file(dir, "b"));
    }

    #[test]
    fn prunes_the_oldest() {
        let dir = tempfile::tempdir().unwrap();
        for (i, name) in ["old", "mid", "new"].iter().enumerate() {
            let path = dir.path().join(name);
            std::fs::write(&path, [0; 100]).unwrap();
            let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000 + i as u64);
            std::fs::File::options().write(true).open(&path).unwrap().set_modified(t).unwrap();
        }
        assert_eq!(prune(dir.path(), 300).unwrap(), 0);
        assert_eq!(prune(dir.path(), 200).unwrap(), 2);
        assert!(dir.path().join("new").exists());
        assert!(!dir.path().join("mid").exists());
    }

    /// Downloads real JPEG and WebP thumbnails. Run with `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore = "needs the network"]
    async fn live_download_and_cache() {
        let dir = tempfile::tempdir().unwrap();
        let fetcher = Fetcher::new(dir.path().to_path_buf()).unwrap();
        let slot = Slot::new(3, (10, 20));
        for url in [
            "https://i.ytimg.com/vi/dQw4w9WgXcQ/mqdefault.jpg",
            "https://i.ytimg.com/vi_webp/dQw4w9WgXcQ/mqdefault.webp",
        ] {
            let p = fetcher.load(url, slot).await.unwrap();
            assert_eq!((p.width, p.height), (slot.width, slot.height));
            assert!(p.rgba.chunks(4).any(|px| px[3] == 255), "{url} has pixels");
            // The cache write runs in the background.
            for _ in 0..50 {
                if cache_file(dir.path(), url).exists() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert!(cache_file(dir.path(), url).exists(), "{url} is cached");
        }
    }

    #[test]
    fn writes_the_cache_in_one_step() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("thumbnails");
        let path = cache_file(&sub, "https://i.ytimg.com/a.jpg");
        write_cached(&sub, &path, b"abc").unwrap();
        assert_eq!(read_cached(&path).unwrap(), b"abc");
        assert_eq!(std::fs::read_dir(&sub).unwrap().count(), 1);
    }
}
