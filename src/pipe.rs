//! Keeps the drive busy while the CPU works: test data for the next buffer is generated (write phase) and the data
//! of the previous buffer is verified (read phase) on a helper thread while the I/O thread has a direct I/O call
//! in flight. Three buffers rotate between the two threads. Only the I/O thread touches the volume, the progress
//! callback and the cancel flag; the helper thread only touches buffers and the salt.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
use std::thread::{Scope, ScopedJoinHandle};

use crate::dio::AlignedBuf;
use crate::pattern::{self, BLOCK};

/// Buffers in rotation per pipeline.
pub const DEPTH: usize = 3;

/// Most damaged runs a [`Tally`] remembers (a drive with more separate damaged runs is garbage anyway).
const MAX_RUNS: usize = 1 << 16;

/// Result of checking one chunk (offsets are inside the chunk).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChunkCheck {
    /// Damaged bytes (a multiple of the block size).
    pub bad: u64,
    pub first_bad: Option<u64>,
    /// Smallest `addr - position` over the damaged blocks that are intact blocks of a later position `addr`.
    pub min_delta: Option<u64>,
    /// Damaged blocks that are not blocks of a later position, as runs (offset, length).
    pub runs: Vec<(u64, u64)>,
}

/// Checks `buf`, read from test position `start`. `read_ok == false` (the read failed) makes the whole chunk bad.
pub fn check_chunk(buf: &[u8], start: u64, salt: u64, read_ok: bool) -> ChunkCheck {
    let mut c = ChunkCheck { bad: 0, first_bad: None, min_delta: None, runs: Vec::new() };
    if !read_ok {
        c.bad = buf.len() as u64;
        c.first_bad = Some(0);
        c.runs.push((0, buf.len() as u64));
        return c;
    }
    for (i, b) in buf.chunks(BLOCK).enumerate() {
        let off = (i * BLOCK) as u64;
        if pattern::count_bad(b, start + off, salt) == 0 {
            continue;
        }
        c.bad += b.len() as u64;
        c.first_bad.get_or_insert(off);
        if let Some(addr) = pattern::later_addr(b, start + off, salt) {
            let d = addr - (start + off);
            c.min_delta = Some(c.min_delta.map_or(d, |m| m.min(d)));
        } else {
            match c.runs.last_mut() {
                Some((o, l)) if *o + *l == off => *l += b.len() as u64,
                _ => c.runs.push((off, b.len() as u64)),
            }
        }
    }
    c
}

/// Chunk results folded in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub bad: u64,
    /// Test position of the first damaged block.
    pub first_bad: Option<u64>,
    /// Wrap period: the smallest distance between a block's position and the later position whose data it holds.
    /// Only a drive that wraps its addresses returns such blocks, so `Some` is the proof of a wrap-around.
    pub period: Option<u64>,
    /// Damaged blocks that are not blocks of a later position, as runs (test position, length), in order.
    pub runs: Vec<(u64, u64)>,
    /// More than [`MAX_RUNS`] runs: the later ones are not in `runs`.
    pub runs_dropped: bool,
}

impl Tally {
    /// Adds the next chunk (chunks must come in order).
    pub fn add(&mut self, start: u64, c: &ChunkCheck) {
        self.bad += c.bad;
        if let Some(b) = c.first_bad {
            self.first_bad.get_or_insert(start + b);
        }
        if let Some(d) = c.min_delta {
            self.period = Some(self.period.map_or(d, |p| p.min(d)));
        }
        for &(o, l) in &c.runs {
            let pos = start + o;
            if let Some(run) = self.runs.last_mut().filter(|r| r.0 + r.1 == pos) {
                run.1 += l;
            } else if self.runs.len() < MAX_RUNS {
                self.runs.push((pos, l));
            } else {
                self.runs_dropped = true;
            }
        }
    }
}

