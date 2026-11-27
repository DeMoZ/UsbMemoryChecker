//! Full check without erasing, in the spirit of H2testw: fills the free space of a mounted volume with test
//! files, reads them all back and checks every 4 KiB block. Slow (the whole free space is written once; a fake
//! drive is caught earlier, see below), but needs
//! no administrator rights and also gives the speed over the whole surface. Files the user already has are not touched,
//! except on a drive that wraps addresses: past its real size the writes land on the start of the drive again
//! and can damage them. Only the test folder is written and removed at the end.
//!
//! Early stop: after each completed 1 GiB test file a few 4 MiB samples are read back (new file, first file,
//! one earlier file). A fake drive that loses or wraps data past its real size is caught within a file or two of
//! passing that size, so the check does not write the rest of the fake capacity. Everything written up to then is
//! still read back and verified. The work runs over a small [`Volume`] trait so that tests can simulate fakes.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use serde::Serialize;

use crate::bench::{Meter, Stats};
use crate::dio::{self, AlignedBuf};
use crate::pattern::{self, BLOCK};
use crate::pipe::{self, Checker, Filler};

pub const FILE_SIZE: u64 = 1 << 30;
/// Left free for the file system's own records.
pub const RESERVE: u64 = 8 << 20;
pub const FOLDER: &str = "UsbMemoryChecker-test";
/// Less test data than this is not worth a run ("no space").
const MIN_TEST: u64 = 4 << 20;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    /// Bytes written and read back.
    pub tested: u64,
    pub ok: u64,
    pub bad: u64,
    /// Position (from the start of the test data) of the first damaged block.
    pub first_bad: Option<u64>,
    /// Volume size and bytes that were already used before the test.
    pub total: u64,
    pub used_before: u64,
    /// Estimate of the space that really keeps data, see [`estimate`]: used before + test data up to the first
    /// damaged block, or the test data that survived after an overwritten lead (wrap-around fake).
    pub real_estimate: u64,
    pub write: Stats,
    pub read: Stats,
    /// The drive refused a write before the volume was full (an I/O error, not "disk full").
    pub write_error: Option<String>,
    /// A sample showed data loss while writing, so the rest of the free space was not written.
    pub stopped_early: bool,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Progress {
    pub op: &'static str,
    pub done: u64,
    pub total: u64,
    /// Speed of the last window, when one is complete.
    pub mbps: Option<f64>,
    /// Read-back after writing stopped early because a sample came back with wrong data (not merely unreadable).
    pub early: bool,
}

#[derive(Debug)]
pub enum Error {
    Cancelled,
    NoSpace(u64),
    Io(String),
    /// Samples showed errors (wrong data or failed reads) that the full read-back did not confirm: writing
    /// stopped early, only part of the free space was tested and the result is unclear; run the check again.
    Inconclusive,
    /// A test file is missing and the volume itself is gone too (unplugged or unmounted): no verdict.
    Gone,
}

/// The place the test files go to: a folder on the mounted volume, or a simulated drive in tests.
pub trait Volume {
    /// (free, total) bytes.
    fn space(&self) -> io::Result<(u64, u64)>;
    /// Makes an empty test folder (an old one is removed).
    fn prepare(&mut self) -> io::Result<()>;
    /// Creates test file number `file` (0-based).
    fn create(&mut self, file: usize) -> io::Result<()>;
    fn write(&mut self, file: usize, buf: &[u8], off: u64) -> io::Result<()>;
    /// Flushes the file to the drive and closes it.
    fn finish(&mut self, file: usize) -> io::Result<()>;
    fn read(&mut self, file: usize, buf: &mut [u8], off: u64) -> io::Result<()>;
    /// Removes the test folder; safe to call more than once.
    fn cleanup(&mut self);
}

/// Sizes of one run; tests shrink them.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub file_size: u64,
    /// Size of one I/O call.
    pub chunk: usize,
    /// Size of one sample.
    pub sample: usize,
    /// No sample of a new file lies in its last `guard` bytes (a drive's own cache may still hold them).
    pub guard: u64,
}

impl Default for Params {
    fn default() -> Self {
        Params { file_size: FILE_SIZE, chunk: crate::bench::CHUNK, sample: 4 << 20, guard: 64 << 20 }
    }
}

impl Params {
    fn check(&self) {
        let b = BLOCK as u64;
        debug_assert!(self.chunk > 0 && self.file_size.is_multiple_of(self.chunk as u64));
        debug_assert!(self.file_size.is_multiple_of(b) && (self.chunk as u64).is_multiple_of(b));
        debug_assert!((self.sample as u64).is_multiple_of(b) && self.guard.is_multiple_of(b));
        debug_assert!(self.guard + 2 * self.sample as u64 <= self.file_size);
    }
}

/// The test folder on a mounted volume.
pub struct DirVolume {
    mount: PathBuf,
    dir: PathBuf,
    prepared: bool,
    writer: Option<(usize, File)>,
    reader: Option<(usize, File)>,
}

impl DirVolume {
    pub fn new(mount: &Path) -> Self {
        DirVolume { mount: mount.to_path_buf(), dir: mount.join(FOLDER), prepared: false, writer: None, reader: None }
    }

    fn path(&self, file: usize) -> PathBuf {
        self.dir.join(format!("{:04}.umc", file + 1))
    }
}

impl Volume for DirVolume {
    fn space(&self) -> io::Result<(u64, u64)> {
        dio::space(&self.mount)
    }

