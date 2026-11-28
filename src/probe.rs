//! Quick real-capacity probe of a whole device, in the spirit of f3probe: a fake 2 TB drive answers in a minute
//! instead of the hours needed to fill it.
//!
//! How a fake drive lies: its controller reports a big size, but only the first `R` bytes are real flash.
//! Addresses past `R` either wrap around onto the real part (usually by dropping high address bits, i.e. modulo a
//! power of two), or go nowhere (writes are lost, reads give zeros / 0xFF / stale data). Some also keep the last
//! writes in a small cache, so a block read right after its write looks fine.
//!
//! The probe:
//! 1. **Coarse round.** About 1000 points spread over the whole device on a power-of-two grid. For every point
//!    `p` also its likely wrap-around targets `p mod 2^j` are written. Writes go from the highest address to the
//!    lowest, so a lower (real) address written later overwrites a cell that a higher (fake) address maps to:
//!    reading the higher one back then gives someone else's data. Then a 16 MiB "flush" area at the start of the
//!    device is written to push the points out of any controller cache, and every point is read back and checked
//!    ([`crate::pattern`]: the data depends on the address, so a block from elsewhere never matches).
//! 2. **Refine rounds.** Between the last good and the first bad point a finer grid (64 points) is tested the same
//!    way, until the boundary is known to 1 MiB.
//!
//! **Nothing is lost**: no block is written before its old content has been read and kept (a block that cannot be
//! read is not written at all), and at the end (also after a cancel or an error) everything is written back in
//! reverse order. Reverse order restores the exact old state of every cell, even when several addresses share one
//! cell. Before the first write the probe also keeps a digest of every block of the first 16 MiB, of the last 1 MiB
//! and of every block of the coarse round; after the write-back it asks the drive to flush its cache, reads these
//! blocks again and compares ([`RestoreCheck`]). A matching re-read shows only that the drive answers with the old
//! data now: it may come from the controller's memory, and only a re-plug followed by a new read shows what is on
//! the flash. Only a power cut or unplugging in the middle can leave damage that the probe cannot report.
//!
//! **A drive that stops answering gets no verdict.** A point whose data came back different (a mismatch) is not the
//! same as one whose write or read returned an error. When the start of the drive stops answering, the run ends as
//! [`Abort::Unresponsive`] and never as "keeps nothing".

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::dio::{self, AccessCause};
use crate::pattern::{self, BLOCK};

const MIB: u64 = 1 << 20;
/// Most failures and bad blocks listed one by one; the others are only counted.
pub const MAX_LISTED: usize = 32;
/// After this many failed operations in a row the start of the drive is read again at once.
const FAILED_IN_A_ROW: u32 = 64;
/// The read-back check stops after this many failed reads in a row.
const UNREADABLE_IN_A_ROW: u32 = 16;

/// Whole device as an array of bytes. Offsets and sizes are multiples of [`BLOCK`].
pub trait Device {
    fn size(&self) -> u64;
    fn read(&mut self, buf: &mut [u8], off: u64) -> io::Result<()>;
    fn write(&mut self, buf: &[u8], off: u64) -> io::Result<()>;
    /// Asks the drive to commit its write cache.
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// Area at the start written after the points to push them out of the drive's cache. Points are never
    /// placed inside it.
    pub flush: u64,
    pub coarse_points: u64,
    pub fine_points: u64,
    /// The boundary is searched to this precision.
    pub resolution: u64,
}

impl Default for Params {
    fn default() -> Self {
        Params { flush: 16 << 20, coarse_points: 1024, fine_points: 64, resolution: 1 << 20 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    /// All points kept their data: the size is real.
    Genuine,
    /// Everything up to `real` works, everything past it is lost: a fake-capacity drive.
    Fake,
    /// Lost points are mixed with good ones past the first bad point: worn out or failing flash.
    Damaged,
    /// Even the first point lost its data (or every write was ignored): the drive does not store data.
    Broken,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub verdict: Verdict,
    /// Size the drive reports.
    pub reported: u64,
    /// Bytes from the start that really keep data (1 MiB precision); equals `reported` for a genuine drive.
    pub real: u64,
    pub points: usize,
    pub bad_points: usize,
    pub seconds: f64,
    /// Blocks whose old content could not be written back (0 normally).
    pub restore_failures: usize,
    /// Points whose data came back different.
    #[serde(default)]
    pub mismatches: usize,
    /// Points where the old-content read, the write or the read-back returned an error.
    #[serde(default)]
    pub io_errors: usize,
    /// 4 KiB blocks that were not written because their old content could not be read.
    #[serde(default)]
    pub unsaved: usize,
    #[serde(default)]
    pub restore_check: RestoreCheck,
    #[serde(default)]
    pub flush: FlushResult,
}

#[derive(Debug)]
pub enum Abort {
    Cancelled,
    /// The system refused access (no rights, the volume is still mounted, write-protected switch).
    Access(AccessFailure),
    TooSmall,
    /// The drive stopped answering: there is no verdict.
    Unresponsive,
}

/// An access error on the raw device: why, in which operation and with which error.
#[derive(Clone, Debug)]
pub struct AccessFailure {
    pub cause: AccessCause,
    pub op: Op,
    pub errno: Option<i32>,
    pub text: String,
}

impl AccessFailure {
    /// None: the error is no access error (a media error, for example).
    pub fn new(e: &io::Error, op: Op) -> Option<Self> {
        dio::access_cause(e, true).map(|cause| AccessFailure {
            cause,
            op,
            errno: e.raw_os_error(),
            text: e.to_string(),
        })
    }
}

fn check_access(e: &io::Error, op: Op) -> Result<(), Abort> {
    match AccessFailure::new(e, op) {
        Some(a) => Err(Abort::Access(a)),
        None => Ok(()),
    }
}

/// The operation a failure belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Op {
    Open,
    ReadOld,
    Write,
    ReadBack,
    Baseline,
    Verify,
    Liveness,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PointClass {
    Good,
    /// Read back fine, but the data was different.
    Mismatch,
    /// A write or a read returned an error (not an access error).
    IoError,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Failure {
    pub offset: u64,
    pub op: Op,
    #[serde(default)]
    pub errno: Option<i32>,
    pub class: PointClass,
}

/// What reading the start of the drive again says (the first MiB of the flush area, written with a known pattern).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Liveness {
    Ok,
    Mismatch,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// The first point is good: search on.
    Continue,
    /// The first point came back different and the start still answers: the drive keeps nothing.
    Broken,
    Unresponsive,
}

/// What the first point of the coarse round and the start of the drive say together.
pub fn decide(first: PointClass, liveness: Liveness) -> Decision {
    match (first, liveness) {
        (PointClass::IoError, _) | (_, Liveness::Error) => Decision::Unresponsive,
        (PointClass::Mismatch, _) => Decision::Broken,
        (PointClass::Good, _) => Decision::Continue,
    }
}

/// The answer to the cache flush after the write-back.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FlushResult {
    #[default]
    Ok,
    /// The drive or the system has no cache flush: not an error.
    Unsupported,
    Failed(String),
}

fn flush_result(r: io::Result<()>) -> FlushResult {
    let Err(e) = r else { return FlushResult::Ok };
    #[cfg(unix)]
    let no_support = e.raw_os_error().is_some_and(|c| c == libc::ENOTTY || c == libc::ENOTSUP || c == libc::EOPNOTSUPP);
    // ERROR_INVALID_FUNCTION
    #[cfg(windows)]
    let no_support = e.raw_os_error() == Some(1);
    #[cfg(not(any(unix, windows)))]
    let no_support = false;
    if no_support || e.kind() == io::ErrorKind::Unsupported {
        FlushResult::Unsupported
    } else {
        FlushResult::Failed(e.to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BadKind {
    Mismatch,
    Unreadable,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BadBlock {
    pub offset: u64,
    pub kind: BadKind,
}

/// The re-read of the blocks that have a digest from before the first write. Empty when nothing was written.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RestoreCheck {
    /// Blocks read again and compared.
    pub checked: usize,
    pub mismatched: usize,
    pub unreadable: usize,
    /// Blocks of the coarse round and of the last MiB that could not be read before the first write.
    pub no_baseline: usize,
    /// Blocks written by the refine rounds: put back, but there is no clean digest to compare them with.
    pub not_reread: usize,
    /// A bad block in the first MiB (the partition table) or in the last MiB (the backup table).
    pub table_area: bool,
    /// The first bad blocks.
    pub first: Vec<BadBlock>,
}

impl RestoreCheck {
    /// No block read back different and every block read back.
    pub fn confirmed(&self) -> bool {
        self.mismatched == 0 && self.unreadable == 0
    }

    fn bad(&mut self, offset: u64, kind: BadKind, size: u64) {
        match kind {
            BadKind::Mismatch => self.mismatched += 1,
            BadKind::Unreadable => self.unreadable += 1,
        }
        self.table_area |= offset < MIB || offset >= size.saturating_sub(MIB);
        if self.first.len() < MAX_LISTED {
            self.first.push(BadBlock { offset, kind });
        }
    }
}

/// The coarse round's grid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Grid {
    pub step: u64,
    pub points: usize,
    /// Points plus their wrap-around targets.
    pub coarse_blocks: usize,
}

/// What a run counted, whether it ended with a verdict or not.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub mismatches: usize,
    pub io_errors: usize,
    pub unsaved: usize,
    pub restore_failures: usize,
    pub failures: Vec<Failure>,
    pub failures_not_listed: usize,
    pub grid: Grid,
}

/// The end of a run: the result, and what is known about the writes whatever the result.
#[derive(Debug)]
pub struct ProbeRun {
    pub result: Result<Outcome, Abort>,
    pub stats: Stats,
    pub restore_check: RestoreCheck,
    pub flush: FlushResult,
}

/// Progress: overall fraction 0..1 and the stage key for the window (`prCoarse`, `prRefine`, `prRestore`).
pub type Progress<'a> = dyn FnMut(f64, &'static str) + 'a;

/// What happens in the run, for the event log: stage name, whether it went well, details.
pub type Events<'a> = dyn FnMut(&'static str, bool, Value) + 'a;

