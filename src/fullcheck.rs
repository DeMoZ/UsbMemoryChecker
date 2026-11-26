//! Full check without erasing, in the spirit of H2testw: fills the free space of a mounted volume with test
//! files, reads them all back and checks every 4 KiB block. Slow (the whole drive is written once), but needs
//! no administrator rights and also gives the speed over the whole surface. Files the user already has stay;
//! only the test folder is written and removed at the end.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use serde::Serialize;

use crate::bench::{CHUNK, Meter, Stats};
use crate::dio::{self, AlignedBuf};
use crate::pattern::{self, BLOCK};

pub const FILE_SIZE: u64 = 1 << 30;
/// Left free for the file system's own records.
pub const RESERVE: u64 = 8 << 20;
pub const FOLDER: &str = "UsbMemoryChecker-test";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    /// Bytes written and checked.
    pub tested: u64,
    pub ok: u64,
    pub bad: u64,
    /// Position (from the start of the test data) of the first damaged block.
    pub first_bad: Option<u64>,
    /// Volume size and bytes that were already used before the test.
    pub total: u64,
    pub used_before: u64,
    /// Estimate of the space that really keeps data: used before + test data up to the first damaged block.
    pub real_estimate: u64,
    pub write: Stats,
    pub read: Stats,
    /// The drive refused a write before the volume was full (an I/O error, not "disk full").
    pub write_error: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Progress {
    pub op: &'static str,
    pub done: u64,
    pub total: u64,
    /// Speed of the last window, when one is complete.
    pub mbps: Option<f64>,
}

#[derive(Debug)]
pub enum Error {
    Cancelled,
    NoSpace(u64),
    Io(String),
}

struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn is_disk_full(e: &std::io::Error) -> bool {
    if e.kind() == std::io::ErrorKind::StorageFull {
        return true;
    }
    #[cfg(unix)]
    if e.raw_os_error() == Some(libc::ENOSPC) {
        return true;
    }
    // ERROR_HANDLE_DISK_FULL, ERROR_DISK_FULL
    #[cfg(windows)]
    if matches!(e.raw_os_error(), Some(39) | Some(112)) {
        return true;
    }
    false
}

