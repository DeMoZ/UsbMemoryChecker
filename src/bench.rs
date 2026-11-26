//! Speed test: writes a file of the chosen size to the drive past the system cache, reads it back and checks
//! the data. Average speed is the whole size over the whole time (including the final flush to the drive);
//! the maximum and minimum are over 16 MiB windows.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use serde::Serialize;

use crate::dio::{self, AlignedBuf};
use crate::pattern;

pub const CHUNK: usize = 4 << 20;
pub const WINDOW: u64 = 16 << 20;
/// Kept free on the drive besides the test file.
pub const RESERVE: u64 = 32 << 20;

#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq)]
pub struct Stats {
    /// MB/s (10^6 bytes per second), like drive makers and other benchmarks.
    pub avg: f64,
    pub max: f64,
    pub min: f64,
    pub seconds: f64,
}

/// Collects window speeds while data moves.
pub struct Meter {
    total_secs: f64,
    total_bytes: u64,
    win_secs: f64,
    win_bytes: u64,
    max: f64,
    min: f64,
}

impl Default for Meter {
    fn default() -> Self {
        Meter { total_secs: 0.0, total_bytes: 0, win_secs: 0.0, win_bytes: 0, max: 0.0, min: f64::INFINITY }
    }
}

impl Meter {
    /// Adds one transfer; returns the speed of a window when it is complete.
    pub fn add(&mut self, bytes: u64, secs: f64) -> Option<f64> {
        self.total_secs += secs;
        self.total_bytes += bytes;
        self.win_secs += secs;
        self.win_bytes += bytes;
        if self.win_bytes < WINDOW {
            return None;
        }
        Some(self.close_window())
    }

    fn close_window(&mut self) -> f64 {
        let speed = mbps(self.win_bytes, self.win_secs);
        self.max = self.max.max(speed);
        self.min = self.min.min(speed);
        self.win_secs = 0.0;
        self.win_bytes = 0;
        speed
    }

    /// Time that belongs to no single transfer (final flush to the drive).
    pub fn add_time(&mut self, secs: f64) {
        self.total_secs += secs;
        self.win_secs += secs;
    }

    pub fn finish(mut self) -> Stats {
        // a short last window (less than half) would show a random speed: it only counts in the average
        if self.win_bytes >= WINDOW / 2 || self.max == 0.0 {
            self.close_window();
        }
        Stats {
            avg: mbps(self.total_bytes, self.total_secs),
            max: self.max,
            min: if self.min.is_finite() { self.min } else { 0.0 },
            seconds: self.total_secs,
        }
    }
}

pub fn mbps(bytes: u64, secs: f64) -> f64 {
    if secs <= 0.0 { 0.0 } else { bytes as f64 / 1e6 / secs }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeedResult {
    pub bytes: u64,
    pub write: Stats,
    pub read: Stats,
    /// 4 KiB blocks that came back different from what was written (should be 0).
    pub bad_blocks: usize,
}

/// One point for the live chart.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Point {
    pub op: &'static str,
    pub done: u64,
    pub total: u64,
    pub mbps: f64,
}

#[derive(Debug)]
pub enum Error {
    Cancelled,
    /// Not enough free space: (needed, free).
    NoSpace(u64, u64),
    Io(String),
}

fn io_err(what: &str, path: &Path, e: std::io::Error) -> Error {
    Error::Io(format!("{what} {}: {e}", path.display()))
}

pub fn test_file(dir: &Path) -> PathBuf {
    dir.join(".UsbMemoryChecker-speed.bin")
}

/// Removes the test file when the test ends in any way.
struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

pub fn run(dir: &Path, bytes: u64, cancel: &AtomicBool, on_point: &mut dyn FnMut(Point)) -> Result<SpeedResult, Error> {
    let bytes = bytes.div_ceil(CHUNK as u64) * CHUNK as u64;
    let (free, _) = dio::space(dir).map_err(|e| io_err("could not read free space of", dir, e))?;
    if free < bytes + RESERVE {
        return Err(Error::NoSpace(bytes + RESERVE, free));
    }
    let path = test_file(dir);
    let _cleanup = Cleanup(path.clone());
    let salt = pattern::new_salt();
    let mut buf = AlignedBuf::new(CHUNK);

    let file = dio::create(&path).map_err(|e| io_err("could not create", &path, e))?;
    let mut meter = Meter::default();
    let mut off = 0u64;
    while off < bytes {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        pattern::fill(&mut buf, off, salt);
        let t = Instant::now();
        dio::write_at(&file, &buf, off).map_err(|e| io_err("could not write", &path, e))?;
        off += CHUNK as u64;
        if let Some(mbps) = meter.add(CHUNK as u64, t.elapsed().as_secs_f64()) {
            on_point(Point { op: "write", done: off, total: bytes, mbps });
        }
    }
    let t = Instant::now();
    dio::full_sync(&file).map_err(|e| io_err("could not flush", &path, e))?;
    meter.add_time(t.elapsed().as_secs_f64());
    drop(file);
    let write = meter.finish();

    let file = dio::open_read(&path).map_err(|e| io_err("could not open", &path, e))?;
    let mut meter = Meter::default();
    let mut bad_blocks = 0;
    let mut off = 0u64;
    while off < bytes {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let t = Instant::now();
        dio::read_at(&file, &mut buf, off).map_err(|e| io_err("could not read", &path, e))?;
        let secs = t.elapsed().as_secs_f64();
        bad_blocks += pattern::count_bad(&buf, off, salt);
        off += CHUNK as u64;
        if let Some(mbps) = meter.add(CHUNK as u64, secs) {
            on_point(Point { op: "read", done: off, total: bytes, mbps });
        }
    }
    Ok(SpeedResult { bytes, write, read: meter.finish(), bad_blocks })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_windows() {
        let mut m = Meter::default();
        // 16 MiB at 100 MB/s, then 16 MiB at 50 MB/s, then a tiny tail
        let w = WINDOW as f64 / 1e6;
        assert!(m.add(WINDOW / 2, w / 200.0).is_none());
        assert!((m.add(WINDOW / 2, w / 200.0).unwrap() - 100.0).abs() < 1e-6);
        assert!((m.add(WINDOW, w / 50.0).unwrap() - 50.0).abs() < 1e-6);
        m.add(1 << 20, 0.0001);
        let s = m.finish();
        assert!((s.max - 100.0).abs() < 1e-6 && (s.min - 50.0).abs() < 1e-6);
        assert!(s.avg > 60.0 && s.avg < 70.0);
    }

    #[test]
    fn small_test_in_a_folder() {
        let dir = std::env::temp_dir().join(format!("umc_bench_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut points = Vec::new();
        let r = run(&dir, 40 << 20, &AtomicBool::new(false), &mut |p| points.push(p)).unwrap();
        assert_eq!(r.bytes, 40 << 20);
        assert_eq!(r.bad_blocks, 0);
        assert!(r.write.avg > 0.0 && r.read.avg > 0.0 && r.write.max >= r.write.min);
        assert!(points.iter().any(|p| p.op == "write") && points.iter().any(|p| p.op == "read"));
        assert!(!test_file(&dir).exists(), "the test file must be removed");
        let e = run(&dir, u64::MAX / 4, &AtomicBool::new(false), &mut |_| {});
        assert!(matches!(e, Err(Error::NoSpace(..))));
        let cancelled = run(&dir, 40 << 20, &AtomicBool::new(true), &mut |_| {});
        assert!(matches!(cancelled, Err(Error::Cancelled)));
        assert!(!test_file(&dir).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