fn abort_info(a: &Abort) -> Value {
    match a {
        Abort::Cancelled => json!({"abort": "cancelled"}),
        Abort::TooSmall => json!({"abort": "tooSmall"}),
        Abort::Unresponsive => json!({"abort": "unresponsive"}),
        Abort::Access(a) => json!({"abort": "access", "cause": a.cause, "op": a.op, "errno": a.errno}),
    }
}

/// Writes with a copy of the old content kept for [`Undo::restore`].
#[derive(Default)]
struct Undo {
    entries: Vec<(u64, Vec<u8>)>,
}

/// How a write of [`Undo::write`] went. Access errors are an [`Abort`] instead.
enum Written {
    Saved,
    /// The old content could not be read: nothing was written.
    Unreadable(io::Error),
    /// The write failed; the old content is kept and will be put back.
    WriteFailed(io::Error),
}

impl Undo {
    /// Writes only over content that was read and kept. The copy is kept before the write, so a write that fails
    /// half-way is put back too.
    fn write(&mut self, dev: &mut dyn Device, data: &[u8], off: u64) -> Result<Written, Abort> {
        let mut old = vec![0u8; data.len()];
        if let Err(e) = dev.read(&mut old, off) {
            check_access(&e, Op::ReadOld)?;
            return Ok(Written::Unreadable(e));
        }
        self.entries.push((off, old));
        match dev.write(data, off) {
            Ok(()) => Ok(Written::Saved),
            Err(e) => {
                check_access(&e, Op::Write)?;
                Ok(Written::WriteFailed(e))
            }
        }
    }

    fn restore(&mut self, dev: &mut dyn Device, progress: &mut Progress) -> usize {
        let total = self.entries.len().max(1);
        let mut failures = 0;
        for (i, (off, old)) in self.entries.drain(..).rev().enumerate() {
            if dev.write(&old, off).is_err() {
                failures += 1;
            }
            if i % 256 == 0 {
                progress(0.95 + 0.03 * i as f64 / total as f64, "prRestore");
            }
        }
        failures
    }
}

fn prev_pow2(x: u64) -> u64 {
    if x == 0 { 0 } else { 1u64 << (63 - x.leading_zeros()) }
}

/// The coarse round's points and the grid step.
fn coarse_points(params: &Params, size: u64) -> (u64, Vec<u64>) {
    let Params { flush, coarse_points, resolution, .. } = *params;
    let step = prev_pow2((size - flush) / coarse_points).max(resolution);
    let mut points: Vec<u64> =
        (1..).map(|k| k * step).skip_while(|&p| p < flush).take_while(|&p| p + BLOCK as u64 <= size).collect();
    let last = size - BLOCK as u64;
    if points.last() != Some(&last) {
        points.push(last);
    }
    (step, points)
}

/// The blocks a round writes: the points and their likely wrap-around targets.
fn test_set(flush: u64, points: &[u64]) -> BTreeSet<u64> {
    let mut set: BTreeSet<u64> = points.iter().copied().collect();
    for &p in points {
        // wrap-around targets of p for every power-of-two modulus above the flush area
        for j in (flush.trailing_zeros() + 1)..64 {
            let m = 1u64 << j;
            if m > p {
                break;
            }
            let a = p % m;
            if a >= flush {
                set.insert(a);
            }
        }
    }
    set
}

fn digest(block: &[u8]) -> [u8; 32] {
    Sha256::digest(block).into()
}

/// A verdict with its numbers: verdict, real size, points tested, bad points.
type Found = (Verdict, u64, usize, usize);

struct Run<'a, 'p, 'e> {
    dev: &'a mut dyn Device,
    params: Params,
    salt: u64,
    round: u64,
    undo: Undo,
    cancel: &'a AtomicBool,
    progress: &'a mut Progress<'p>,
    events: &'a mut Events<'e>,
    stats: Stats,
    /// Digest of every block of the baseline, taken before the first write.
    baseline: BTreeMap<u64, [u8; 32]>,
    /// Blocks of the coarse round and of the last MiB that could not be read for the baseline.
    no_baseline: BTreeSet<u64>,
    failed_in_a_row: u32,
}

