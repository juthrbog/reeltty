//! POSIX shared memory objects, the Kitty graphics transmission medium `t=s`.

use std::ffi::CString;
use std::fs::File;
use std::io::Write as _;
use std::ptr;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result};

/// The size of a huge page. Smaller objects are written with write(2),
/// because a huge page for them would waste most of its memory.
const HUGE_PAGE: usize = 2 << 20;

/// Creates the object `name` (it must start with `/`) with `bytes` in it.
/// The terminal unlinks it after it reads it.
pub fn write(name: &str, bytes: &[u8]) -> Result<()> {
    use std::os::fd::FromRawFd as _;

    let c = CString::new(name)?;
    // SAFETY: c is a valid C string.
    let fd = unsafe { libc::shm_open(c.as_ptr(), libc::O_CREAT | libc::O_EXCL | libc::O_RDWR, 0o600) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error()).with_context(|| format!("shm_open {name}"));
    }
    // SAFETY: fd is a new descriptor that nothing else owns.
    let mut f = unsafe { File::from_raw_fd(fd) };
    let huge = bytes.len() >= HUGE_PAGE && huge_pages() && !NO_POPULATE.load(Ordering::Relaxed);
    let written = match if huge { write_huge(&f, bytes) } else { Ok(false) } {
        Ok(true) => Ok(()),
        // On Linux, write(2) sets the size, so no ftruncate or mmap is necessary.
        Ok(false) => f.write_all(bytes).map_err(Into::into),
        Err(e) => Err(e),
    };
    if let Err(e) = written {
        unlink(name);
        return Err(e).with_context(|| format!("write {name}"));
    }
    Ok(())
}

/// Writes `bytes` through a mapping that asks for huge pages. A new object
/// gets new pages, and with 4 KiB pages the kernel spends more time on them
/// than on the copy: about 6 ms for a 1440p frame, against about 1.3 ms with
/// huge pages.
///
/// Returns false, with nothing written, when the kernel is older than 5.14
/// and has no `MADV_POPULATE_WRITE`. Then the caller writes with write(2).
fn write_huge(f: &File, bytes: &[u8]) -> Result<bool> {
    use std::os::fd::AsRawFd as _;

    let len = libc::off_t::try_from(bytes.len())?;
    f.set_len(len.cast_unsigned())?;
    // SAFETY: a new shared mapping of the whole object, which is `bytes.len()`
    // bytes long. Only this function uses it, and it unmaps it before it returns.
    let ptr = unsafe { libc::mmap(ptr::null_mut(), bytes.len(), libc::PROT_WRITE, libc::MAP_SHARED, f.as_raw_fd(), 0) };
    if ptr == libc::MAP_FAILED {
        return Err(std::io::Error::last_os_error()).context("mmap");
    }
    // SAFETY: `ptr` is the mapping from above, and nothing has touched it yet.
    // Huge pages are only a hint, so that result does not matter.
    // ftruncate gives tmpfs pages no space. If /dev/shm is full, a write to
    // the mapping gets SIGBUS, which kills the process. MADV_POPULATE_WRITE
    // gets the pages first, and returns that failure as an error.
    let populated = unsafe {
        libc::madvise(ptr, bytes.len(), libc::MADV_HUGEPAGE);
        if libc::madvise(ptr, bytes.len(), libc::MADV_POPULATE_WRITE) == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    };
    if populated.is_ok() {
        // SAFETY: every page of the mapping is there, so the copy cannot
        // fault. The mapping does not overlap `bytes`.
        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.cast::<u8>(), bytes.len()) };
    }
    // SAFETY: the mapping from above, which nothing uses after this.
    unsafe { libc::munmap(ptr, bytes.len()) };
    match populated {
        Ok(()) => Ok(true),
        Err(e) if e.raw_os_error() == Some(libc::EINVAL) => {
            NO_POPULATE.store(true, Ordering::Relaxed);
            Ok(false)
        }
        Err(e) => Err(e).context("madvise MADV_POPULATE_WRITE"),
    }
}

/// Set when the kernel has no `MADV_POPULATE_WRITE`, so `write_huge` is not
/// tried again.
static NO_POPULATE: AtomicBool = AtomicBool::new(false);

/// Whether new objects in /dev/shm can get huge pages when asked. Without
/// them, a mapping is slower than write(2).
fn huge_pages() -> bool {
    static HUGE: OnceLock<bool> = OnceLock::new();
    *HUGE.get_or_init(|| {
        let mounts = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
        let shmem = std::fs::read_to_string("/sys/kernel/mm/transparent_hugepage/shmem_enabled").unwrap_or_default();
        let huge = huge_pages_from(&mounts, &shmem);
        tracing::debug!("huge pages for /dev/shm: {huge}");
        huge
    })
}