    fn prepare(&mut self) -> io::Result<()> {
        self.prepared = true;
        let _ = std::fs::remove_dir_all(&self.dir);
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| io::Error::new(e.kind(), format!("could not create {}: {e}", self.dir.display())))
    }

    fn create(&mut self, file: usize) -> io::Result<()> {
        self.writer = None;
        let f = dio::create(&self.path(file))?;
        self.writer = Some((file, f));
        Ok(())
    }

    fn write(&mut self, file: usize, buf: &[u8], off: u64) -> io::Result<()> {
        match &self.writer {
            Some((i, f)) if *i == file => dio::write_at(f, buf, off),
            _ => Err(io::Error::other("test file is not open for writing")),
        }
    }

    fn finish(&mut self, file: usize) -> io::Result<()> {
        match self.writer.take() {
            Some((i, f)) if i == file => dio::full_sync(&f),
            _ => Ok(()),
        }
    }

    fn read(&mut self, file: usize, buf: &mut [u8], off: u64) -> io::Result<()> {
        self.writer = None;
        if !matches!(&self.reader, Some((i, _)) if *i == file) {
            self.reader = None;
            self.reader = Some((file, dio::open_read(&self.path(file))?));
        }
        let (_, f) = self.reader.as_ref().expect("reader just opened");
        dio::read_at(f, buf, off)
    }

    fn cleanup(&mut self) {
        // Windows cannot delete open files
        self.writer = None;
        self.reader = None;
        if self.prepared {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

impl Drop for DirVolume {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn is_disk_full(e: &io::Error) -> bool {
    if e.kind() == io::ErrorKind::StorageFull {
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

/// Samples to read after file `k` (0-based) is written: (file, offset, length). `lens` are the file lengths up to
/// `k`. S1: start of file k; S2: a random place of file k outside its last `guard` bytes; S3: start of file 0;
/// S4: a random place of a random file between 1 and k-1. Offsets are block-aligned, no duplicates.
fn sample_plan(k: usize, lens: &[u64], salt: u64, p: &Params) -> Vec<(usize, u64, usize)> {
    let b = BLOCK as u64;
    let sample = p.sample as u64;
    let mut plan: Vec<(usize, u64, usize)> = Vec::new();
    let mut add = |file: usize, off: u64| {
        let len = sample.min(lens[file] - off) as usize;
        if len > 0 && !plan.iter().any(|&(f, o, _)| f == file && o == off) {
            plan.push((file, off, len));
        }
    };
    let r = pattern::splitmix(salt ^ k as u64);
    add(k, 0);
    if lens[k] >= p.guard + sample {
        add(k, (r % ((lens[k] - p.guard - sample) / b + 1)) * b);
    }
    if k >= 1 {
        add(0, 0);
    }
    if k >= 2 {
        let r2 = pattern::splitmix(r);
        let j = 1 + (r2 % (k as u64 - 1)) as usize;
        let span = lens[j].saturating_sub(sample);
        add(j, (pattern::splitmix(r2) % (span / b + 1)) * b);
    }
    plan
}

/// Lowest cell at or above `used` that the damaged test positions `[q, q + len)` sit on, for a drive that wraps
/// addresses with period `period` (test position q is at device address `used + q`, cell `(used + q) % period`).
fn lowest_cell(used: u64, period: u64, q: u64, len: u64) -> Option<u64> {
    if len >= period {
        return (period > used).then_some(used);
    }
    let c0 = (used + q) % period;
    let end = c0 + len;
    let mut best: Option<u64> = None;
    let mut segment = |a: u64, b: u64| {
        let lo = a.max(used);
        if lo < b {
            best = Some(best.map_or(lo, |m| m.min(lo)));
        }
    };
    segment(c0, end.min(period));
    if end > period {
        segment(0, end - period);
    }
    best
}

/// Estimate of the space that really keeps data (see design D4).
///
/// Without a wrap (no block of a later position was seen) it is the used space plus the test data up to the first
/// damaged block. On a drive that wraps addresses, `period` is its real size R (the distance between a position
/// and the later position whose data it holds), counted from the start of the device. Every damaged block that is
/// not such a later block sits on cell `(used_before + q) % R`; the lowest of these cells at or above `used_before`
/// (below it is the user's area, rewritten by the system) caps the estimate. So it never covers damage and never
/// exceeds R.
fn estimate(used_before: u64, tested: u64, t: &pipe::Tally) -> u64 {
    match (t.first_bad, t.period) {
        (None, _) => used_before + tested,
        (Some(p), None) => used_before + p,
        (Some(_), Some(period)) => {
            let cap = t.runs.iter().filter_map(|&(q, len)| lowest_cell(used_before, period, q, len)).min();
            cap.map_or(period, |c| c.min(period))
        }
    }
}

/// `limit` caps the amount of test data (for tests); None — all free space.
pub fn run(
    mount: &Path,
    limit: Option<u64>,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Outcome, Error> {
    run_on(&mut DirVolume::new(mount), limit, Params::default(), cancel, progress)
}

/// The full check over any [`Volume`]; the test folder is removed however it ends.
pub fn run_on(
    vol: &mut dyn Volume,
    limit: Option<u64>,
    params: Params,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Outcome, Error> {
    params.check();
    let r = run_inner(vol, limit, &params, cancel, progress);
    vol.cleanup();
    r
}

fn run_inner(
    vol: &mut dyn Volume,
    limit: Option<u64>,
    params: &Params,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(Progress),
) -> Result<Outcome, Error> {
    // an old test folder (an interrupted run) is removed first, so its files do not count as used space
    vol.prepare().map_err(|e| Error::Io(e.to_string()))?;
    let (free, total) = vol.space().map_err(|e| Error::Io(format!("could not read free space: {e}")))?;
    let mut planned = free.saturating_sub(RESERVE) / (1 << 20) * (1 << 20);
    if let Some(l) = limit {
        planned = planned.min(l);
    }
    if planned < MIN_TEST {
        return Err(Error::NoSpace(free));
    }
    let used_before = total.saturating_sub(free);
    let salt = pattern::new_salt();

    std::thread::scope(|scope| {
        // ---- write: files of 1 GiB, the data of each block depends on its position in the whole test
        let mut lens: Vec<u64> = Vec::new();
        let mut meter = Meter::default();
        let mut written = 0u64;
        let mut write_error = None;
        let mut stopped_early = false;
        let mut data_loss_seen = false;
        let mut filler = Filler::start(scope, salt, 0, planned, params.chunk);
        let mut sample_buf = AlignedBuf::new(params.sample);
        'files: while written < planned {
            let file = lens.len();
            match vol.create(file) {
                Ok(()) => {}
                Err(e) if is_disk_full(&e) => break,
                // "not found" means the drive is gone only when the volume itself is gone; a missing test file on a mounted
                // volume is damage (a wrap-around fake can overwrite the directory)
                Err(e) if e.kind() == io::ErrorKind::NotFound && vol.space().is_err() => return Err(Error::Gone),
                Err(e) => return Err(Error::Io(format!("could not create test file {}: {e}", file + 1))),
            }
            lens.push(0);
            let target = params.file_size.min(planned - written);
            let mut in_file = 0u64;
            while in_file < target {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                let Some(c) = filler.next() else {
                    return Err(Error::Io("the test data thread stopped unexpectedly".into()));
                };
                debug_assert_eq!(c.start, written);
                let t = Instant::now();
                let r = vol.write(file, &c.buf[..c.len], in_file);
                let secs = t.elapsed().as_secs_f64();
                if let Err(e) = r {
                    if !is_disk_full(&e) {
                        write_error = Some(e.to_string());
                    }
                    break 'files;
                }
                in_file += c.len as u64;
                written += c.len as u64;
                lens[file] = in_file;
                filler.recycle(c.buf);
                let mbps = meter.add(c.len as u64, secs);
                progress(Progress { op: "write", done: written, total: planned, mbps, early: false });
            }
            let t = Instant::now();
            let _ = vol.finish(file);
            meter.add_time(t.elapsed().as_secs_f64());
            if written >= planned {
                break;
            }
            // ---- samples of what was written so far: a fake drive shows itself here
            for (f, off, len) in sample_plan(file, &lens, salt, params) {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                // a read that fails for another reason than access is tried once more (a USB hiccup is not damage)
                let mut r = vol.read(f, &mut sample_buf[..len], off);
                if r.as_ref().is_err_and(|e| !dio::is_access_error(e)) {
                    r = vol.read(f, &mut sample_buf[..len], off);
                }
                // (damaged, the data itself was wrong: a failed read proves no data loss)
                let (damaged, wrong_data) = match r {
                    Ok(()) => {
                        let bad = pipe::check_chunk(&sample_buf[..len], f as u64 * params.file_size + off, salt, true)
                            .bad
                            > 0;
                        (bad, bad)
                    }
                    // "not found" means the drive is gone only when the volume itself is gone; a missing test file on a mounted
                    // volume is damage (a wrap-around fake can overwrite the directory)
                    Err(e) if e.kind() == io::ErrorKind::NotFound && vol.space().is_err() => return Err(Error::Gone),
                    Err(e) if dio::is_access_error(&e) => {
                        return Err(Error::Io(format!("could not read a sample: {e}")));
                    }
                    Err(_) => (true, false),
                };
                if damaged {
                    stopped_early = true;
                    data_loss_seen = wrong_data;
                    break 'files;
                }
            }
        }
        drop(filler);
        let write = meter.finish();

        // ---- read back everything that was written and check it
        let mut meter = Meter::default();
        let checker = Checker::start(scope, salt, params.chunk);
        let mut checked = 0u64;
        for (file, &len) in lens.iter().enumerate() {
            let mut off = 0u64;
            while off < len {
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                let n = (len - off).min(params.chunk as u64) as usize;
                let mut buf = checker.buffer();
                let t = Instant::now();
                let r = vol.read(file, &mut buf[..n], off);
                let secs = t.elapsed().as_secs_f64();
                let read_ok = match r {
                    Ok(()) => true,
                    // "not found" means the drive is gone only when the volume itself is gone; a missing test file on a mounted
                    // volume is damage (a wrap-around fake can overwrite the directory)
                    Err(e) if e.kind() == io::ErrorKind::NotFound && vol.space().is_err() => return Err(Error::Gone),
                    Err(e) if dio::is_access_error(&e) => return Err(Error::Io(format!("could not read back: {e}"))),
                    Err(_) => false,
                };
                checker.check(buf, checked, n, read_ok);
                checked += n as u64;
                off += n as u64;
                let mbps = meter.add(n as u64, secs);
                progress(Progress { op: "read", done: checked, total: written, mbps, early: data_loss_seen });
            }
        }
        let Some(tally) = checker.finish() else {
            return Err(Error::Io("the verification thread stopped unexpectedly".into()));
        };
        if stopped_early && tally.bad == 0 {
            // the drive kept everything that was written: the stop came from read errors, not from data loss
            return Err(Error::Inconclusive);
        }
        Ok(Outcome {
            tested: checked,
            ok: checked - tally.bad,
            bad: tally.bad,
            first_bad: tally.first_bad,
            total,
            used_before,
            real_estimate: estimate(used_before, checked, &tally),
            write,
            read: meter.finish(),
            write_error,
            stopped_early,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, VecDeque};
    use std::sync::Arc;
    use std::time::Duration;

    const MIB: u64 = 1 << 20;

    #[derive(Clone, Debug, PartialEq)]
    enum Op {
        Create(usize),
        Write(usize, u64, usize),
        Finish(usize),
        Read(usize, u64, usize),
    }

    type AddrMap = Box<dyn Fn(u64) -> Option<u64>>;

    /// A simulated volume: file i, byte o lives at device address `used + i * file_size + o`; `map` says which
    /// cell of the drive a written device block lands in (None: the write is lost), `read_map` does the same for
    /// reads (default: like `map`). A cell that was never written (and a lost block) reads as `limbo`.
    struct SimVolume {
        total: u64,
        used: u64,
        file_size: u64,
        map: AddrMap,
        read_map: Option<AddrMap>,
        limbo: u8,
        cells: HashMap<u64, Vec<u8>>,
        cache_blocks: usize,
        cache: HashMap<u64, Vec<u8>>,
        cache_order: VecDeque<u64>,
        read_error: Option<Box<dyn Fn(u64) -> Option<io::ErrorKind>>>,
        slow_reads: Option<(usize, Duration)>,
        cancel_at_read: Option<(usize, Arc<AtomicBool>)>,
        reads: usize,
        create_error: Option<usize>,
        disk_full_at: Option<u64>,
        /// A write that reaches this test position fails with an I/O error (not "disk full").
        write_error_at: Option<u64>,
        /// Numbers (1-based) of the read calls that fail with an I/O error, whatever they read.
        flaky_reads: Vec<usize>,
        /// Reads of this file fail with "not found" (the volume went away).
        missing_file: Option<usize>,
        /// Creating this file fails with "not found" (the test folder is gone).
        create_not_found: Option<usize>,
        /// `space()` fails after the first call (the one at the start of the check): the volume itself is gone.
        volume_gone: bool,
        space_calls: std::cell::Cell<u32>,
        /// Numbers (1-based) of the read calls whose first byte comes back flipped (no error).
        flip_reads: Vec<usize>,
        /// Bytes of an old test folder that `prepare` removes (they count as used space until then).
        leftover: u64,
        /// Lengths of the reads that got a block from the write cache.
        cache_hit_lens: Vec<usize>,
        log: Vec<Op>,
        cleaned: bool,
    }

    fn params() -> Params {
        Params { file_size: 8 * MIB, chunk: MIB as usize, sample: 256 << 10, guard: 2 * MIB }
    }

    impl SimVolume {
        fn new(total: u64, used: u64, map: impl Fn(u64) -> Option<u64> + 'static) -> Self {
            SimVolume {
                total,
                used,
                file_size: params().file_size,
                map: Box::new(map),
                read_map: None,
                limbo: 0,
                cells: HashMap::new(),
                cache_blocks: 0,
                cache: HashMap::new(),
                cache_order: VecDeque::new(),
                read_error: None,
                slow_reads: None,
                cancel_at_read: None,
                reads: 0,
                create_error: None,
                disk_full_at: None,
                write_error_at: None,
                flaky_reads: Vec::new(),
                missing_file: None,
                create_not_found: None,
                volume_gone: false,
                space_calls: std::cell::Cell::new(0),
                flip_reads: Vec::new(),
                leftover: 0,
                cache_hit_lens: Vec::new(),
                log: Vec::new(),
                cleaned: false,
            }
        }

        fn genuine(total: u64, used: u64) -> Self {
            Self::new(total, used, Some)
        }

        fn dev(&self, file: usize, off: u64) -> u64 {
            self.used + file as u64 * self.file_size + off
        }
    }

    impl Volume for SimVolume {
        fn space(&self) -> io::Result<(u64, u64)> {
            self.space_calls.set(self.space_calls.get() + 1);
            if self.volume_gone && self.space_calls.get() > 1 {
                return Err(io::Error::from(io::ErrorKind::NotFound));
            }
            Ok((self.total - self.used - self.leftover, self.total))
        }
        fn prepare(&mut self) -> io::Result<()> {
            self.leftover = 0;
            Ok(())
        }
        fn create(&mut self, file: usize) -> io::Result<()> {
            self.log.push(Op::Create(file));
            if self.create_not_found == Some(file) {
                return Err(io::Error::from(io::ErrorKind::NotFound));
            }
            if self.create_error == Some(file) {
                return Err(io::Error::other("cannot create"));
            }
            Ok(())
        }
        fn write(&mut self, file: usize, buf: &[u8], off: u64) -> io::Result<()> {
            self.log.push(Op::Write(file, off, buf.len()));
            let pos = file as u64 * self.file_size + off;
            if self.disk_full_at.is_some_and(|f| pos + buf.len() as u64 > f) {
                return Err(io::Error::from(io::ErrorKind::StorageFull));
            }
            if self.write_error_at.is_some_and(|f| pos + buf.len() as u64 > f) {
                return Err(io::Error::other("write failed"));
            }
            let base = self.dev(file, off);
            for (i, block) in buf.chunks(BLOCK).enumerate() {
                let addr = base + (i * BLOCK) as u64;
                if self.cache_blocks > 0 {
                    if self.cache.insert(addr, block.to_vec()).is_none() {
                        self.cache_order.push_back(addr);
                    }
                    if self.cache_order.len() > self.cache_blocks {
                        let old = self.cache_order.pop_front().unwrap();
                        let data = self.cache.remove(&old).unwrap();
                        if let Some(cell) = (self.map)(old) {
                            self.cells.insert(cell, data);
                        }
                    }
                } else if let Some(cell) = (self.map)(addr) {
                    self.cells.insert(cell, block.to_vec());
                }
            }
            Ok(())
        }
        fn finish(&mut self, file: usize) -> io::Result<()> {
            self.log.push(Op::Finish(file));
            Ok(())
        }
        fn read(&mut self, file: usize, buf: &mut [u8], off: u64) -> io::Result<()> {
            self.log.push(Op::Read(file, off, buf.len()));
            self.reads += 1;
            if let Some((n, flag)) = &self.cancel_at_read
                && *n == self.reads
            {
                flag.store(true, Ordering::Relaxed);
            }
            if self.missing_file == Some(file) {
                return Err(io::Error::from(io::ErrorKind::NotFound));
            }
            if self.flaky_reads.contains(&self.reads) {
                return Err(io::Error::other("read hiccup"));
            }
            if let Some((len, d)) = self.slow_reads
                && len == buf.len()
            {
                std::thread::sleep(d);
            }
            let base = self.dev(file, off);
            let mut hit = false;
            for (i, block) in buf.chunks_mut(BLOCK).enumerate() {
                let addr = base + (i * BLOCK) as u64;
                if let Some(kind) = self.read_error.as_ref().and_then(|f| f(addr)) {
                    return Err(io::Error::from(kind));
                }
                let data = self.cache.get(&addr).or_else(|| {
                    let m = self.read_map.as_ref().unwrap_or(&self.map);
                    m(addr).and_then(|c| self.cells.get(&c))
                });
                match data {
                    Some(d) => block.copy_from_slice(d),
                    None => block.fill(self.limbo),
                }
                hit |= self.cache.contains_key(&addr);
            }
            if hit {
                self.cache_hit_lens.push(buf.len());
            }
            if self.flip_reads.contains(&self.reads) {
                buf[0] ^= 0xFF;
            }
            Ok(())
        }
        fn cleanup(&mut self) {
            self.cleaned = true;
        }
    }

    fn run_sim(sim: &mut SimVolume) -> (Result<Outcome, Error>, Vec<Progress>) {
        run_sim_with(sim, &AtomicBool::new(false))
    }

    fn run_sim_with(sim: &mut SimVolume, cancel: &AtomicBool) -> (Result<Outcome, Error>, Vec<Progress>) {
        let mut events = Vec::new();
        let r = run_on(sim, None, params(), cancel, &mut |p| events.push(p));
        (r, events)
    }

    /// Every event is write or read, and `done` never decreases within a phase.
    fn check_events(events: &[Progress]) {
        let mut last = HashMap::new();
        for e in events {
            assert!(e.op == "write" || e.op == "read");
            let l = last.entry(e.op).or_insert(0);
            assert!(e.done >= *l, "done went back in {}", e.op);
            *l = e.done;
        }
    }

    fn assert_clean(sim: &SimVolume, events: &[Progress]) {
        assert!(sim.cleaned);
        check_events(events);
    }

    fn within_below(estimate: u64, real: u64, slack: u64) {
        assert!(estimate <= real && real - estimate <= slack, "estimate {estimate} vs real {real}");
    }

    // ---- real folder

    #[test]
    fn fills_and_checks_a_folder() {
        let mount = std::env::temp_dir().join(format!("umc_full_{}", std::process::id()));
        std::fs::create_dir_all(&mount).unwrap();
        std::fs::write(mount.join("keep.txt"), b"mine").unwrap();
        let mut last = None;
        let out = run(&mount, Some(24 << 20), &AtomicBool::new(false), &mut |p| last = Some(p)).unwrap();
        assert_eq!(out.tested, 24 << 20);
        assert_eq!((out.ok, out.bad, out.first_bad), (24 << 20, 0, None));
        assert!(out.write_error.is_none());
        assert!(!out.stopped_early);
        assert_eq!(std::fs::read(mount.join("keep.txt")).unwrap(), b"mine", "other files are untouched");
        assert_eq!(last.unwrap().op, "read");
        assert!(out.real_estimate >= out.tested);
        assert!(!mount.join(FOLDER).exists(), "test folder must be removed");
        let c = run(&mount, Some(24 << 20), &AtomicBool::new(true), &mut |_| {});
        assert!(matches!(c, Err(Error::Cancelled)));
        assert!(!mount.join(FOLDER).exists());
        std::fs::remove_dir_all(&mount).unwrap();
    }

    // ---- genuine simulated volume

    #[test]
    fn genuine_sim_volume() {
        let mut sim = SimVolume::genuine(64 * MIB, MIB);
        let (r, ev) = run_sim(&mut sim);
        let out = r.unwrap();
        assert_eq!(out.tested, 55 * MIB);
        assert_eq!((out.bad, out.first_bad, out.stopped_early), (0, None, false));
        assert_eq!(out.real_estimate, MIB + out.tested);
        assert_clean(&sim, &ev);
    }

    #[test]
    fn sim_cells_hold_the_pattern() {
        let mut sim = SimVolume::genuine(64 * MIB, MIB);
        let (r, _) = run_sim(&mut sim);
        let out = r.unwrap();
        assert_eq!(out.bad, 0);
        assert_eq!(sim.cells.len() as u64, out.tested / BLOCK as u64);
        for (cell, data) in &sim.cells {
            let pos = u64::from_le_bytes(data[..8].try_into().unwrap());
            assert_eq!(pos, cell - sim.used, "block is laid out at its test position");
        }
    }

    #[test]
    fn disk_full_mid_write() {
        let mut sim = SimVolume::genuine(64 * MIB, MIB);
        sim.disk_full_at = Some(20 * MIB);
        let (r, ev) = run_sim(&mut sim);
        let out = r.unwrap();
        assert!(out.write_error.is_none());
        assert_eq!(out.tested, 20 * MIB);
        assert_eq!(out.bad, 0);
        let last_write = sim.log.iter().rposition(|o| matches!(o, Op::Write(..))).unwrap();
        assert!(
            sim.log[last_write + 1..].iter().all(|o| matches!(o, Op::Read(_, _, l) if *l == MIB as usize)),
            "no sample read after the last file"
        );
        assert_clean(&sim, &ev);
    }

    #[test]
    fn create_error_cleans_up() {
        let mut sim = SimVolume::genuine(64 * MIB, MIB);
        sim.create_error = Some(0);
        let (r, ev) = run_sim(&mut sim);
        assert!(matches!(r, Err(Error::Io(_))));
        assert_clean(&sim, &ev);
    }

    // ---- estimate and sample plan

    #[test]
    fn estimate_cases() {
        let t = |first_bad, period, runs: &[(u64, u64)]| pipe::Tally {
            first_bad,
            period,
            runs: runs.to_vec(),
            ..pipe::Tally::default()
        };
        let e = |t: &pipe::Tally| estimate(10, 100, t);
        assert_eq!(e(&t(None, None, &[])), 110, "genuine");
        assert_eq!(e(&t(Some(40), None, &[(40, 60)])), 50, "lost writes: used + first bad");
        assert_eq!(e(&t(Some(0), None, &[(0, 100)])), 10, "no proof of a wrap");
        // wrap with period (real size) 70, used 10
        assert_eq!(e(&t(Some(0), Some(70), &[])), 70, "wrap, nothing else damaged");
        assert_eq!(e(&t(Some(0), Some(70), &[(50, 4)])), 60, "damage at 50 sits on cell 60");
        assert_eq!(e(&t(Some(0), Some(70), &[(95, 4)])), 35, "damage at 95 (second pass): cell (10 + 95) % 70");
        assert_eq!(e(&t(Some(0), Some(70), &[(65, 4)])), 70, "cell 5 is the user's area");
        assert_eq!(e(&t(Some(0), Some(70), &[(55, 10)])), 65, "a run over the end of the cells");
        assert_eq!(e(&t(Some(0), Some(70), &[(55, 30)])), 10, "a run over the end reaches the used space");
        assert_eq!(e(&t(Some(0), Some(70), &[(95, 4), (50, 4)])), 35, "the lowest cell wins");
        assert_eq!(estimate(80, 100, &t(Some(0), Some(70), &[(5, 4)])), 70, "user files bigger than the real size");
    }

    #[test]
    fn lowest_cell_cases() {
        assert_eq!(lowest_cell(10, 70, 50, 4), Some(60));
        assert_eq!(lowest_cell(10, 70, 65, 4), None);
        assert_eq!(lowest_cell(10, 70, 55, 10), Some(65));
        assert_eq!(lowest_cell(10, 70, 55, 30), Some(10));
        assert_eq!(lowest_cell(10, 70, 0, 70), Some(10));
        assert_eq!(lowest_cell(80, 70, 0, 70), None);
        assert_eq!(lowest_cell(0, 70, 5, 3), Some(5));
    }

    #[test]
    fn default_params() {
        assert_eq!(Params::default().chunk, 4 << 20);
        assert_eq!(crate::bench::CHUNK, 4 << 20);
    }

    #[test]
    fn partial_chip_small_hole_then_wrap() {
        // address space 48 MiB, cells 46..48 MiB lose data, addresses above 48 MiB wrap to 0
        for _ in 0..8 {
            let mut sim = SimVolume::new(256 * MIB, MIB, |d| {
                let a = d % (48 * MIB);
                (a < 46 * MIB).then_some(a)
            });
            let (r, ev) = run_sim(&mut sim);
            let out = r.unwrap();
            assert!(out.stopped_early);
            within_below(out.real_estimate, 46 * MIB, params().guard + MIB);
            assert_clean(&sim, &ev);
        }
    }

    #[test]
    fn wrap_with_user_data() {
        // R = 40 MiB with 20 MiB already used: positions from 20 MiB on overwrite the start of the drive
        let real = 40 * MIB;
        let mut sim = SimVolume::new(256 * MIB, 20 * MIB, move |d| Some(d % real));
        let (r, ev) = run_sim(&mut sim);
        let out = r.unwrap();
        assert!(out.stopped_early);
        assert!(out.tested <= real + 2 * params().file_size);
        within_below(out.real_estimate, real, params().guard + MIB);
        assert_clean(&sim, &ev);
    }

    #[test]
    fn one_bad_block_at_the_start_is_no_wrap() {
        // a genuine drive with 20 MiB used and one lost block at the very start of the test data
        let mut sim = SimVolume::new(64 * MIB, 20 * MIB, |d| (d != 20 * MIB).then_some(d));
        let (r, ev) = run_sim(&mut sim);
        let out = r.unwrap();
        // (the sample of file 0 hits the bad block, so writing stops early; the estimate must not change)
        assert_eq!((out.bad, out.first_bad), (BLOCK as u64, Some(0)));
        assert_eq!(out.real_estimate, 20 * MIB, "used_before + first_bad, not a wrap");
        assert_clean(&sim, &ev);
    }

    #[test]
    fn sample_plan_positions() {
        let p = params();
        let lens = [p.file_size; 6];
        let salt = 12345;
        let p0 = sample_plan(0, &lens[..1], salt, &p);
        assert!(!p0.is_empty() && p0.iter().all(|s| s.0 == 0));
        let p1 = sample_plan(1, &lens[..2], salt, &p);
        assert!(p1.iter().any(|s| s.0 == 1) && p1.iter().any(|s| s.0 == 0));
        assert!(p1.iter().all(|s| s.0 <= 1));
        for k in 0..6 {
            for salt in 0..50u64 {
                let plan = sample_plan(k, &lens[..=k], salt, &p);
                for (i, &(f, off, len)) in plan.iter().enumerate() {
                    assert!(f <= k && off % BLOCK as u64 == 0 && len % BLOCK == 0 && len > 0);
                    assert!(off + len as u64 <= lens[f]);
                    assert!(plan.iter().skip(i + 1).all(|&(f2, o2, _)| (f2, o2) != (f, off)), "duplicate");
                    if f == k && off != 0 {
                        assert!(off + len as u64 <= lens[f] - p.guard, "sample inside the guard");
                    }
                }
                if k == 5 {
                    assert!(plan.iter().any(|s| s.0 == 5 && s.1 == 0));
                    assert!(plan.iter().any(|s| s.0 == 0 && s.1 == 0));
                    assert!(plan.iter().any(|s| (1..=4).contains(&s.0)));
                }
            }
        }
    }

    #[test]
    fn default_sampling_cost() {
        let p = Params::default();
        let lens = [p.file_size; 8];
        for k in 0..8 {
            let bytes: u64 = sample_plan(k, &lens[..=k], 99, &p).iter().map(|s| s.2 as u64).sum();
            assert!(bytes <= FILE_SIZE / 64, "k = {k}: {bytes}");
        }
    }

    // ---- fake volumes

    #[test]
    fn genuine_volume_is_not_stopped() {
        let mut sim = SimVolume::genuine(64 * MIB, MIB);
        let (r, ev) = run_sim(&mut sim);
        let out = r.unwrap();
        assert!(!out.stopped_early);
        assert_clean(&sim, &ev);
        let last_write = sim.log.iter().rposition(|o| matches!(o, Op::Write(..))).unwrap();
        assert!(sim.log[..last_write].iter().any(|o| matches!(o, Op::Read(_, _, l) if *l == 256 << 10)));
        let last_finish = sim.log.iter().rposition(|o| matches!(o, Op::Finish(_))).unwrap();
        let tail = &sim.log[last_finish + 1..];
        assert_eq!(tail[0], Op::Read(0, 0, MIB as usize), "read-back follows the last file directly");
        assert!(tail.iter().all(|o| matches!(o, Op::Read(_, _, l) if *l == MIB as usize)));
        assert_eq!(tail.len() as u64, 55);
    }

    #[test]
    fn wrap_around_fake_stops_early() {
        for real in [40 * MIB, 32 * MIB] {
            let mut sim = SimVolume::new(256 * MIB, MIB, move |d| Some(d % real));
            let (r, ev) = run_sim(&mut sim);
            let out = r.unwrap();
            assert!(out.stopped_early, "R = {real}");
            assert!(out.tested <= real + 2 * params().file_size);
            within_below(out.real_estimate, real, params().guard + MIB);
            assert_clean(&sim, &ev);
        }
    }

    #[test]
    fn lost_writes_fake_stops_early() {
        let real = 40 * MIB;
        for limbo in [0x00, 0xFF] {
            let mut sim = SimVolume::new(256 * MIB, MIB, move |d| (d < real).then_some(d));
            sim.limbo = limbo;
            let (r, ev) = run_sim(&mut sim);
            let out = r.unwrap();
            assert!(out.stopped_early);
            assert!(out.tested <= real + 2 * params().file_size);
            within_below(out.real_estimate, real, params().guard + MIB);
            assert_clean(&sim, &ev);
            // the read-back covers exactly what was written: positions from 39 MiB (device 40 MiB) are lost
            let written: u64 = sim.log.iter().map(|o| if let Op::Write(_, _, l) = o { *l as u64 } else { 0 }).sum();
            assert_eq!(out.tested, written);
            assert_eq!((out.ok + out.bad, out.bad), (written, written - 39 * MIB));
            assert_eq!(out.first_bad, Some(39 * MIB));
        }
    }

    #[test]
    fn stale_reads_fake_stops_early() {
        let real = 40 * MIB;
        let mut sim = SimVolume::new(256 * MIB, MIB, move |d| (d < real).then_some(d));
        sim.read_map = Some(Box::new(move |d| Some(d % real)));
        let (r, ev) = run_sim(&mut sim);
        let out = r.unwrap();
        assert!(out.stopped_early);
        assert!(out.tested <= real + 2 * params().file_size);
        assert_clean(&sim, &ev);
    }

    #[test]
    fn cache_does_not_hide_lost_writes() {
        let real = 40 * MIB;
        let mut sim = SimVolume::new(256 * MIB, MIB, move |d| (d < real).then_some(d));
        // the cache (1.5 MiB) is smaller than the guard (2 MiB) but larger than nothing: a sample inside the last
        // 1.5 MiB of the new file would be answered from it and show intact data
        sim.cache_blocks = (3 * MIB / 2 / BLOCK as u64) as usize;
        let (r, ev) = run_sim(&mut sim);
        let out = r.unwrap();
        assert!(out.stopped_early);
        assert!(out.tested <= real + 2 * params().file_size);
        assert!(!sim.cache_hit_lens.is_empty(), "the cache was in play (read-back)");
        assert!(
            !sim.cache_hit_lens.contains(&params().sample),
            "no sample read was answered from the cache: the guard keeps them out of it"
        );
        assert_clean(&sim, &ev);
    }

    #[test]
    fn partial_chip_then_wrap() {
        let mut sim = SimVolume::new(256 * MIB, MIB, |d| {
            let a = d % (64 * MIB);
            (a < 40 * MIB).then_some(a)
        });
        let (r, ev) = run_sim(&mut sim);
        let out = r.unwrap();
        assert!(out.stopped_early);
        within_below(out.real_estimate, 40 * MIB, params().guard + MIB);
        assert_clean(&sim, &ev);
    }

    #[test]
    fn failing_flash_in_the_middle() {
        let mut sim = SimVolume::new(256 * MIB, MIB, |d| (!(20 * MIB..44 * MIB).contains(&d)).then_some(d));
        let (r, ev) = run_sim(&mut sim);
        let out = r.unwrap();
        assert!(out.stopped_early);
        assert_eq!(out.first_bad, Some(19 * MIB));
        assert_eq!(out.real_estimate, 20 * MIB);
        // at most two files after the start of the damaged region (position 19 MiB)
        assert!(out.tested <= 19 * MIB + 2 * params().file_size, "tested {}", out.tested);
        assert_clean(&sim, &ev);
    }

    #[test]
    fn small_damaged_region_found_by_read_back() {
        let mut sim = SimVolume::new(256 * MIB, MIB, |d| (!(20 * MIB..21 * MIB).contains(&d)).then_some(d));
        let (r, ev) = run_sim(&mut sim);
        let out = r.unwrap();
        assert!(out.bad >= MIB);
        assert_eq!(out.first_bad, Some(19 * MIB));
        assert_clean(&sim, &ev);
    }

    #[test]
    fn drive_that_keeps_nothing() {
        let mut sim = SimVolume::new(256 * MIB, MIB, |_| None);
        let (r, ev) = run_sim(&mut sim);
        let out = r.unwrap();
        assert!(out.stopped_early);
        assert_eq!(out.tested, params().file_size);
        assert_eq!(out.real_estimate, MIB);
        assert_clean(&sim, &ev);
    }

    #[test]
    fn sample_media_error_stops_early() {
        let mut sim = SimVolume::genuine(64 * MIB, MIB);
        sim.read_error = Some(Box::new(|a| (a == MIB).then_some(io::ErrorKind::Other)));
        let (r, ev) = run_sim(&mut sim);
        let out = r.unwrap();
        assert!(out.stopped_early);
        assert_clean(&sim, &ev);
    }

    #[test]
    fn access_error_aborts() {
        // during sampling: the very first sample (file 0, offset 0)
        let mut sim = SimVolume::genuine(64 * MIB, MIB);
        sim.read_error =
            Some(Box::new(|a| (a < MIB + BLOCK as u64 && a >= MIB).then_some(io::ErrorKind::PermissionDenied)));
        let (r, ev) = run_sim(&mut sim);
        assert!(matches!(r, Err(Error::Io(_))));
        assert_clean(&sim, &ev);
        // only during the read-back: the last (partial) file is never sampled
        let mut sim = SimVolume::genuine(64 * MIB, MIB);
        let from = MIB + 6 * 8 * MIB;
        sim.read_error = Some(Box::new(move |a| (a >= from).then_some(io::ErrorKind::PermissionDenied)));
        let (r, ev) = run_sim(&mut sim);
        assert!(matches!(r, Err(Error::Io(_))));
        assert_clean(&sim, &ev);
    }

    #[test]
    fn create_not_found_is_gone() {
        let mut sim = SimVolume::genuine(64 * MIB, MIB);
        sim.volume_gone = true;
        sim.create_not_found = Some(1);
        let (r, ev) = run_sim(&mut sim);
        assert!(matches!(r, Err(Error::Gone)), "{r:?}");
        assert_clean(&sim, &ev);
    }

    #[test]
    fn missing_file_in_sample_is_gone() {
        let mut sim = SimVolume::genuine(64 * MIB, MIB);
        sim.volume_gone = true;
        sim.missing_file = Some(0);
        let (r, ev) = run_sim(&mut sim);
        assert!(matches!(r, Err(Error::Gone)), "{r:?}");
        assert_clean(&sim, &ev);
    }

    #[test]
    fn missing_file_in_readback_is_gone() {
        // the last (partial) file is never sampled, so only the read-back opens it
        let mut sim = SimVolume::genuine(64 * MIB, MIB);
        sim.volume_gone = true;
        sim.missing_file = Some(6);
        let (r, ev) = run_sim(&mut sim);
        assert!(matches!(r, Err(Error::Gone)), "{r:?}");
        assert_clean(&sim, &ev);
    }

    #[test]
    fn a_missing_test_file_on_a_mounted_volume_is_damage() {
        // a wrap-around fake can overwrite the directory: the volume is there, the file is not
        let mut sim = SimVolume::genuine(64 * MIB, MIB);
        sim.missing_file = Some(0);
        let (r, ev) = run_sim(&mut sim);
        let out = r.expect("a result, not Gone");
        assert!(out.bad > 0 && out.stopped_early, "{out:?}");
        assert_clean(&sim, &ev);
        // in the read-back only (the last file is never sampled)
        let mut sim = SimVolume::genuine(64 * MIB, MIB);
        sim.missing_file = Some(6);
        let (r, ev) = run_sim(&mut sim);
        assert!(r.unwrap().bad > 0);
        assert_clean(&sim, &ev);
    }

    #[test]
    fn cancel_during_sample_reads() {
        let real = 40 * MIB;
        let flag = Arc::new(AtomicBool::new(false));
        let mut sim = SimVolume::new(256 * MIB, MIB, move |d| Some(d % real));
        sim.cancel_at_read = Some((1, Arc::clone(&flag)));
        let (r, ev) = run_sim_with(&mut sim, &flag);
        assert!(matches!(r, Err(Error::Cancelled)));
        assert!(matches!(sim.log.last(), Some(Op::Read(..))));
        assert_eq!(sim.log.iter().filter(|o| matches!(o, Op::Read(..))).count(), 1);
        assert_clean(&sim, &ev);
    }

    #[test]
    fn sample_time_not_in_write_stats() {
        let mut sim = SimVolume::genuine(64 * MIB, MIB);
        let delay = Duration::from_millis(300);
        sim.slow_reads = Some((256 << 10, delay));
        let (r, ev) = run_sim(&mut sim);
        let out = r.unwrap();
        let samples = sim.log.iter().filter(|o| matches!(o, Op::Read(_, _, l) if *l == 256 << 10)).count() as f64;
        assert!(samples > 0.0);
        // the sample reads together take samples * delay: none of that time may be in the write stats
        assert!(out.write.seconds < samples * delay.as_secs_f64(), "write.seconds = {}", out.write.seconds);
        assert_clean(&sim, &ev);
    }

    #[test]
    fn early_flag_in_read_progress() {
        let real = 40 * MIB;
        let mut sim = SimVolume::new(256 * MIB, MIB, move |d| Some(d % real));
        let (r, ev) = run_sim(&mut sim);
        assert!(r.unwrap().stopped_early);
        assert!(ev.iter().filter(|e| e.op == "read").all(|e| e.early));
        assert!(ev.iter().filter(|e| e.op == "write").all(|e| !e.early));
        let mut sim = SimVolume::genuine(64 * MIB, MIB);
        let (r, ev) = run_sim(&mut sim);
        r.unwrap();
        assert!(ev.iter().all(|e| !e.early));
    }

    #[test]
    fn too_little_free_space() {
        let mut sim = SimVolume::genuine(10 * MIB, 0);
        let (r, ev) = run_sim(&mut sim);
        assert!(matches!(r, Err(Error::NoSpace(_))));
        assert!(sim.log.is_empty(), "nothing is written");
        assert!(ev.is_empty());
    }

    #[test]
    fn write_error_ends_writing_without_sampling() {
        let mut sim = SimVolume::genuine(64 * MIB, MIB);
        sim.write_error_at = Some(20 * MIB);
        let (r, ev) = run_sim(&mut sim);
        let out = r.unwrap();
        assert!(out.write_error.is_some());
        assert_eq!((out.tested, out.bad, out.stopped_early), (20 * MIB, 0, false));
        assert!(!sim.log.contains(&Op::Finish(2)), "the broken file is not flushed");
        let last_write = sim.log.iter().rposition(|o| matches!(o, Op::Write(..))).unwrap();
        let tail = &sim.log[last_write + 1..];
        assert!(tail.iter().all(|o| matches!(o, Op::Read(_, _, l) if *l == MIB as usize)), "no sample after the error");
        assert_eq!(tail.len() as u64, 20);
        assert_clean(&sim, &ev);
    }

    #[test]
    fn unreadable_chunk_counts_and_the_check_goes_on() {
        let mut sim = SimVolume::genuine(64 * MIB, MIB);
        // the last (partial) file is never sampled: its third chunk fails during the read-back only
        let bad_addr = MIB + 6 * 8 * MIB + 2 * MIB + 3 * BLOCK as u64;
        sim.read_error = Some(Box::new(move |a| (a == bad_addr).then_some(io::ErrorKind::Other)));
        let (r, ev) = run_sim(&mut sim);
        let out = r.unwrap();
        assert!(!out.stopped_early);
        assert_eq!(out.tested, 55 * MIB);
        assert_eq!((out.bad, out.first_bad), (MIB, Some(50 * MIB)));
        assert_eq!(ev.last().unwrap().done, 55 * MIB, "the check went on to the end");
        assert_clean(&sim, &ev);
    }

    #[test]
    fn small_damaged_region_nobody_samples() {
        // device 52..53 MiB lies in the last (partial) file, which no checkpoint samples
        let mut sim = SimVolume::new(64 * MIB, MIB, |d| (!(52 * MIB..53 * MIB).contains(&d)).then_some(d));
        let (r, ev) = run_sim(&mut sim);
        let out = r.unwrap();
        assert!(!out.stopped_early, "writing went on");
        assert_eq!(out.tested, 55 * MIB);
        assert_eq!((out.bad, out.first_bad), (MIB, Some(51 * MIB)));
        assert_clean(&sim, &ev);
    }

    #[test]
    fn sample_plan_for_the_fifth_file() {
        let p = params();
        let lens = [p.file_size; 5];
        for salt in 0..100u64 {
            let plan = sample_plan(4, &lens, salt, &p);
            assert!(plan.iter().any(|s| (s.0, s.1) == (4, 0)));
            assert!(plan.iter().any(|s| (s.0, s.1) == (0, 0)));
            assert!(plan.iter().any(|s| (1..=3).contains(&s.0)));
            assert!(plan.iter().filter(|s| s.0 == 4).all(|s| s.1 + s.2 as u64 <= p.file_size - p.guard));
            assert!((3..=4).contains(&plan.len()));
        }
    }

    #[test]
    fn a_failed_sample_read_is_tried_again() {
        let mut sim = SimVolume::genuine(64 * MIB, MIB);
        sim.flaky_reads = vec![1];
        let (r, ev) = run_sim(&mut sim);
        let out = r.unwrap();
        assert!(!out.stopped_early);
        assert_eq!((out.bad, out.tested), (0, 55 * MIB));
        assert_clean(&sim, &ev);
    }

    #[test]
    fn early_stop_with_a_clean_read_back_is_inconclusive() {
        let mut sim = SimVolume::genuine(64 * MIB, MIB);
        sim.flaky_reads = vec![1, 2];
        let (r, ev) = run_sim(&mut sim);
        assert!(matches!(r, Err(Error::Inconclusive)));
        assert_clean(&sim, &ev);
    }

    #[test]
    fn dir_volume_with_several_files() {
        let mount = std::env::temp_dir().join(format!("umc_full_dir_{}", std::process::id()));
        std::fs::create_dir_all(&mount).unwrap();
        let mut events = Vec::new();
        let mut vol = DirVolume::new(&mount);
        let out = run_on(&mut vol, Some(40 * MIB), params(), &AtomicBool::new(false), &mut |p| events.push(p)).unwrap();
        assert!(!out.stopped_early);
        assert_eq!((out.bad, out.tested), (0, 40 * MIB));
        check_events(&events);
        assert!(!mount.join(FOLDER).exists(), "test folder must be removed");
        // cancelled with readers open on earlier files
        let flag = AtomicBool::new(false);
        let mut reads = 0;
        let r = run_on(&mut vol, Some(40 * MIB), params(), &flag, &mut |p| {
            if p.op == "read" {
                reads += 1;
                if reads == 12 {
                    flag.store(true, Ordering::Relaxed);
                }
            }
        });
        assert!(matches!(r, Err(Error::Cancelled)));
        assert!(!mount.join(FOLDER).exists());
        std::fs::remove_dir_all(&mount).unwrap();
    }

    #[test]
    fn wrap_with_dead_cells_near_the_start() {
        // wrap mod 40 MiB, cells 4..5 MiB return zeros: only the first 4 MiB of the drive are usable
        let real = 40 * MIB;
        let mut sim = SimVolume::new(256 * MIB, MIB, move |d| {
            let a = d % real;
            (!(4 * MIB..5 * MIB).contains(&a)).then_some(a)
        });
        let (r, ev) = run_sim(&mut sim);
        let out = r.unwrap();
        assert!(out.stopped_early);
        assert!(out.real_estimate <= 4 * MIB, "estimate {} covers the dead cells", out.real_estimate);
        assert_clean(&sim, &ev);
    }

    #[test]
    fn wrap_with_an_unreadable_cell_near_the_start() {
        let real = 40 * MIB;
        let mut sim = SimVolume::new(256 * MIB, MIB, move |d| Some(d % real));
        sim.read_error = Some(Box::new(move |a| (a % real == 5 * MIB).then_some(io::ErrorKind::Other)));
        let (r, ev) = run_sim(&mut sim);
        let out = r.unwrap();
        assert!(out.real_estimate <= 5 * MIB, "estimate {} covers the unreadable cell", out.real_estimate);
        assert_clean(&sim, &ev);
    }

    #[test]
    fn wrap_with_a_hole_in_the_middle() {
        let real = 40 * MIB;
        let mut sim = SimVolume::new(256 * MIB, MIB, move |d| {
            let a = d % real;
            (!(20 * MIB..21 * MIB).contains(&a)).then_some(a)
        });
        let (r, ev) = run_sim(&mut sim);
        let out = r.unwrap();
        assert!(out.real_estimate <= 20 * MIB);
        assert!(out.real_estimate + params().guard + MIB >= 20 * MIB, "not far below the hole");
        assert_clean(&sim, &ev);
    }

    #[test]
    fn wrap_with_a_controller_cache_is_not_overestimated() {
        for cache in [3 * MIB / 2, 2 * MIB] {
            let real = 40 * MIB;
            let mut sim = SimVolume::new(256 * MIB, MIB, move |d| Some(d % real));
            sim.cache_blocks = (cache / BLOCK as u64) as usize;
            let (r, ev) = run_sim(&mut sim);
            let out = r.unwrap();
            assert!(out.stopped_early);
            assert!(out.real_estimate <= real, "estimate {} above the real size", out.real_estimate);
            assert_clean(&sim, &ev);
        }
    }

    #[test]
    fn a_leftover_test_folder_does_not_shrink_the_test() {
        let mut sim = SimVolume::genuine(64 * MIB, MIB);
        sim.leftover = 40 * MIB;
        let (r, ev) = run_sim(&mut sim);
        let out = r.unwrap();
        assert_eq!(out.tested, 55 * MIB);
        assert_eq!(out.used_before, MIB);
        assert_clean(&sim, &ev);
    }

    #[test]
    fn read_error_stop_is_not_reported_as_data_loss() {
        // the sample fails twice (stop), but no data was wrong: the progress must not say "data loss found"
        let mut sim = SimVolume::genuine(64 * MIB, MIB);
        sim.flaky_reads = vec![1, 2];
        let (r, ev) = run_sim(&mut sim);
        assert!(matches!(r, Err(Error::Inconclusive)));
        assert!(ev.iter().all(|e| !e.early));
    }

    #[test]
    fn wrap_with_a_hole_at_the_end_of_the_lead() {
        // wrap mod 37 MiB, used 1 MiB, dead cells 4..5 MiB right after the overwritten lead
        let real = 37 * MIB;
        for _ in 0..8 {
            let mut sim = SimVolume::new(256 * MIB, MIB, move |d| {
                let a = d % real;
                (!(4 * MIB..5 * MIB).contains(&a)).then_some(a)
            });
            let (r, ev) = run_sim(&mut sim);
            let out = r.unwrap();
            assert!(out.real_estimate <= 4 * MIB, "estimate {} covers the dead cells", out.real_estimate);
            assert_clean(&sim, &ev);
        }
    }

    #[test]
    fn wrap_with_an_unreadable_block_at_the_end_of_the_lead() {
        let real = 37 * MIB;
        let mut sim = SimVolume::new(256 * MIB, MIB, move |d| Some(d % real));
        sim.read_error = Some(Box::new(move |a| (a % real == 4 * MIB + 8192).then_some(io::ErrorKind::Other)));
        let (r, ev) = run_sim(&mut sim);
        let out = r.unwrap();
        assert!(out.real_estimate <= 4 * MIB, "estimate {} covers the unreadable block", out.real_estimate);
        assert_clean(&sim, &ev);
    }

    #[test]
    fn files_bigger_than_the_real_size() {
        // 20 MiB of user files on a drive that wraps at 16 MiB: the estimate is the real size, not the used space
        let real = 16 * MIB;
        let mut sim = SimVolume::new(256 * MIB, 20 * MIB, move |d| Some(d % real));
        let (r, ev) = run_sim(&mut sim);
        let out = r.unwrap();
        assert!(out.real_estimate <= real, "estimate {}", out.real_estimate);
        assert_clean(&sim, &ev);
    }

    #[test]
    fn wrap_holes_are_never_covered() {
        // seeded: wrap drives of various sizes with up to two dead regions; no estimate covers a dead cell
        let mut seed = 0x1234_5678u64;
        let mut rnd = |n: u64| {
            seed = pattern::splitmix(seed);
            seed % n
        };
        let q = 256 * 1024u64;
        for case in 0..16 {
            let real = (48 + rnd(200)) * q;
            let used = rnd(2) * MIB;
            let holes: Vec<(u64, u64)> = (0..1 + rnd(2))
                .map(|_| {
                    let a = used + rnd((real - used) / BLOCK as u64) * BLOCK as u64;
                    (a, a + (1 + rnd(16)) * 64 * 1024)
                })
                .collect();
            let first_dead = holes.iter().map(|h| h.0).min().unwrap().min(real);
            let hv = holes.clone();
            let mut sim = SimVolume::new(256 * MIB, used, move |d| {
                let a = d % real;
                (!hv.iter().any(|&(x, y)| (x..y).contains(&a))).then_some(a)
            });
            let (r, _) = run_sim(&mut sim);
            let out = r.unwrap();
            assert!(out.real_estimate <= first_dead, "case {case}: estimate {} holes {holes:?}", out.real_estimate);
        }
    }

    #[test]
    fn dir_volume_prepare_removes_a_leftover_folder() {
        let mount = std::env::temp_dir().join(format!("umc_full_left_{}", std::process::id()));
        std::fs::create_dir_all(mount.join(FOLDER)).unwrap();
        std::fs::write(mount.join(FOLDER).join("0001.umc"), b"old").unwrap();
        let mut vol = DirVolume::new(&mount);
        vol.prepare().unwrap();
        assert!(mount.join(FOLDER).is_dir());
        assert_eq!(std::fs::read_dir(mount.join(FOLDER)).unwrap().count(), 0, "the old files are gone");
        vol.cleanup();
        assert!(!mount.join(FOLDER).exists());
        std::fs::remove_dir_all(&mount).unwrap();
    }

    // ---- wrap estimate regressions (period measured exactly, damage mapped to cells)

    #[test]
    fn wrap_dead_cells_with_a_cache() {
        let real = 40 * MIB;
        let mut sim = SimVolume::new(256 * MIB, MIB, move |d| {
            let a = d % real;
            (!(9 * MIB / 4..5 * MIB / 2).contains(&a)).then_some(a)
        });
        sim.cache_blocks = 384;
        let (r, ev) = run_sim(&mut sim);
        let out = r.unwrap();
        assert!(out.real_estimate <= 9 * MIB / 4, "estimate {}", out.real_estimate);
        assert_clean(&sim, &ev);
    }

    #[test]
    fn wrap_two_holes_at_the_start() {
        let real = 33 * MIB;
        let mut sim = SimVolume::new(256 * MIB, 0, move |d| {
            let a = d % real;
            let dead = (512 << 10..552 << 10).contains(&a) || (6 * MIB..15 * MIB / 2).contains(&a);
            (!dead).then_some(a)
        });
        let (r, ev) = run_sim(&mut sim);
        let out = r.unwrap();
        assert!(out.real_estimate <= 512 << 10, "estimate {}", out.real_estimate);
        assert_clean(&sim, &ev);
    }

    #[test]
    fn wrap_small_period_two_holes() {
        let b = BLOCK as u64;
        let real = 768 * b;
        let mut sim = SimVolume::new(256 * MIB, 100 * b, move |d| {
            let a = d % real;
            let dead = (50 * b..60 * b).contains(&a) || (200 * b..210 * b).contains(&a);
            (!dead).then_some(a)
        });
        let (r, ev) = run_sim(&mut sim);
        let out = r.unwrap();
        assert!(out.real_estimate <= 200 * b, "estimate {} blocks", out.real_estimate / b);
        assert_clean(&sim, &ev);
    }

    #[test]
    fn wrap_with_files_slightly_bigger_than_the_real_size_and_a_cache() {
        let b = BLOCK as u64;
        let real = 6801 * b;
        let mut sim = SimVolume::new(256 * MIB, 7080 * b, move |d| Some(d % real));
        sim.cache_blocks = 423;
        let (r, ev) = run_sim(&mut sim);
        let out = r.unwrap();
        assert!(out.real_estimate <= real, "estimate {} blocks", out.real_estimate / b);
        assert_clean(&sim, &ev);
    }

    /// Largest E such that the device addresses [0, E) sit on distinct cells and every address from `used` on
    /// keeps data: what a partition from the start of the device could really use.
    fn true_real(map: &dyn Fn(u64) -> Option<u64>, used: u64, limit: u64) -> u64 {
        let mut seen = std::collections::HashSet::new();
        let mut d = 0;
        while d < limit {
            match map(d) {
                Some(c) => {
                    if !seen.insert(c) {
                        return d;
                    }
                }
                None if d >= used => return d,
                None => {}
            }
            d += BLOCK as u64;
        }
        limit
    }

    #[test]
    fn estimate_fuzz() {
        let mut seed = 0x5EED_1234u64;
        let mut rnd = move |n: u64| {
            seed = pattern::splitmix(seed);
            if n == 0 { 0 } else { seed % n }
        };
        let b = BLOCK as u64;
        let total = 256 * MIB;
        for case in 0..80 {
            let kind = rnd(3); // 0 wrap, 1 lost writes, 2 partial chip then wrap
            let real = (8 * MIB / b + rnd(40 * MIB / b)) * b;
            let space = real + (1 + rnd(16 * MIB / b)) * b;
            let used = match rnd(4) {
                0 => 0,
                1 => rnd(4 * MIB / b) * b,
                2 => rnd(real / b) * b,
                _ => rnd(2 * real / b) * b,
            };
            let holes: Vec<(u64, u64)> = (0..rnd(4))
                .map(|_| {
                    let a = rnd(real / b) * b;
                    (a, a + (1 + rnd(256)) * b * if rnd(3) == 0 { 16 } else { 1 })
                })
                .collect();
            let hv = holes.clone();
            let map = move |d: u64| -> Option<u64> {
                let a = match kind {
                    1 => (d < real).then_some(d)?,
                    2 => {
                        let a = d % space;
                        (a < real).then_some(a)?
                    }
                    _ => d % real,
                };
                (!hv.iter().any(|&(x, y)| (x..y).contains(&a))).then_some(a)
            };
            let cache = if rnd(3) == 0 { 1 + rnd(params().guard / b) as usize } else { 0 };
            let truth = true_real(&map, used, total);
            let mut sim = SimVolume::new(total, used, map);
            sim.limbo = if rnd(2) == 0 { 0 } else { 0xFF };
            sim.cache_blocks = cache;
            let (r, _) = run_sim(&mut sim);
            let Ok(out) = r else { continue };
            // Known limit: writing stopped before the wrap became visible (written <= R + cache), so no block of a
            // later position was seen and the estimate is `used + first bad`, which can exceed R by up to `used`.
            let period = if kind == 1 {
                u64::MAX
            } else if kind == 2 {
                space
            } else {
                real
            };
            let wrap_not_visible = kind != 1 && out.tested <= period.saturating_add(cache as u64 * b);
            let bound = if wrap_not_visible { truth + used } else { truth };
            assert!(
                out.real_estimate <= bound,
                "case {case} kind {kind} real {} space {} used {} holes {holes:?} cache {cache}: estimate {} > {} \
                 (true {}, tested {})",
                real / b,
                space / b,
                used / b,
                out.real_estimate / b,
                bound / b,
                truth / b,
                out.tested / b
            );
        }
    }

    #[test]
    fn inconclusive_when_a_sample_was_wrong_but_the_read_back_is_clean() {
        // the first sample read returns a wrong block once (a stray bit flip on the bus), the read-back is fine
        let mut sim = SimVolume::genuine(64 * MIB, MIB);
        sim.flip_reads = vec![1];
        let (r, ev) = run_sim(&mut sim);
        assert!(matches!(r, Err(Error::Inconclusive)));
        assert_clean(&sim, &ev);
    }

    #[test]
    fn a_panicking_helper_is_an_error() {
        pipe::panic_next_helpers(true);
        let mut sim = SimVolume::genuine(64 * MIB, MIB);
        let (r, _) = run_sim(&mut sim);
        pipe::panic_next_helpers(false);
        assert!(matches!(r, Err(Error::Io(_))), "{r:?}");
        assert!(sim.cleaned);
    }
}