impl Run<'_, '_, '_> {
    fn check_cancel(&self) -> Result<(), Abort> {
        if self.cancel.load(Ordering::Relaxed) { Err(Abort::Cancelled) } else { Ok(()) }
    }

    /// Lists a failure (the first [`MAX_LISTED`] one by one, the rest counted).
    fn record(&mut self, offset: u64, op: Op, errno: Option<i32>, class: PointClass) {
        if self.stats.failures.len() < MAX_LISTED {
            let f = Failure { offset, op, errno, class };
            (self.events)("failure", false, json!(f));
            self.stats.failures.push(f);
        } else {
            self.stats.failures_not_listed += 1;
        }
    }

    fn io_failure(&mut self, offset: u64, op: Op, e: &io::Error) {
        self.record(offset, op, e.raw_os_error(), PointClass::IoError);
    }

    /// Counts failed operations in a row: after many of them the start of the drive is read at once, so a drive
    /// that hangs with slow timeouts does not take hours.
    fn operation(&mut self, ok: bool, salt: u64) -> Result<(), Abort> {
        if ok {
            self.failed_in_a_row = 0;
            return Ok(());
        }
        self.failed_in_a_row += 1;
        if self.failed_in_a_row >= FAILED_IN_A_ROW {
            self.failed_in_a_row = 0;
            if self.liveness(salt)? == Liveness::Error {
                return Err(Abort::Unresponsive);
            }
        }
        Ok(())
    }

    /// Reads the first MiB of the flush area again and compares it with the flush pattern of the round `salt`.
    fn liveness(&mut self, salt: u64) -> Result<Liveness, Abort> {
        let mut buf = vec![0u8; self.params.flush.min(MIB) as usize];
        match self.dev.read(&mut buf, 0) {
            Ok(()) if pattern::count_bad(&buf, 0, salt ^ 0xF1) == 0 => Ok(Liveness::Ok),
            Ok(()) => Ok(Liveness::Mismatch),
            Err(e) => {
                check_access(&e, Op::Liveness)?;
                self.io_failure(0, Op::Liveness, &e);
                Ok(Liveness::Error)
            }
        }
    }

    fn round_salt(&self, round: u64) -> u64 {
        self.salt ^ round.wrapping_mul(0x9E37_79B9_7F4A_7C15)
    }

    /// Digests of the start of the drive (the flush area), of the last MiB and of the coarse round's blocks, taken
    /// before the first write. A block of the start that cannot be read ends the run before anything is written.
    fn baseline(&mut self, size: u64, set: &BTreeSet<u64>) -> Result<(), Abort> {
        let flush = self.params.flush;
        let salt = self.round_salt(1);
        let tail: Vec<u64> = (size - size.min(MIB)..size).step_by(BLOCK).collect();
        let steps = (flush / BLOCK as u64 + tail.len() as u64 + set.len() as u64).max(1) as f64;
        let mut done = 0u64;
        let mut chunk = vec![0u8; MIB as usize];
        let mut block = vec![0u8; BLOCK];
        let mut bad_start = false;
        let mut off = 0;
        while off < flush {
            self.check_cancel()?;
            let len = (flush - off).min(MIB) as usize;
            match self.dev.read(&mut chunk[..len], off) {
                Ok(()) => {
                    self.operation(true, salt)?;
                    for (i, b) in chunk[..len].as_chunks::<BLOCK>().0.iter().enumerate() {
                        self.baseline.insert(off + (i * BLOCK) as u64, digest(b));
                    }
                }
                Err(e) => {
                    check_access(&e, Op::Baseline)?;
                    self.operation(false, salt)?;
                    // block by block, to find the exact ones
                    for addr in (off..off + len as u64).step_by(BLOCK) {
                        match self.dev.read(&mut block, addr) {
                            Ok(()) => {
                                self.operation(true, salt)?;
                                self.baseline.insert(addr, digest(&block));
                            }
                            Err(e) => {
                                check_access(&e, Op::Baseline)?;
                                self.io_failure(addr, Op::Baseline, &e);
                                bad_start = true;
                                self.operation(false, salt)?;
                            }
                        }
                    }
                }
            }
            off += len as u64;
            done += len as u64 / BLOCK as u64;
            (self.progress)(0.05 * done as f64 / steps, "prPrepare");
        }
        if bad_start {
            return Err(Abort::Unresponsive);
        }
        let rest: BTreeSet<u64> = tail.into_iter().chain(set.iter().copied()).collect();
        for (i, addr) in rest.into_iter().enumerate() {
            if i % 16 == 0 {
                self.check_cancel()?;
                (self.progress)((0.05 * (done + i as u64) as f64 / steps).min(0.05), "prPrepare");
            }
            if self.baseline.contains_key(&addr) {
                continue;
            }
            match self.dev.read(&mut block, addr) {
                Ok(()) => {
                    self.baseline.insert(addr, digest(&block));
                    self.operation(true, salt)?;
                }
                Err(e) => {
                    check_access(&e, Op::Baseline)?;
                    self.no_baseline.insert(addr);
                    self.io_failure(addr, Op::Baseline, &e);
                    self.operation(false, salt)?;
                }
            }
        }
        Ok(())
    }

    /// Tests the points (ascending): the class of every point and what the start of the drive says afterwards.
    fn test(
        &mut self,
        points: &[u64],
        from: f64,
        to: f64,
        stage: &'static str,
    ) -> Result<(Vec<PointClass>, Liveness), Abort> {
        let r = self.round_inner(points, from, to, stage);
        let counts = |extra: Value, stats: &Stats, round: u64| {
            let mut info = json!({"n": round, "points": points.len(), "mismatches": stats.mismatches,
                "ioErrors": stats.io_errors, "unsaved": stats.unsaved});
            if let (Some(m), Some(e)) = (info.as_object_mut(), extra.as_object()) {
                m.extend(e.clone());
            }
            info
        };
        match r {
            Ok((classes, live)) => {
                let bad = classes.iter().filter(|c| **c != PointClass::Good).count();
                (self.events)("round", true, counts(json!({"bad": bad}), &self.stats, self.round));
                (self.events)("liveness", live != Liveness::Error, json!({"result": live}));
                Ok((classes, live))
            }
            Err(a) => {
                (self.events)("round", false, counts(abort_info(&a), &self.stats, self.round));
                Err(a)
            }
        }
    }

    fn round_inner(
        &mut self,
        points: &[u64],
        from: f64,
        to: f64,
        stage: &'static str,
    ) -> Result<(Vec<PointClass>, Liveness), Abort> {
        self.round += 1;
        let salt = self.round_salt(self.round);
        let flush = self.params.flush;
        let set = test_set(flush, points);
        let is_point: BTreeSet<u64> = points.iter().copied().collect();
        let flush_chunks = flush.div_ceil(MIB);
        let steps = (set.len() as u64 + flush_chunks * 256 + points.len() as u64).max(1) as f64;
        let mut done = 0u64;
        let mut tick = |run: &mut Self, n: u64| {
            done += n;
            (run.progress)(from + (to - from) * (done as f64 / steps).min(1.0), stage);
        };

        let mut block = vec![0u8; BLOCK];
        // blocks that could not be saved or written: points among them are I/O errors
        let mut failed: BTreeSet<u64> = BTreeSet::new();
        for (i, &addr) in set.iter().rev().enumerate() {
            pattern::fill(&mut block, addr, salt);
            let (error, read_ok) = match self.undo.write(self.dev, &block, addr)? {
                Written::Saved => (None, true),
                Written::Unreadable(e) => {
                    self.stats.unsaved += 1;
                    (Some((Op::ReadOld, e)), false)
                }
                Written::WriteFailed(e) => (Some((Op::Write, e)), true),
            };
            let failure = error.is_some();
            if let Some((op, e)) = error {
                failed.insert(addr);
                if is_point.contains(&addr) {
                    self.stats.io_errors += 1;
                }
                self.io_failure(addr, op, &e);
            }
            // the old-content read, then (when it worked) the write
            if read_ok {
                self.operation(true, salt)?;
            }
            self.operation(!failure, salt)?;
            if i % 16 == 0 {
                self.check_cancel()?;
                tick(self, 16);
            }
        }
        let mut chunk = vec![0u8; MIB as usize];
        let mut off = 0;
        while off < flush {
            let len = (flush - off).min(MIB) as usize;
            pattern::fill(&mut chunk[..len], off, salt ^ 0xF1);
            match self.undo.write(self.dev, &chunk[..len], off)? {
                Written::Saved => self.operation(true, salt)?,
                // the start of the drive cannot be read: it is not written, and nothing can be told about the drive
                Written::Unreadable(e) => {
                    self.stats.unsaved += len / BLOCK;
                    self.io_failure(off, Op::ReadOld, &e);
                    return Err(Abort::Unresponsive);
                }
                Written::WriteFailed(e) => {
                    self.io_failure(off, Op::Write, &e);
                    self.operation(false, salt)?;
                }
            }
            off += len as u64;
            self.check_cancel()?;
            tick(self, 256);
        }
        let mut classes = Vec::with_capacity(points.len());
        for (i, &p) in points.iter().enumerate() {
            let class = if failed.contains(&p) {
                PointClass::IoError
            } else {
                match self.dev.read(&mut block, p) {
                    Ok(()) => {
                        let class = if pattern::count_bad(&block, p, salt) == 0 {
                            PointClass::Good
                        } else {
                            self.stats.mismatches += 1;
                            self.record(p, Op::ReadBack, None, PointClass::Mismatch);
                            PointClass::Mismatch
                        };
                        self.operation(true, salt)?;
                        class
                    }
                    Err(e) => {
                        check_access(&e, Op::ReadBack)?;
                        self.stats.io_errors += 1;
                        self.io_failure(p, Op::ReadBack, &e);
                        self.operation(false, salt)?;
                        PointClass::IoError
                    }
                }
            };
            classes.push(class);
            if i % 16 == 0 {
                self.check_cancel()?;
                tick(self, 16);
            }
        }
        let live = self.liveness(salt)?;
        if live == Liveness::Error {
            return Err(Abort::Unresponsive);
        }
        Ok((classes, live))
    }

    fn search(&mut self, size: u64, points: &[u64]) -> Result<Found, Abort> {
        let Params { fine_points, resolution, .. } = self.params;
        let (classes, live) = self.test(points, 0.05, 0.6, "prCoarse")?;
        let mut tested = points.len();
        let mut bad = classes.iter().filter(|c| **c != PointClass::Good).count();
        match decide(classes[0], live) {
            Decision::Unresponsive => return Err(Abort::Unresponsive),
            Decision::Broken => return Ok((Verdict::Broken, 0, tested, bad)),
            Decision::Continue => {}
        }
        let ok: Vec<bool> = classes.iter().map(|c| *c == PointClass::Good).collect();
        let Some(first_bad) = ok.iter().position(|g| !g) else {
            return Ok((Verdict::Genuine, size, tested, bad));
        };
        let mut damaged = ok[first_bad..].iter().any(|g| *g);
        let (mut lo, mut hi) = (points[first_bad - 1], points[first_bad]);

        let mut progress_at = 0.6;
        while hi - lo > resolution {
            let step = prev_pow2((hi - lo) / fine_points).max(resolution);
            let fine: Vec<u64> = (1..).map(|k| lo + k * step).take_while(|&p| p < hi).collect();
            if fine.is_empty() {
                break;
            }
            let next = progress_at + (0.95 - progress_at) * 0.5;
            let (classes, _) = self.test(&fine, progress_at, next, "prRefine")?;
            let ok: Vec<bool> = classes.iter().map(|c| *c == PointClass::Good).collect();
            progress_at = next;
            tested += fine.len();
            bad += ok.iter().filter(|g| !**g).count();
            match ok.iter().position(|g| !g) {
                Some(j) => {
                    damaged |= ok[j..].iter().any(|g| *g);
                    hi = fine[j];
                    if j > 0 {
                        lo = fine[j - 1];
                    }
                }
                None => lo = *fine.last().expect("not empty"),
            }
            if step == resolution {
                break;
            }
        }
        let real = lo / resolution * resolution;
        Ok((if damaged { Verdict::Damaged } else { Verdict::Fake }, real, tested, bad))
    }

    /// The baseline, then the search.
    fn run(&mut self, size: u64) -> Result<Found, Abort> {
        let (step, points) = coarse_points(&self.params, size);
        let set = test_set(self.params.flush, &points);
        self.stats.grid = Grid { step, points: points.len(), coarse_blocks: set.len() };
        let read = self.baseline(size, &set);
        let info = |extra: Value, run: &Self| {
            let mut info = json!({"step": step, "points": points.len(), "coarseBlocks": set.len(),
                "blocks": run.baseline.len(), "noBaseline": run.no_baseline.len()});
            if let (Some(m), Some(e)) = (info.as_object_mut(), extra.as_object()) {
                m.extend(e.clone());
            }
            info
        };
        match read {
            Ok(()) => {
                let info = info(json!({}), self);
                (self.events)("baseline", true, info);
            }
            Err(a) => {
                let info = info(abort_info(&a), self);
                (self.events)("baseline", false, info);
                return Err(a);
            }
        }
        self.search(size, &points)
    }
}

