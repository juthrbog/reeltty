//! POSIX shared memory objects, the Kitty graphics transmission medium `t=s`.

use std::ffi::CString;
use std::io::Write as _;

use anyhow::{Context as _, Result};

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
    let mut f = unsafe { std::fs::File::from_raw_fd(fd) };
    // On Linux, write(2) sets the size, so no ftruncate or mmap is necessary.
    if let Err(e) = f.write_all(bytes) {
        unlink(name);
        return Err(e).with_context(|| format!("write {name}"));
    }
    Ok(())
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
}
