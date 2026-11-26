//! Direct I/O: files and devices opened past the system cache, so the measured speed and the read-back data
//! come from the drive itself and not from memory.
//!
//! - macOS: `F_NOCACHE` on the descriptor and `F_FULLFSYNC` at the end of a write.
//! - Windows: `FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH`. Buffers, offsets and sizes must then be
//!   multiples of the sector size: everything here works in 4 KiB-aligned blocks ([`AlignedBuf`]).

use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

/// Alignment of every buffer and every I/O size: covers 512-byte and 4K-sector drives.
pub const ALIGN: usize = 4096;

/// A zeroed buffer aligned to [`ALIGN`] (direct I/O on Windows needs it).
pub struct AlignedBuf {
    ptr: *mut u8,
    len: usize,
}

unsafe impl Send for AlignedBuf {}

impl AlignedBuf {
    pub fn new(len: usize) -> Self {
        assert!(len > 0 && len.is_multiple_of(ALIGN), "aligned buffer size must be a multiple of {ALIGN}");
        let layout = Layout::from_size_align(len, ALIGN).expect("layout");
        // SAFETY: layout has a non-zero size.
        let ptr = unsafe { alloc_zeroed(layout) };
        if ptr.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        AlignedBuf { ptr, len }
    }

    pub fn len(&self) -> usize {
        self.len
    }
}

impl std::ops::Deref for AlignedBuf {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        // SAFETY: ptr is a live allocation of len bytes.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl std::ops::DerefMut for AlignedBuf {
    fn deref_mut(&mut self) -> &mut [u8] {
        // SAFETY: ptr is a live allocation of len bytes, uniquely borrowed.
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

impl Drop for AlignedBuf {
    fn drop(&mut self) {
        // SAFETY: allocated in `new` with the same layout.
        unsafe { dealloc(self.ptr, Layout::from_size_align(self.len, ALIGN).expect("layout")) }
    }
}

#[cfg(windows)]
const FILE_FLAG_WRITE_THROUGH: u32 = 0x8000_0000;
#[cfg(windows)]
const FILE_FLAG_NO_BUFFERING: u32 = 0x2000_0000;

/// Turns the system cache off for this descriptor (macOS). Windows sets it when opening.
fn no_cache(file: &File) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: plain fcntl on an open descriptor.
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_NOCACHE, 1) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    let _ = file;
    Ok(())
}

fn options() -> OpenOptions {
    #[allow(unused_mut)]
    let mut o = OpenOptions::new();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        o.custom_flags(FILE_FLAG_NO_BUFFERING | FILE_FLAG_WRITE_THROUGH);
        // FILE_SHARE_READ | FILE_SHARE_WRITE: other programs (Explorer, antivirus) may keep the volume open.
        o.share_mode(0x1 | 0x2);
    }
    o
}

/// New file for writing past the cache (replaces an old one).
pub fn create(path: &Path) -> io::Result<File> {
    let f = options().write(true).create(true).truncate(true).open(path)?;
    no_cache(&f)?;
    Ok(f)
}

/// Existing file for reading past the cache.
pub fn open_read(path: &Path) -> io::Result<File> {
    let f = options().read(true).open(path)?;
    no_cache(&f)?;
    Ok(f)
}

/// Whole device (`/dev/rdiskN`, `\\.\PhysicalDriveN`) for reading and writing past the cache.
pub fn open_device(path: &Path) -> io::Result<File> {
    let f = options().read(true).write(true).open(path)?;
    no_cache(&f)?;
    Ok(f)
}

/// Waits until the data is really on the drive (on macOS `fsync` alone only reaches the drive's cache).
pub fn full_sync(file: &File) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: plain fcntl on an open descriptor.
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_FULLFSYNC) } != -1 {
            return Ok(());
        }
    }
    file.sync_all()
}

pub fn read_at(file: &File, buf: &mut [u8], off: u64) -> io::Result<()> {
    let mut done = 0;
    while done < buf.len() {
        #[cfg(unix)]
        let n = std::os::unix::fs::FileExt::read_at(file, &mut buf[done..], off + done as u64)?;
        #[cfg(windows)]
        let n = std::os::windows::fs::FileExt::seek_read(file, &mut buf[done..], off + done as u64)?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "end of the device"));
        }
        done += n;
    }
    Ok(())
}

pub fn write_at(file: &File, buf: &[u8], off: u64) -> io::Result<()> {
    let mut done = 0;
    while done < buf.len() {
        #[cfg(unix)]
        let n = std::os::unix::fs::FileExt::write_at(file, &buf[done..], off + done as u64)?;
        #[cfg(windows)]
        let n = std::os::windows::fs::FileExt::seek_write(file, &buf[done..], off + done as u64)?;
        if n == 0 {
            return Err(io::Error::new(io::ErrorKind::WriteZero, "the device accepted no data"));
        }
        done += n;
    }
    Ok(())
}

/// (free for the user, total) bytes of the volume that holds `path`.
pub fn space(path: &Path) -> io::Result<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(io::Error::other)?;
        // SAFETY: statvfs fills the struct; c is a valid C string.
        let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let unit = s.f_frsize as u64;
        Ok((s.f_bavail as u64 * unit, s.f_blocks as u64 * unit))
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let (mut free, mut total) = (0u64, 0u64);
        // SAFETY: wide is NUL-terminated; out pointers are valid.
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free,
                &mut total,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((free, total))
    }
}

/// Error that means "no rights / the system blocked the access", not "the drive lost the data".
pub fn is_access_error(e: &io::Error) -> bool {
    if matches!(e.kind(), io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem) {
        return true;
    }
    #[cfg(unix)]
    if let Some(code) = e.raw_os_error() {
        return [libc::EPERM, libc::EACCES, libc::EROFS, libc::EBUSY, libc::ENODEV, libc::ENXIO].contains(&code);
    }
    #[cfg(windows)]
    if let Some(code) = e.raw_os_error() {
        // ACCESS_DENIED, WRITE_PROTECT, NOT_READY, SHARING_VIOLATION, LOCK_VIOLATION, DEVICE_NOT_CONNECTED, NO_SUCH_DEVICE
        return [5, 19, 21, 32, 33, 1167, 433].contains(&code);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligned_buffer() {
        let mut b = AlignedBuf::new(ALIGN * 3);
        assert_eq!(b.as_ptr() as usize % ALIGN, 0);
        assert!(b.iter().all(|&x| x == 0));
        b[5] = 7;
        assert_eq!(b[5], 7);
        assert_eq!(b.len(), ALIGN * 3);
    }

    #[test]
    fn write_and_read_past_the_cache() {
        let dir = std::env::temp_dir().join(format!("umc_dio_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("f.bin");
        let mut buf = AlignedBuf::new(ALIGN * 4);
        for (i, x) in buf.iter_mut().enumerate() {
            *x = (i % 251) as u8;
        }
        let f = create(&path).unwrap();
        write_at(&f, &buf, 0).unwrap();
        full_sync(&f).unwrap();
        drop(f);
        let mut back = AlignedBuf::new(ALIGN * 4);
        read_at(&open_read(&path).unwrap(), &mut back, 0).unwrap();
        assert_eq!(&buf[..], &back[..]);
        let (free, total) = space(&dir).unwrap();
        assert!(total > 0 && free <= total);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