/// Reads every block that has a digest from before the first write again and compares.
fn verify(
    dev: &mut dyn Device,
    size: u64,
    flush: u64,
    baseline: &BTreeMap<u64, [u8; 32]>,
    mut check: RestoreCheck,
    progress: &mut Progress,
) -> RestoreCheck {
    let blocks: Vec<(u64, &[u8; 32])> = baseline.iter().map(|(a, d)| (*a, d)).collect();
    let mut chunk = vec![0u8; MIB as usize];
    // below this address the blocks of a chunk that failed are read one by one
    let mut one_by_one_until = 0;
    let (mut failed_in_a_row, mut i, mut reported) = (0u32, 0usize, 0usize);
    while i < blocks.len() {
        if failed_in_a_row >= UNREADABLE_IN_A_ROW {
            for (addr, _) in &blocks[i..] {
                check.bad(*addr, BadKind::Unreadable, size);
            }
            break;
        }
        if i >= reported {
            progress(0.98 + 0.02 * i as f64 / blocks.len() as f64, "prRestore");
            reported = i + 256;
        }
        let (addr, expected) = blocks[i];
        // the start of the drive is read in whole chunks
        if addr < flush && addr.is_multiple_of(MIB) && addr >= one_by_one_until {
            let len = (flush - addr).min(MIB);
            let n = (len / BLOCK as u64) as usize;
            if blocks.get(i + n - 1).is_some_and(|b| b.0 == addr + len - BLOCK as u64) {
                if dev.read(&mut chunk[..len as usize], addr).is_ok() {
                    failed_in_a_row = 0;
                    for (b, (a, d)) in chunk[..len as usize].as_chunks::<BLOCK>().0.iter().zip(&blocks[i..i + n]) {
                        check.checked += 1;
                        if digest(b) != **d {
                            check.bad(*a, BadKind::Mismatch, size);
                        }
                    }
                    i += n;
                } else {
                    one_by_one_until = addr + len;
                }
                continue;
            }
        }
        let mut block = [0u8; BLOCK];
        if dev.read(&mut block, addr).is_ok() {
            failed_in_a_row = 0;
            check.checked += 1;
            if digest(&block) != *expected {
                check.bad(addr, BadKind::Mismatch, size);
            }
        } else {
            failed_in_a_row += 1;
            check.bad(addr, BadKind::Unreadable, size);
        }
        i += 1;
    }
    check
}