/// `limit` caps the amount of test data (for tests); None — all free space.
pub fn run(
    mount: &Path,
    limit: Option<u64>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Outcome, Error> {
    let (free, total) = dio::space(mount).map_err(|e| Error::Io(format!("could not read free space: {e}")))?;
    let mut planned = free.saturating_sub(RESERVE) / (1 << 20) * (1 << 20);
    if let Some(l) = limit {
        planned = planned.min(l);
    }
    if planned < CHUNK as u64 {
        return Err(Error::NoSpace(free));
    }
    let used_before = total.saturating_sub(free);
    let dir = mount.join(FOLDER);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| Error::Io(format!("could not create {}: {e}", dir.display())))?;
    let _cleanup = Cleanup(dir.clone());
    let salt = pattern::new_salt();
    let mut buf = AlignedBuf::new(CHUNK);

    // ---- write: files of 1 GiB, the data of each block depends on its position in the whole test
    let mut files: Vec<(PathBuf, u64)> = Vec::new();
    let mut meter = Meter::default();
    let mut written = 0u64;
    let mut write_error = None;
    'files: while written < planned {
        let path = dir.join(format!("{:04}.umc", files.len() + 1));
        let file = match dio::create(&path) {
            Ok(f) => f,
            Err(e) if is_disk_full(&e) => break,
            Err(e) => return Err(Error::Io(format!("could not create {}: {e}", path.display()))),
        };
        let mut in_file = 0u64;
        while in_file < FILE_SIZE && written < planned {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let len = (planned - written).min(CHUNK as u64).min(FILE_SIZE - in_file) as usize;
            pattern::fill(&mut buf[..len], written, salt);
            let t = Instant::now();
            if let Err(e) = dio::write_at(&file, &buf[..len], in_file) {
                if !is_disk_full(&e) {
                    write_error = Some(e.to_string());
                }
                files.push((path, in_file));
                break 'files;
            }
            in_file += len as u64;
            written += len as u64;
            let mbps = meter.add(len as u64, t.elapsed().as_secs_f64());
            progress(Progress { op: "write", done: written, total: planned, mbps });
        }
        let t = Instant::now();
        let _ = dio::full_sync(&file);
        meter.add_time(t.elapsed().as_secs_f64());
        files.push((path, in_file));
    }
    let write = meter.finish();

    // ---- read back and check
    let mut meter = Meter::default();
    let (mut checked, mut bad) = (0u64, 0u64);
    let mut first_bad = None;
    for (path, len) in &files {
        let file = dio::open_read(path).ok();
        let mut off = 0u64;
        while off < *len {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let n = (*len - off).min(CHUNK as u64) as usize;
            let t = Instant::now();
            let read = file.as_ref().map(|f| dio::read_at(f, &mut buf[..n], off));
            let secs = t.elapsed().as_secs_f64();
            let bad_here = match read {
                Some(Ok(())) => pattern::count_bad(&buf[..n], checked, salt) as u64 * BLOCK as u64,
                _ => n as u64,
            };
            if bad_here > 0 {
                bad += bad_here;
                first_bad.get_or_insert(checked + first_bad_block(&buf[..n], checked, salt, read.is_some()));
            }
            checked += n as u64;
            off += n as u64;
            let mbps = meter.add(n as u64, secs);
            progress(Progress { op: "read", done: checked, total: written, mbps });
        }
    }
    let real_estimate = used_before + first_bad.unwrap_or(checked);
    Ok(Outcome {
        tested: checked,
        ok: checked - bad,
        bad,
        first_bad,
        total,
        used_before,
        real_estimate,
        write,
        read: meter.finish(),
        write_error,
    })
}

/// Offset of the first damaged block inside a chunk (0 if the chunk could not be read at all).
fn first_bad_block(buf: &[u8], start: u64, salt: u64, was_read: bool) -> u64 {
    if !was_read {
        return 0;
    }
    buf.as_chunks::<BLOCK>()
        .0
        .iter()
        .enumerate()
        .find(|(i, b)| pattern::count_bad(&b[..], start + (*i * BLOCK) as u64, salt) > 0)
        .map(|(i, _)| (i * BLOCK) as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_and_checks_a_folder() {
        let mount = std::env::temp_dir().join(format!("umc_full_{}", std::process::id()));
        std::fs::create_dir_all(&mount).unwrap();
        let mut last = None;
        let out = run(&mount, Some(24 << 20), &AtomicBool::new(false), &mut |p| last = Some(p)).unwrap();
        assert_eq!(out.tested, 24 << 20);
        assert_eq!((out.ok, out.bad, out.first_bad), (24 << 20, 0, None));
        assert!(out.write_error.is_none());
        assert_eq!(last.unwrap().op, "read");
        assert!(out.real_estimate >= out.tested);
        assert!(!mount.join(FOLDER).exists(), "test folder must be removed");
        let c = run(&mount, Some(24 << 20), &AtomicBool::new(true), &mut |_| {});
        assert!(matches!(c, Err(Error::Cancelled)));
        assert!(!mount.join(FOLDER).exists());
        std::fs::remove_dir_all(&mount).unwrap();
    }

    #[test]
    fn finds_the_first_damaged_block() {
        let mut buf = vec![0u8; BLOCK * 8];
        pattern::fill(&mut buf, 1 << 20, 5);
        buf[BLOCK * 5 + 3] ^= 0xFF;
        assert_eq!(first_bad_block(&buf, 1 << 20, 5, true), (BLOCK * 5) as u64);
        assert_eq!(first_bad_block(&buf, 1 << 20, 5, false), 0);
    }
}