/// Whether /dev/shm gives huge pages to a mapping that asks for them, from
/// /proc/mounts and the system setting `shmem_enabled`, where brackets mark
/// the current value.
fn huge_pages_from(mounts: &str, shmem_enabled: &str) -> bool {
    if shmem_enabled.contains("[deny]") {
        return false;
    }
    if shmem_enabled.contains("[force]") {
        return true;
    }
    // The last mount on /dev/shm is the one that is used.
    let Some(options) = mounts.lines().rev().find_map(|line| {
        let mut fields = line.split_whitespace();
        let (_, dir, fs, options) = (fields.next()?, fields.next()?, fields.next()?, fields.next()?);
        (dir == "/dev/shm" && fs == "tmpfs").then_some(options)
    }) else {
        return false;
    };
    options.split(',').any(|o| matches!(o, "huge=always" | "huge=within_size" | "huge=advise"))
}

/// Removes the object `name`. It is not an error if it is already gone.
pub fn unlink(name: &str) {
    if let Ok(c) = CString::new(name) {
        // SAFETY: c is a valid C string.
        unsafe { libc::shm_unlink(c.as_ptr()) };
    }
}

/// True if the object `name` still exists, so the terminal did not read it yet.
pub fn exists(name: &str) -> bool {
    std::path::Path::new("/dev/shm").join(name.trim_start_matches('/')).exists()
}

/// An object that is unlinked on drop, unless the terminal already unlinked it.
pub struct ShmObject(String);

impl ShmObject {
    pub fn create(name: &str, bytes: &[u8]) -> Result<Self> {
        write(name, bytes)?;
        Ok(Self(name.to_string()))
    }
}

impl Drop for ShmObject {
    fn drop(&mut self) {
        unlink(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_unlinks() {
        let name = format!("/reeltty-test-{}", std::process::id());
        let obj = ShmObject::create(&name, b"abc").unwrap();
        assert!(exists(&name));
        assert_eq!(std::fs::read(format!("/dev/shm{name}")).unwrap(), b"abc");
        assert!(write(&name, b"x").is_err(), "O_EXCL refuses an object that exists");
        drop(obj);
        assert!(!exists(&name));
    }

    #[test]
    fn writes_large_objects() {
        // Larger than a huge page and not a whole number of them, so it goes
        // through `write_huge` on systems with huge pages.
        let bytes: Vec<u8> = (0..HUGE_PAGE * 3 + 4100).map(|i| u8::try_from(i % 251).unwrap()).collect();
        let name = format!("/reeltty-test-large-{}", std::process::id());
        let obj = ShmObject::create(&name, &bytes).unwrap();
        assert_eq!(std::fs::read(format!("/dev/shm{name}")).unwrap(), bytes);
        drop(obj);
        assert!(!exists(&name));
    }

    #[test]
    fn writes_large_objects_through_a_mapping() {
        use std::os::fd::FromRawFd as _;

        let bytes: Vec<u8> = (0..HUGE_PAGE + 12).map(|i| u8::try_from(i % 253).unwrap()).collect();
        let name = format!("/reeltty-test-mapped-{}", std::process::id());
        let c = CString::new(name.as_str()).unwrap();
        // SAFETY: c is a valid C string.
        let fd = unsafe { libc::shm_open(c.as_ptr(), libc::O_CREAT | libc::O_EXCL | libc::O_RDWR, 0o600) };
        assert!(fd >= 0);
        // SAFETY: fd is a new descriptor that nothing else owns.
        let f = unsafe { File::from_raw_fd(fd) };
        let written = write_huge(&f, &bytes);
        let read = std::fs::read(format!("/dev/shm{name}"));
        unlink(&name);
        // Kernels before 5.14 write nothing here, and `write` uses write(2).
        if written.unwrap() {
            assert_eq!(read.unwrap(), bytes);
        }
    }

    #[test]
    fn finds_huge_pages() {
        let mounts = |o: &str| format!("proc /proc proc rw 0 0\ntmpfs /dev/shm tmpfs {o} 0 0\n");
        let shmem = "always within_size [advise] never deny force";
        assert!(huge_pages_from(&mounts("rw,nosuid,nodev,inode64,huge=advise,usrquota"), shmem));
        assert!(huge_pages_from(&mounts("rw,huge=always"), shmem));
        assert!(huge_pages_from(&mounts("rw,huge=within_size"), shmem));
        // Without the option, tmpfs gives no huge pages.
        assert!(!huge_pages_from(&mounts("rw,nosuid,nodev"), shmem));
        assert!(!huge_pages_from(&mounts("rw,huge=never"), shmem));
        // The system setting can deny or force them on every mount.
        assert!(!huge_pages_from(&mounts("rw,huge=advise"), "always within_size advise never [deny] force"));
        assert!(huge_pages_from(&mounts("rw"), "always within_size advise never deny [force]"));
        // No /dev/shm, or one that is not tmpfs.
        assert!(!huge_pages_from("proc /proc proc rw 0 0\n", shmem));
        assert!(!huge_pages_from("none /dev/shm ramfs rw,huge=advise 0 0\n", shmem));
        // The last mount on /dev/shm counts.
        let two = format!("{}{}", mounts("rw,huge=advise"), "tmpfs /dev/shm tmpfs rw 0 0\n");
        assert!(!huge_pages_from(&two, shmem));
        // Old kernels have no `shmem_enabled`.
        assert!(huge_pages_from(&mounts("rw,huge=advise"), ""));
    }
}