/// Runs the probe; the device's old content is written back and checked before returning, whatever the result.
pub fn probe(
    dev: &mut dyn Device,
    params: Params,
    cancel: &AtomicBool,
    progress: &mut Progress,
    events: &mut Events,
) -> ProbeRun {
    let started = Instant::now();
    let reported = dev.size();
    let size = reported / BLOCK as u64 * BLOCK as u64;
    if size < params.flush + 4 * params.resolution {
        return ProbeRun {
            result: Err(Abort::TooSmall),
            stats: Stats::default(),
            restore_check: RestoreCheck::default(),
            flush: FlushResult::default(),
        };
    }
    let mut run = Run {
        dev,
        params,
        salt: pattern::new_salt(),
        round: 0,
        undo: Undo::default(),
        cancel,
        progress,
        events,
        stats: Stats::default(),
        baseline: BTreeMap::new(),
        no_baseline: BTreeSet::new(),
        failed_in_a_row: 0,
    };
    let result = run.run(size);
    let Run { dev, mut undo, progress, events, mut stats, baseline, no_baseline, .. } = run;
    let mut restore_check = RestoreCheck::default();
    let mut flush = FlushResult::default();
    // without a write there is nothing to put back and nothing to check
    if !undo.entries.is_empty() {
        let not_reread: BTreeSet<u64> = undo
            .entries
            .iter()
            .flat_map(|(off, old)| (*off..off + old.len() as u64).step_by(BLOCK))
            .filter(|a| !baseline.contains_key(a) && !no_baseline.contains(a))
            .collect();
        let writes = undo.entries.len();
        stats.restore_failures = undo.restore(dev, progress);
        events(
            "restore",
            stats.restore_failures == 0,
            json!({"writes": writes, "restoreFailures": stats.restore_failures}),
        );
        flush = flush_result(dev.flush());
        events("flush", !matches!(flush, FlushResult::Failed(_)), json!({"result": flush}));
        let start = RestoreCheck { no_baseline: no_baseline.len(), not_reread: not_reread.len(), ..Default::default() };
        restore_check = verify(dev, size, params.flush, &baseline, start, progress);
        events("verify", restore_check.confirmed(), json!(restore_check));
    }
    progress(1.0, "prRestore");
    let result = result.map(|(verdict, real, points, bad_points)| Outcome {
        verdict,
        reported,
        real,
        points,
        bad_points,
        seconds: started.elapsed().as_secs_f64(),
        restore_failures: stats.restore_failures,
        mismatches: stats.mismatches,
        io_errors: stats.io_errors,
        unsaved: stats.unsaved,
        restore_check: restore_check.clone(),
        flush: flush.clone(),
    });
    ProbeRun { result, stats, restore_check, flush }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, VecDeque};

    const GB: u64 = 1_000_000_000;
    const GIB: u64 = 1 << 30;
    const MIB: u64 = 1 << 20;

    /// Simulated drive: `map` sends a logical block to a physical cell or nowhere (writes lost, reads give
    /// `limbo`). Optional cache of the last written blocks answers reads first.
    struct Sim {
        reported: u64,
        map: Box<dyn Fn(u64) -> Option<u64>>,
        cells: HashMap<u64, Vec<u8>>,
        limbo: u8,
        cache: VecDeque<(u64, Vec<u8>)>,
        cache_blocks: usize,
        bad_reads: Box<dyn Fn(u64) -> bool>,
        /// Writes of these blocks fail with a media error.
        fail_writes_at: Box<dyn Fn(u64) -> bool>,
        /// Reads of these blocks fail once something has been written / once the cache has been flushed.
        bad_reads_after_write: Box<dyn Fn(u64) -> bool>,
        bad_reads_after_flush: Box<dyn Fn(u64) -> bool>,
        /// Every operation after the Nth fails; `heals_after` more operations later the drive answers again.
        fail_after: Option<usize>,
        heals_after: Option<usize>,
        /// A write that starts here stores only its first block and fails, once.
        half_write_at: Option<u64>,
        /// Block 0 keeps what the first write put there: later writes are silently lost.
        block0_sticks: bool,
        block0_written: bool,
        flush_error: Option<i32>,
        flushed: bool,
        /// Operations, write calls and read calls made so far.
        ops: usize,
        writes: usize,
        reads: usize,
        reads_after_flush: usize,
        /// The number of operations made when write call number N began.
        ops_at_write: Vec<usize>,
        /// Every block address a write was attempted on.
        written: BTreeSet<u64>,
    }

    fn original(cell: u64) -> Vec<u8> {
        let mut b = vec![0u8; BLOCK];
        pattern::fill(&mut b, cell, 777);
        b
    }

    impl Sim {
        fn new(reported: u64, map: impl Fn(u64) -> Option<u64> + 'static) -> Self {
            Sim {
                reported,
                map: Box::new(map),
                cells: HashMap::new(),
                limbo: 0,
                cache: VecDeque::new(),
                cache_blocks: 0,
                bad_reads: Box::new(|_| false),
                fail_writes_at: Box::new(|_| false),
                bad_reads_after_write: Box::new(|_| false),
                bad_reads_after_flush: Box::new(|_| false),
                fail_after: None,
                heals_after: None,
                half_write_at: None,
                block0_sticks: false,
                block0_written: false,
                flush_error: None,
                flushed: false,
                ops: 0,
                writes: 0,
                reads: 0,
                reads_after_flush: 0,
                ops_at_write: Vec::new(),
                written: BTreeSet::new(),
            }
        }

        fn operation(&mut self) -> io::Result<()> {
            self.ops += 1;
            let failing = self
                .fail_after
                .is_some_and(|n| self.ops > n && self.heals_after.is_none_or(|more| self.ops <= n + more));
            if failing { Err(io::Error::other("I/O error")) } else { Ok(()) }
        }

        /// Every cell the probe touched holds its original content again.
        fn intact(&self) -> bool {
            self.cells.iter().all(|(c, d)| *d == original(*c))
        }
    }

    impl Device for Sim {
        fn size(&self) -> u64 {
            self.reported
        }
        fn read(&mut self, buf: &mut [u8], off: u64) -> io::Result<()> {
            self.reads += 1;
            self.reads_after_flush += usize::from(self.flushed);
            self.operation()?;
            for (i, block) in buf.as_chunks_mut::<BLOCK>().0.iter_mut().enumerate() {
                let addr = off + (i * BLOCK) as u64;
                if (self.bad_reads)(addr)
                    || (self.writes > 0 && (self.bad_reads_after_write)(addr))
                    || (self.flushed && (self.bad_reads_after_flush)(addr))
                {
                    return Err(io::Error::other("I/O error"));
                }
                if let Some((_, d)) = self.cache.iter().rev().find(|(a, _)| *a == addr) {
                    block.copy_from_slice(d);
                    continue;
                }
                match (self.map)(addr) {
                    Some(c) => block.copy_from_slice(&self.cells.get(&c).cloned().unwrap_or_else(|| original(c))),
                    None => block.fill(self.limbo),
                }
            }
            Ok(())
        }
        fn flush(&mut self) -> io::Result<()> {
            self.flushed = true;
            match self.flush_error {
                Some(code) => Err(io::Error::from_raw_os_error(code)),
                None => Ok(()),
            }
        }
        fn write(&mut self, buf: &[u8], off: u64) -> io::Result<()> {
            self.ops_at_write.push(self.ops);
            self.writes += 1;
            self.written.extend((0..buf.len() / BLOCK).map(|i| off + (i * BLOCK) as u64));
            self.operation()?;
            let half = self.half_write_at == Some(off) && buf.len() > BLOCK;
            if half {
                self.half_write_at = None;
            }
            for (i, block) in buf.as_chunks::<BLOCK>().0.iter().enumerate() {
                let addr = off + (i * BLOCK) as u64;
                if (self.fail_writes_at)(addr) || (half && i > 0) {
                    return Err(io::Error::other("I/O error"));
                }
                if self.block0_sticks && addr == 0 {
                    if self.block0_written {
                        continue;
                    }
                    self.block0_written = true;
                }
                if self.cache_blocks > 0 {
                    self.cache.push_back((addr, block.to_vec()));
                    if self.cache.len() > self.cache_blocks {
                        self.cache.pop_front();
                    }
                }
                if let Some(c) = (self.map)(addr) {
                    self.cells.insert(c, block.to_vec());
                }
            }
            Ok(())
        }
    }

    fn run(sim: &mut Sim) -> Outcome {
        let mut last = 0.0;
        let run = probe(
            sim,
            Params::default(),
            &AtomicBool::new(false),
            &mut |f, _| {
                assert!(f + 1e-9 >= last, "progress goes back: {f} < {last}");
                last = f;
            },
            &mut |_, _, _| {},
        );
        let out = run.result.unwrap_or_else(|e| panic!("probe failed: {e:?}"));
        assert!((last - 1.0).abs() < 1e-9);
        assert!(sim.intact(), "the old content was not restored");
        assert_eq!(out.restore_failures, 0);
        assert!(out.restore_check.confirmed(), "{:?}", out.restore_check);
        out
    }

    /// A run without any assumption about its end.
    fn raw(sim: &mut Sim) -> ProbeRun {
        probe(sim, Params::default(), &AtomicBool::new(false), &mut |_, _| {}, &mut |_, _, _| {})
    }

    fn unresponsive(r: &ProbeRun) -> bool {
        matches!(r.result, Err(Abort::Unresponsive))
    }

    /// The coarse round's points of a drive of this size.
    fn points_of(size: u64) -> Vec<u64> {
        coarse_points(&Params::default(), size).1
    }

    fn assert_real(out: &Outcome, real: u64) {
        assert!(out.real <= real, "real {} > true {}", out.real, real);
        assert!(real - out.real <= MIB + BLOCK as u64, "real {} too far below true {}", out.real, real);
    }

    #[test]
    fn genuine_drive() {
        let mut sim = Sim::new(2 * 1000 * GB, Some);
        let out = run(&mut sim);
        assert_eq!(out.verdict, Verdict::Genuine);
        assert_eq!(out.real, out.reported);
        assert_eq!(out.bad_points, 0);
    }

    #[test]
    fn small_genuine_drive() {
        let mut sim = Sim::new(64 * MIB, Some);
        let out = run(&mut sim);
        assert_eq!(out.verdict, Verdict::Genuine);
    }

    #[test]
    fn wraps_around_a_power_of_two() {
        // 2 TB label, 8 GiB chip, high address bits ignored
        let mut sim = Sim::new(2 * 1000 * GB, |a| Some(a % (8 * GIB)));
        let out = run(&mut sim);
        assert_eq!(out.verdict, Verdict::Fake);
        assert_real(&out, 8 * GIB);
    }

    #[test]
    fn partial_chip_then_wrap() {
        // 7.5 GB usable of an 8 GiB address space, the rest of it lost, above it wraps
        let real = 7_500_000_000 / BLOCK as u64 * BLOCK as u64;
        let mut sim = Sim::new(1000 * GB, move |a| {
            let w = a % (8 * GIB);
            (w < real).then_some(w)
        });
        let out = run(&mut sim);
        assert_eq!(out.verdict, Verdict::Fake);
        assert_real(&out, real);
    }

    #[test]
    fn lost_writes_read_as_ff() {
        let real = 31 * GB / BLOCK as u64 * BLOCK as u64;
        let mut sim = Sim::new(512 * GB, move |a| (a < real).then_some(a));
        sim.limbo = 0xFF;
        let out = run(&mut sim);
        assert_eq!(out.verdict, Verdict::Fake);
        assert_real(&out, real);
        assert!(out.mismatches > 0);
        assert_eq!(out.io_errors, 0);
    }

    #[test]
    fn cache_does_not_hide_the_fake() {
        let real = 4 * GIB;
        let mut sim = Sim::new(256 * GB, move |a| (a < real).then_some(a));
        sim.cache_blocks = 512; // 2 MiB of recent writes
        let out = run(&mut sim);
        assert_eq!(out.verdict, Verdict::Fake);
        assert_real(&out, real);
    }

    #[test]
    fn failing_flash_in_the_middle() {
        let mut sim = Sim::new(64 * GB, |a| (!(20 * GB..21 * GB).contains(&a)).then_some(a));
        let out = run(&mut sim);
        assert_eq!(out.verdict, Verdict::Damaged);
        assert!(out.real <= 20 * GB && out.real >= 20 * GB - MIB - BLOCK as u64);
    }

    #[test]
    fn read_errors_count_as_lost_data() {
        let real = 16 * GB / BLOCK as u64 * BLOCK as u64;
        let mut sim = Sim::new(128 * GB, move |a| (a < real).then_some(a));
        sim.bad_reads = Box::new(move |a| a >= real);
        let out = raw(&mut sim).result.unwrap();
        assert_eq!(out.verdict, Verdict::Fake);
        assert_real(&out, real);
        assert!(out.io_errors > 0);
        assert!(sim.intact());
        // what could not be read was not written
        assert!(out.unsaved > 0);
        assert!(sim.written.iter().all(|&a| a < real));
    }

    #[test]
    fn drive_that_keeps_nothing() {
        let mut sim = Sim::new(32 * GB, |_| None);
        let out = run(&mut sim);
        assert_eq!(out.verdict, Verdict::Broken);
        assert_eq!(out.real, 0);
        assert!(out.mismatches > 0);
        assert_eq!(out.io_errors, 0);
    }

    #[test]
    fn cancel_restores_the_data() {
        let mut sim = Sim::new(64 * GB, |a| Some(a % (4 * GIB)));
        let cancel = AtomicBool::new(false);
        let mut calls = 0;
        let r = probe(
            &mut sim,
            Params::default(),
            &cancel,
            &mut |_, stage| {
                // after the baseline, in the middle of the first round's writes
                if stage == "prCoarse" {
                    calls += 1;
                    if calls == 40 {
                        cancel.store(true, Ordering::Relaxed);
                    }
                }
            },
            &mut |_, _, _| {},
        );
        assert!(matches!(r.result, Err(Abort::Cancelled)));
        assert!(sim.intact());
        assert!(r.restore_check.checked > 4096 && r.restore_check.confirmed(), "{:?}", r.restore_check);
    }

    #[test]
    fn too_small() {
        let mut sim = Sim::new(8 * MIB, Some);
        assert!(matches!(raw(&mut sim).result, Err(Abort::TooSmall)));
    }

    #[test]
    fn unreadable_old_content_is_not_overwritten() {
        let target = points_of(64 * GB)[300];
        let mut sim = Sim::new(64 * GB, Some);
        sim.bad_reads = Box::new(move |a| a == target);
        let r = raw(&mut sim);
        let out = r.result.unwrap();
        assert!(!sim.written.contains(&target), "the unreadable block was written");
        assert!(!sim.cells.contains_key(&target), "its cell changed");
        assert!(sim.intact());
        assert_eq!(out.unsaved, 1);
        assert_eq!((out.io_errors, out.mismatches), (1, 0));
        assert!(r.stats.failures.iter().any(|f| f.offset == target && f.op == Op::ReadOld));
    }

    #[test]
    fn write_failure_half_way_is_restored() {
        let mut sim = Sim::new(64 * GB, Some);
        sim.half_write_at = Some(3 * MIB);
        let r = raw(&mut sim);
        let out = r.result.unwrap();
        assert!(sim.half_write_at.is_none(), "the failing write happened");
        assert!(sim.intact());
        assert_eq!(out.restore_failures, 0);
        assert!(out.restore_check.confirmed());
        assert!(r.stats.failures.iter().any(|f| f.offset == 3 * MIB && f.op == Op::Write));
    }

    #[test]
    fn unreadable_start_writes_nothing() {
        let mut sim = Sim::new(64 * GB, Some);
        sim.bad_reads = Box::new(|a| a == 8 * MIB);
        let r = raw(&mut sim);
        assert!(unresponsive(&r));
        assert_eq!(sim.writes, 0);
        assert_eq!(r.restore_check, RestoreCheck::default());
        assert!(r.stats.failures.iter().any(|f| f.offset == 8 * MIB && f.op == Op::Baseline));
    }

    #[test]
    fn baseline_costs_no_writes() {
        let mut sim = Sim::new(64 * GB, Some);
        let r = raw(&mut sim);
        let grid = r.stats.grid;
        assert_eq!(r.result.unwrap().verdict, Verdict::Genuine);
        // one round: every block of the coarse set and the 16 flush chunks, written once and written back once
        assert_eq!(sim.writes, 2 * (grid.coarse_blocks + 16));
    }

    #[test]
    fn drive_that_fails_everything_is_not_broken() {
        let mut sim = Sim::new(125 * GB, Some);
        sim.fail_after = Some(0);
        let r = raw(&mut sim);
        assert!(unresponsive(&r), "{:?}", r.result);
        assert_eq!(sim.writes, 0);
        // it does not take the whole drive to find out: the start is read again after 64 failures
        assert!(sim.reads < 80, "{}", sim.reads);
    }

    /// Operations of a genuine drive until half of the coarse round's blocks are written.
    fn half_of_the_coarse_round() -> usize {
        let mut dry = Sim::new(64 * GB, Some);
        let grid = raw(&mut dry).stats.grid;
        dry.ops_at_write[grid.coarse_blocks / 2]
    }

    #[test]
    fn drive_that_stops_mid_round() {
        let half = half_of_the_coarse_round();
        // it never answers again: no verdict, and every block that was written is tried again and counted
        let mut sim = Sim::new(64 * GB, Some);
        sim.fail_after = Some(half);
        let r = raw(&mut sim);
        assert!(unresponsive(&r), "{:?}", r.result);
        assert!(r.stats.restore_failures > 100, "{}", r.stats.restore_failures);
        assert!(!r.restore_check.confirmed());
        assert!(r.stats.unsaved > 0 && r.stats.io_errors > 0);

        // it answers again after the run gave up (64 failures in a row, then the read of the start): the write-back
        // puts every cell written before the failure back
        let mut sim = Sim::new(64 * GB, Some);
        sim.fail_after = Some(half);
        sim.heals_after = Some(FAILED_IN_A_ROW as usize + 1);
        let r = raw(&mut sim);
        assert!(unresponsive(&r), "{:?}", r.result);
        assert!(!sim.cells.is_empty());
        assert!(sim.intact());
        assert_eq!(r.stats.restore_failures, 0);
        assert!(r.restore_check.confirmed(), "{:?}", r.restore_check);
    }

    #[test]
    fn one_transient_error_on_the_first_point() {
        let first = points_of(64 * GB)[0];
        let mut sim = Sim::new(64 * GB, Some);
        let failed = std::cell::Cell::new(false);
        sim.fail_writes_at = Box::new(move |a| a == first && !failed.replace(true));
        let r = raw(&mut sim);
        assert!(unresponsive(&r), "{:?}", r.result);
        assert_eq!(r.stats.io_errors, 1);
        assert!(sim.intact());
    }

    #[test]
    fn an_unreadable_flush_chunk_in_a_round_is_not_written() {
        let mut sim = Sim::new(64 * GB, Some);
        sim.bad_reads_after_write = Box::new(|a| a == 5 * MIB);
        let r = raw(&mut sim);
        assert!(unresponsive(&r), "{:?}", r.result);
        assert!(!sim.written.contains(&(5 * MIB)));
        assert_eq!(r.stats.unsaved, 256);
        assert!(sim.intact());
    }

    #[test]
    fn a_start_that_stops_reading_after_a_round_is_not_broken() {
        // every point is lost (the drive keeps nothing), but the start cannot be read again after the round
        let mut sim = Sim::new(32 * GB, |_| None);
        sim.bad_reads_after_write = Box::new(|a| a == 0);
        assert!(unresponsive(&raw(&mut sim)));
    }

    #[test]
    fn privacy_refusal_on_the_first_read_writes_nothing() {
        #[cfg(unix)]
        {
            struct Refusing(Sim);
            impl Device for Refusing {
                fn size(&self) -> u64 {
                    self.0.size()
                }
                fn read(&mut self, _: &mut [u8], _: u64) -> io::Result<()> {
                    Err(io::Error::from_raw_os_error(libc::EPERM))
                }
                fn write(&mut self, buf: &[u8], off: u64) -> io::Result<()> {
                    self.0.write(buf, off)
                }
            }
            let mut dev = Refusing(Sim::new(64 * GB, Some));
            let r = probe(&mut dev, Params::default(), &AtomicBool::new(false), &mut |_, _| {}, &mut |_, _, _| {});
            let Err(Abort::Access(a)) = r.result else { panic!("{:?}", r.result) };
            assert_eq!((a.op, a.errno), (Op::Baseline, Some(libc::EPERM)));
            assert_eq!(a.cause, if cfg!(target_os = "macos") { AccessCause::Privacy } else { AccessCause::Denied });
            assert_eq!(dev.0.writes, 0);
        }
    }

    #[test]
    fn decide_table() {
        use Decision as D;
        use Liveness as L;
        use PointClass as P;
        for (first, live, want) in [
            (P::Good, L::Ok, D::Continue),
            (P::Good, L::Mismatch, D::Continue),
            (P::Good, L::Error, D::Unresponsive),
            (P::Mismatch, L::Ok, D::Broken),
            (P::Mismatch, L::Mismatch, D::Broken),
            (P::Mismatch, L::Error, D::Unresponsive),
            (P::IoError, L::Ok, D::Unresponsive),
            (P::IoError, L::Mismatch, D::Unresponsive),
            (P::IoError, L::Error, D::Unresponsive),
        ] {
            assert_eq!(decide(first, live), want, "{first:?} {live:?}");
        }
    }

    #[test]
    fn restore_check_genuine() {
        let mut sim = Sim::new(64 * GB, Some);
        let out = run(&mut sim);
        let c = out.restore_check;
        assert!(c.checked > 4096, "{}", c.checked);
        assert_eq!((c.mismatched, c.unreadable, c.no_baseline, c.not_reread, c.table_area), (0, 0, 0, 0, false));
        assert_eq!(out.flush, FlushResult::Ok);
    }

    #[test]
    fn restore_check_wraparound_has_no_false_alarm() {
        let mut sim = Sim::new(2 * 1000 * GB, |a| Some(a % (8 * GIB)));
        let out = run(&mut sim);
        assert_eq!((out.restore_check.mismatched, out.restore_check.unreadable), (0, 0));
    }

    #[test]
    fn restore_check_catches_a_lost_write_back() {
        let mut sim = Sim::new(64 * GB, Some);
        sim.block0_sticks = true;
        let r = raw(&mut sim);
        let c = &r.restore_check;
        assert_eq!((c.mismatched, c.unreadable, c.table_area), (1, 0, true));
        assert_eq!(c.first, vec![BadBlock { offset: 0, kind: BadKind::Mismatch }]);
        assert!(!c.confirmed());
        assert_eq!(r.result.unwrap().restore_failures, 0, "the drive reported every write as done");
    }

    #[test]
    fn restore_check_unreadable() {
        let mut sim = Sim::new(64 * GB, Some);
        sim.bad_reads_after_flush = Box::new(|a| a == 4 * MIB);
        let c = raw(&mut sim).restore_check;
        assert_eq!((c.mismatched, c.unreadable, c.table_area), (0, 1, false));
        assert_eq!(c.first, vec![BadBlock { offset: 4 * MIB, kind: BadKind::Unreadable }]);
        assert!(c.checked > 4096);

        // a drive that reads nothing any more: the check stops after 16 failed reads in a row
        let mut sim = Sim::new(64 * GB, Some);
        sim.bad_reads_after_flush = Box::new(|_| true);
        let r = raw(&mut sim);
        let c = &r.restore_check;
        assert_eq!(c.checked, 0);
        assert!(c.unreadable > 4096 && c.table_area);
        assert_eq!(c.first.len(), MAX_LISTED);
        // one chunk and then 16 single blocks, not thousands of reads
        assert_eq!(sim.reads_after_flush, 1 + UNREADABLE_IN_A_ROW as usize);
    }

    #[test]
    fn refine_blocks_not_reread() {
        let mut sim = Sim::new(2 * 1000 * GB, |a| Some(a % (8 * GIB)));
        let out = run(&mut sim);
        assert!(out.restore_check.not_reread > 0);
    }

    #[test]
    fn flush_unsupported_is_not_an_error() {
        let mut sim = Sim::new(64 * GB, Some);
        sim.flush_error = Some(if cfg!(windows) { 1 } else { 25 }); // ERROR_INVALID_FUNCTION / ENOTTY
        let out = run(&mut sim);
        assert_eq!(out.flush, FlushResult::Unsupported);
        assert_eq!(out.verdict, Verdict::Genuine);
        let mut sim = Sim::new(64 * GB, Some);
        sim.flush_error = Some(if cfg!(windows) { 1117 } else { 5 });
        assert!(matches!(run(&mut sim).flush, FlushResult::Failed(_)));
    }

    #[test]
    fn a_failed_write_back_is_counted() {
        // the drive refuses every second write of one block: the pattern goes in, the write-back does not
        let target = points_of(64 * GB)[100];
        let calls = std::cell::Cell::new(0);
        let mut sim = Sim::new(64 * GB, Some);
        sim.fail_writes_at = Box::new(move |a| a == target && calls.replace(calls.get() + 1) == 1);
        let r = raw(&mut sim);
        assert_eq!(r.result.unwrap().restore_failures, 1);
        assert_eq!(r.restore_check.mismatched, 1);
    }

    #[test]
    fn events_name_the_stages() {
        let mut sim = Sim::new(64 * GB, Some);
        let mut names = Vec::new();
        probe(&mut sim, Params::default(), &AtomicBool::new(false), &mut |_, _| {}, &mut |n, _, _| names.push(n));
        assert_eq!(names, ["baseline", "round", "liveness", "restore", "flush", "verify"]);
    }
}
