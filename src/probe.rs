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
//! **Nothing is lost**: before every write the probe reads what is there, and at the end (also after a cancel or
//! an error) writes it all back in reverse order. Reverse order restores the exact old state of every cell, even
//! when several addresses share one cell. Only a power cut or unplugging in the middle can leave damage.

use std::collections::BTreeSet;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::dio::is_access_error;
use crate::pattern::{self, BLOCK};

/// Whole device as an array of bytes. Offsets and sizes are multiples of [`BLOCK`].
pub trait Device {
    fn size(&self) -> u64;
    fn read(&mut self, buf: &mut [u8], off: u64) -> io::Result<()>;
    fn write(&mut self, buf: &[u8], off: u64) -> io::Result<()>;
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
}

#[derive(Debug)]
pub enum Abort {
    Cancelled,
    /// The system refused access (no rights, the volume is still mounted, write-protected switch).
    Access(String),
    TooSmall,
}

/// Progress: overall fraction 0..1 and the stage key for the window (`prCoarse`, `prRefine`, `prRestore`).
pub type Progress<'a> = dyn FnMut(f64, &'static str) + 'a;

/// Writes with a copy of the old content kept for [`Undo::restore`].
#[derive(Default)]
struct Undo {
    entries: Vec<(u64, Option<Vec<u8>>)>,
}

impl Undo {
    /// false — the drive failed the write (a bad spot, not an error of the run).
    fn write(&mut self, dev: &mut dyn Device, data: &[u8], off: u64) -> Result<bool, Abort> {
        let mut old = vec![0u8; data.len()];
        let old = match dev.read(&mut old, off) {
            Ok(()) => Some(old),
            Err(e) if is_access_error(&e) => return Err(Abort::Access(e.to_string())),
            Err(_) => None,
        };
        self.entries.push((off, old));
        match dev.write(data, off) {
            Ok(()) => Ok(true),
            Err(e) if is_access_error(&e) => Err(Abort::Access(e.to_string())),
            Err(_) => Ok(false),
        }
    }

    fn restore(&mut self, dev: &mut dyn Device, progress: &mut Progress) -> usize {
        let total = self.entries.len().max(1);
        let mut failures = 0;
        for (i, (off, old)) in self.entries.drain(..).rev().enumerate() {
            if let Some(old) = old
                && dev.write(&old, off).is_err()
            {
                failures += 1;
            }
            if i % 256 == 0 {
                progress(0.95 + 0.05 * i as f64 / total as f64, "prRestore");
            }
        }
        failures
    }
}

fn prev_pow2(x: u64) -> u64 {
    if x == 0 { 0 } else { 1u64 << (63 - x.leading_zeros()) }
}

struct Run<'a, 'p> {
    dev: &'a mut dyn Device,
    params: Params,
    salt: u64,
    round: u64,
    undo: Undo,
    cancel: &'a AtomicBool,
    progress: &'a mut Progress<'p>,
}

impl Run<'_, '_> {
    fn check_cancel(&self) -> Result<(), Abort> {
        if self.cancel.load(Ordering::Relaxed) { Err(Abort::Cancelled) } else { Ok(()) }
    }

    /// Tests the points (ascending); true — the point kept its data.
    fn test(&mut self, points: &[u64], from: f64, to: f64, stage: &'static str) -> Result<Vec<bool>, Abort> {
        self.round += 1;
        let salt = self.salt ^ self.round.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let flush = self.params.flush;
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
        let flush_chunks = flush.div_ceil(1 << 20);
        let steps = (set.len() as u64 + flush_chunks * 256 + points.len() as u64).max(1) as f64;
        let mut done = 0u64;
        let mut tick = |run: &mut Self, n: u64| {
            done += n;
            (run.progress)(from + (to - from) * (done as f64 / steps).min(1.0), stage);
        };

        let mut block = vec![0u8; BLOCK];
        let mut failed: BTreeSet<u64> = BTreeSet::new();
        for (i, &addr) in set.iter().rev().enumerate() {
            pattern::fill(&mut block, addr, salt);
            if !self.undo.write(self.dev, &block, addr)? {
                failed.insert(addr);
            }
            if i % 16 == 0 {
                self.check_cancel()?;
                tick(self, 16);
            }
        }
        let mut chunk = vec![0u8; 1 << 20];
        let mut off = 0;
        while off < flush {
            let len = (flush - off).min(1 << 20) as usize;
            pattern::fill(&mut chunk[..len], off, salt ^ 0xF1);
            self.undo.write(self.dev, &chunk[..len], off)?;
            off += len as u64;
            self.check_cancel()?;
            tick(self, 256);
        }
        let mut ok = Vec::with_capacity(points.len());
        for (i, &p) in points.iter().enumerate() {
            let good = !failed.contains(&p)
                && match self.dev.read(&mut block, p) {
                    Ok(()) => pattern::count_bad(&block, p, salt) == 0,
                    Err(e) if is_access_error(&e) => return Err(Abort::Access(e.to_string())),
                    Err(_) => false,
                };
            ok.push(good);
            if i % 16 == 0 {
                self.check_cancel()?;
                tick(self, 16);
            }
        }
        Ok(ok)
    }