// Test hook: the helper threads started next by this thread panic (to test that this becomes an error).
#[cfg(test)]
thread_local! {
    static PANIC_NEXT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub fn panic_next_helpers(on: bool) {
    PANIC_NEXT.with(|p| p.set(on));
}

fn inject_panic() -> bool {
    #[cfg(test)]
    return PANIC_NEXT.with(|p| p.get());
    #[cfg(not(test))]
    false
}

/// A buffer of test data: `buf[..len]` belongs to test position `start`.
pub struct Chunk {
    pub start: u64,
    pub len: usize,
    pub buf: AlignedBuf,
}

/// Generates test data for `[start, end)` in chunks on a helper thread; iterate to get the chunks in order.
pub struct Filler {
    rx: Receiver<Chunk>,
    free: Sender<AlignedBuf>,
}

impl Filler {
    /// `end - start` must be a multiple of the block size; the last chunk is shorter.
    pub fn start<'scope, 'env>(
        scope: &'scope Scope<'scope, 'env>,
        salt: u64,
        start: u64,
        end: u64,
        chunk: usize,
    ) -> Filler {
        let (tx, rx) = sync_channel::<Chunk>(DEPTH);
        let (free, free_rx) = channel::<AlignedBuf>();
        for _ in 0..DEPTH {
            let _ = free.send(AlignedBuf::new(chunk));
        }
        let boom = inject_panic();
        scope.spawn(move || {
            // a panic here ends the stream (the consumer sees `None` and reports an error) instead of
            // re-panicking when the scope ends
            let _ = catch_unwind(AssertUnwindSafe(|| {
                assert!(!boom, "injected helper panic");
                let mut pos = start;
                while pos < end {
                    let Ok(mut buf) = free_rx.recv() else { return };
                    let len = (end - pos).min(chunk as u64) as usize;
                    pattern::fill(&mut buf[..len], pos, salt);
                    if tx.send(Chunk { start: pos, len, buf }).is_err() {
                        return;
                    }
                    pos += len as u64;
                }
            }));
        });
        Filler { rx, free }
    }

    /// Hands a written buffer back for reuse.
    pub fn recycle(&self, buf: AlignedBuf) {
        let _ = self.free.send(buf);
    }
}

impl Iterator for Filler {
    type Item = Chunk;
    fn next(&mut self) -> Option<Chunk> {
        self.rx.recv().ok()
    }
}

struct Job {
    buf: AlignedBuf,
    start: u64,
    len: usize,
    read_ok: bool,
}

/// Verifies read chunks on a helper thread and folds the results into a [`Tally`].
pub struct Checker<'scope> {
    jobs: Option<SyncSender<Job>>,
    free: Receiver<AlignedBuf>,
    chunk: usize,
    handle: Option<ScopedJoinHandle<'scope, Option<Tally>>>,
}

