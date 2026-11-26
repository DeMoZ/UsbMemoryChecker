//! Whole device opened for raw access (administrator rights): `/dev/rdiskN` on macOS, `\\.\PhysicalDriveN` on
//! Windows. Used only by the elevated helper for the quick probe.

use std::fs::File;
use std::io;
use std::path::Path;

use crate::dio::{self, ALIGN, AlignedBuf};
use crate::probe::Device;

pub struct RawDevice {
    file: File,
    size: u64,
    buf: AlignedBuf,
}

impl RawDevice {
    pub fn open(path: &Path) -> io::Result<Self> {
        let file = dio::open_device(path)?;
        let size = device_size(&file)?;
        Ok(RawDevice { file, size, buf: AlignedBuf::new(1 << 20) })
    }
}

impl Device for RawDevice {
    fn size(&self) -> u64 {
        self.size
    }

    // Data goes through an aligned buffer: raw devices accept only sector-aligned memory.
    fn read(&mut self, out: &mut [u8], off: u64) -> io::Result<()> {
        let mut done = 0;
        while done < out.len() {
            let n = (out.len() - done).min(self.buf.len());
            debug_assert!(n.is_multiple_of(ALIGN));
            dio::read_at(&self.file, &mut self.buf[..n], off + done as u64)?;
            out[done..done + n].copy_from_slice(&self.buf[..n]);
            done += n;
        }
        Ok(())
    }

    fn write(&mut self, data: &[u8], off: u64) -> io::Result<()> {
        let mut done = 0;
        while done < data.len() {
            let n = (data.len() - done).min(self.buf.len());
            self.buf[..n].copy_from_slice(&data[done..done + n]);
            dio::write_at(&self.file, &self.buf[..n], off + done as u64)?;
            done += n;
        }
        Ok(())
    }
}

/// Size the device reports, asked from the device itself.
#[cfg(target_os = "macos")]
fn device_size(file: &File) -> io::Result<u64> {
    use std::os::fd::AsRawFd;
    // _IOR('d', 24, uint32_t) and _IOR('d', 25, uint64_t) from <sys/disk.h>
    const DKIOCGETBLOCKSIZE: libc::c_ulong = 0x4004_6418;
    const DKIOCGETBLOCKCOUNT: libc::c_ulong = 0x4008_6419;
    let (mut block, mut count) = (0u32, 0u64);
    // SAFETY: ioctls write one integer each into valid locations.
    unsafe {
        if libc::ioctl(file.as_raw_fd(), DKIOCGETBLOCKSIZE, &mut block) != 0
            || libc::ioctl(file.as_raw_fd(), DKIOCGETBLOCKCOUNT, &mut count) != 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(block as u64 * count)
}

#[cfg(windows)]
fn device_size(file: &File) -> io::Result<u64> {
    use std::os::windows::io::AsRawHandle;
    const IOCTL_DISK_GET_LENGTH_INFO: u32 = 0x0007_405C;
    let mut length: i64 = 0;
    let mut returned = 0u32;
    // SAFETY: the output buffer is one i64 (GET_LENGTH_INFORMATION).
    let ok = unsafe {
        windows_sys::Win32::System::IO::DeviceIoControl(
            file.as_raw_handle() as _,
            IOCTL_DISK_GET_LENGTH_INFO,
            std::ptr::null(),
            0,
            (&mut length as *mut i64).cast(),
            8,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(length as u64)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn device_size(file: &File) -> io::Result<u64> {
    use std::io::{Seek, SeekFrom};
    let mut f = file;
    f.seek(SeekFrom::End(0))
}

/// Volumes of a disk locked and dismounted for the time of the probe (Windows blocks raw writes into mounted
/// volumes). Dropping it unlocks them, and Windows mounts them again.
#[cfg(windows)]
pub struct VolumeLocks(#[allow(dead_code)] Vec<File>);

#[cfg(windows)]
impl VolumeLocks {
    pub fn take(letters: &[String]) -> Result<Self, String> {
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;
        const FSCTL_LOCK_VOLUME: u32 = 0x0009_0018;
        const FSCTL_DISMOUNT_VOLUME: u32 = 0x0009_0020;
        let ioctl = |f: &File, code: u32| {
            let mut returned = 0u32;
            // SAFETY: control codes without buffers.
            unsafe {
                windows_sys::Win32::System::IO::DeviceIoControl(
                    f.as_raw_handle() as _,
                    code,
                    std::ptr::null(),
                    0,
                    std::ptr::null_mut(),
                    0,
                    &mut returned,
                    std::ptr::null_mut(),
                ) != 0
            }
        };
        let mut held = Vec::new();
        for letter in letters {
            let path = format!(r"\\.\{letter}:");
            let f = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .share_mode(0x1 | 0x2)
                .open(&path)
                .map_err(|e| format!("could not open volume {letter}: {e}"))?;
            let mut locked = false;
            for _ in 0..10 {
                if ioctl(&f, FSCTL_LOCK_VOLUME) {
                    locked = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
            if !locked {
                return Err(format!("volume {letter}: is in use by another program; close its files and try again"));
            }
            if !ioctl(&f, FSCTL_DISMOUNT_VOLUME) {
                return Err(format!("could not dismount volume {letter}: {}", io::Error::last_os_error()));
            }
            held.push(f);
        }
        Ok(VolumeLocks(held))
    }
}