    fn search(&mut self, size: u64) -> Result<(Verdict, u64, usize, usize), Abort> {
        let Params { flush, coarse_points, fine_points, resolution } = self.params;
        let step = prev_pow2((size - flush) / coarse_points).max(resolution);
        let mut points: Vec<u64> =
            (1..).map(|k| k * step).skip_while(|&p| p < flush).take_while(|&p| p + BLOCK as u64 <= size).collect();
        let last = size - BLOCK as u64;
        if points.last() != Some(&last) {
            points.push(last);
        }
        let ok = self.test(&points, 0.0, 0.6, "prCoarse")?;
        let mut tested = points.len();
        let mut bad = ok.iter().filter(|g| !**g).count();
        let Some(first_bad) = ok.iter().position(|g| !g) else {
            return Ok((Verdict::Genuine, size, tested, bad));
        };
        if first_bad == 0 {
            return Ok((Verdict::Broken, 0, tested, bad));
        }
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
            let ok = self.test(&fine, progress_at, next, "prRefine")?;
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
}

/// Runs the probe; the device's old content is written back before returning, whatever the result.
pub fn probe(
    dev: &mut dyn Device,
    params: Params,
    cancel: &AtomicBool,
    progress: &mut Progress,
) -> Result<Outcome, Abort> {
    let started = Instant::now();
    let reported = dev.size();
    let size = reported / BLOCK as u64 * BLOCK as u64;
    if size < params.flush + 4 * params.resolution {
        return Err(Abort::TooSmall);
    }
    let mut run = Run { dev, params, salt: pattern::new_salt(), round: 0, undo: Undo::default(), cancel, progress };
    let result = run.search(size);
    let Run { dev, mut undo, progress, .. } = run;
    let restore_failures = undo.restore(dev, progress);
    progress(1.0, "prRestore");
    let (verdict, real, points, bad_points) = result?;
    Ok(Outcome {
        verdict,
        reported,
        real,
        points,
        bad_points,
        seconds: started.elapsed().as_secs_f64(),
        restore_failures,
    })
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
            }
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
            for (i, block) in buf.as_chunks_mut::<BLOCK>().0.iter_mut().enumerate() {
                let addr = off + (i * BLOCK) as u64;
                if (self.bad_reads)(addr) {
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
        fn write(&mut self, buf: &[u8], off: u64) -> io::Result<()> {
            for (i, block) in buf.as_chunks::<BLOCK>().0.iter().enumerate() {
                let addr = off + (i * BLOCK) as u64;
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
        let cancel = AtomicBool::new(false);
        let mut last = 0.0;
        let out = probe(sim, Params::default(), &cancel, &mut |f, _| {
            assert!(f + 1e-9 >= last, "progress goes back: {f} < {last}");
            last = f;
        })
        .unwrap_or_else(|e| panic!("probe failed: {e:?}"));
        assert!((last - 1.0).abs() < 1e-9);
        assert!(sim.intact(), "the old content was not restored");
        assert_eq!(out.restore_failures, 0);
        out
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
        let out = probe(&mut sim, Params::default(), &AtomicBool::new(false), &mut |_, _| {}).unwrap();
        assert_eq!(out.verdict, Verdict::Fake);
        assert_real(&out, real);
    }

    #[test]
    fn drive_that_keeps_nothing() {
        let mut sim = Sim::new(32 * GB, |_| None);
        let out = run(&mut sim);
        assert_eq!(out.verdict, Verdict::Broken);
        assert_eq!(out.real, 0);
    }

    #[test]
    fn cancel_restores_the_data() {
        let mut sim = Sim::new(64 * GB, |a| Some(a % (4 * GIB)));
        let cancel = AtomicBool::new(false);
        let mut calls = 0;
        let r = probe(&mut sim, Params::default(), &cancel, &mut |_, _| {
            calls += 1;
            if calls == 40 {
                cancel.store(true, Ordering::Relaxed);
            }
        });
        assert!(matches!(r, Err(Abort::Cancelled)));
        assert!(sim.intact());
    }

    #[test]
    fn too_small() {
        let mut sim = Sim::new(8 * MIB, Some);
        assert!(matches!(
            probe(&mut sim, Params::default(), &AtomicBool::new(false), &mut |_, _| {}),
            Err(Abort::TooSmall)
        ));
    }
}