impl<'scope> Checker<'scope> {
    pub fn start<'env>(scope: &'scope Scope<'scope, 'env>, salt: u64, chunk: usize) -> Checker<'scope> {
        let (jobs, jobs_rx) = sync_channel::<Job>(DEPTH);
        let (free_tx, free) = channel::<AlignedBuf>();
        for _ in 0..DEPTH {
            let _ = free_tx.send(AlignedBuf::new(chunk));
        }
        let boom = inject_panic();
        let handle = scope.spawn(move || {
            // None: the verify thread panicked, the result is unknown (never an empty, "clean" tally)
            catch_unwind(AssertUnwindSafe(|| {
                assert!(!boom, "injected helper panic");
                let mut tally = Tally::default();
                for job in jobs_rx {
                    let c = check_chunk(&job.buf[..job.len], job.start, salt, job.read_ok);
                    tally.add(job.start, &c);
                    let _ = free_tx.send(job.buf);
                }
                tally
            }))
            .ok()
        });
        Checker { jobs: Some(jobs), free, chunk, handle: Some(handle) }
    }

    /// A free buffer to read into (waits while all of them are being verified).
    pub fn buffer(&self) -> AlignedBuf {
        self.free.recv().unwrap_or_else(|_| AlignedBuf::new(self.chunk))
    }

    /// Queues `buf[..len]`, read from test position `start`, for verification.
    pub fn check(&self, buf: AlignedBuf, start: u64, len: usize, read_ok: bool) {
        if let Some(j) = &self.jobs {
            let _ = j.send(Job { buf, start, len, read_ok });
        }
    }

    /// Waits for everything queued and returns the totals; None if the verify thread panicked.
    pub fn finish(mut self) -> Option<Tally> {
        self.jobs = None;
        self.handle.take().and_then(|h| h.join().ok().flatten())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_chunk_finds_bad_blocks() {
        let mut buf = vec![0u8; BLOCK * 8];
        pattern::fill(&mut buf, 1 << 20, 5);
        buf[BLOCK * 5 + 3] ^= 0xFF;
        let c = check_chunk(&buf, 1 << 20, 5, true);
        assert_eq!((c.bad, c.first_bad, c.min_delta), (BLOCK as u64, Some(5 * BLOCK as u64), None));
        assert_eq!(c.runs, vec![(5 * BLOCK as u64, BLOCK as u64)]);
        let mut shifted = vec![0u8; BLOCK * 8];
        pattern::fill(&mut shifted, 2 << 20, 5);
        let c = check_chunk(&shifted, 1 << 20, 5, true);
        assert_eq!((c.bad, c.first_bad), (8 * BLOCK as u64, Some(0)));
        assert!(c.runs.is_empty(), "blocks of a later position are not plain damage");
        assert_eq!(c.min_delta, Some(1 << 20), "blocks of a later position prove a wrap");
        let c = check_chunk(&buf, 1 << 20, 5, false);
        assert_eq!((c.bad, c.first_bad, c.min_delta), (8 * BLOCK as u64, Some(0), None));
        assert_eq!(c.runs, vec![(0, 8 * BLOCK as u64)]);
    }

    #[test]
    fn wrap_period_is_the_smallest_distance() {
        let salt = 9;
        let mut buf = vec![0u8; BLOCK * 6];
        pattern::fill(&mut buf, 0, salt);
        // block 0 holds position 90 blocks (distance 90), block 1 holds position 31 blocks (distance 30),
        // block 3 is zeros (damaged, but no proof of anything)
        let mut x = vec![0u8; BLOCK];
        pattern::fill(&mut x, 90 * BLOCK as u64, salt);
        buf[..BLOCK].copy_from_slice(&x);
        pattern::fill(&mut x, 31 * BLOCK as u64, salt);
        buf[BLOCK..2 * BLOCK].copy_from_slice(&x);
        buf[3 * BLOCK..4 * BLOCK].fill(0);
        let c = check_chunk(&buf, 0, salt, true);
        assert_eq!(c.min_delta, Some(30 * BLOCK as u64));
        assert_eq!(c.runs, vec![(3 * BLOCK as u64, BLOCK as u64)], "later blocks are not counted as plain damage");
        let mut t = Tally::default();
        t.add(0, &c);
        assert_eq!((t.period, t.bad), (Some(30 * BLOCK as u64), 3 * BLOCK as u64));
    }

    #[test]
    fn tally_merges_runs_across_chunks() {
        let salt = 3;
        let len = (4 * BLOCK) as u64;
        let mut t = Tally::default();
        // chunk 0: last two blocks damaged; chunk 1: first block damaged; chunk 2: intact; chunk 3: block 1 damaged
        for (i, bad) in [vec![2, 3], vec![0], vec![], vec![1]].into_iter().enumerate() {
            let start = i as u64 * len;
            let mut buf = vec![0u8; 4 * BLOCK];
            pattern::fill(&mut buf, start, salt);
            for b in bad {
                buf[b * BLOCK + 9] ^= 0x55;
            }
            t.add(start, &check_chunk(&buf, start, salt, true));
        }
        let b = BLOCK as u64;
        assert_eq!(t.runs, vec![(2 * b, 3 * b), (13 * b, b)]);
        assert_eq!((t.bad, t.first_bad, t.period), (4 * b, Some(2 * b), None));
    }

    #[test]
    fn filler_matches_serial_fill() {
        let end = (5 << 20) + 12 * 1024;
        let chunk = 1 << 20;
        let mut expected = vec![0u8; end];
        pattern::fill(&mut expected, 0, 77);
        let mut got = Vec::new();
        std::thread::scope(|s| {
            let mut f = Filler::start(s, 77, 0, end as u64, chunk);
            let mut next_start = 0u64;
            let mut last_len = 0;
            while let Some(c) = f.next() {
                assert_eq!(c.start, next_start);
                next_start += c.len as u64;
                last_len = c.len;
                got.extend_from_slice(&c.buf[..c.len]);
                f.recycle(c.buf);
            }
            assert_eq!(last_len, 12 * 1024);
            assert!(f.next().is_none());
        });
        assert_eq!(got, expected);
    }

    #[test]
    fn checker_matches_serial_check() {
        let salt = 11;
        let chunk = 64 * 1024;
        let n = 6;
        let mut serial = Tally::default();
        let mut bufs = Vec::new();
        for i in 0..n {
            let start = (i * chunk) as u64;
            let mut b = vec![0u8; chunk];
            pattern::fill(&mut b, start, salt);
            if i == 1 || i == 3 {
                b[BLOCK * 2 + 1] ^= 1;
            }
            let ok = i != 4;
            serial.add(start, &check_chunk(&b, start, salt, ok));
            bufs.push((b, ok));
        }
        let piped = std::thread::scope(|s| {
            let c = Checker::start(s, salt, chunk);
            for (i, (b, ok)) in bufs.iter().enumerate() {
                let mut buf = c.buffer();
                buf[..chunk].copy_from_slice(b);
                c.check(buf, (i * chunk) as u64, chunk, *ok);
            }
            c.finish()
        });
        assert_eq!(piped, Some(serial.clone()));
        assert!(serial.bad > 0 && serial.first_bad.is_some());
    }

    #[test]
    fn dropping_early_does_not_hang() {
        std::thread::scope(|s| {
            let mut f = Filler::start(s, 1, 0, 64 << 20, 1 << 20);
            let c = f.next().unwrap();
            assert_eq!(c.start, 0);
            drop(c);
            drop(f);
            let ck = Checker::start(s, 1, 1 << 20);
            let buf = ck.buffer();
            ck.check(buf, 0, 1 << 20, true);
            drop(ck);
        });
    }

    #[test]
    fn a_panicking_helper_is_an_error_not_a_panic() {
        panic_next_helpers(true);
        let (none_chunk, none_tally) = std::thread::scope(|s| {
            let mut f = Filler::start(s, 1, 0, 8 << 20, 1 << 20);
            let ck = Checker::start(s, 1, 1 << 20);
            let buf = ck.buffer();
            ck.check(buf, 0, 1 << 20, true);
            (f.next().is_none(), ck.finish().is_none())
        });
        panic_next_helpers(false);
        assert!(none_chunk && none_tally);
    }
}
