# Design

## Context

See proposal.md (Why). Current code:

- `fullcheck::run(mount, limit, cancel, progress)` writes `UsbMemoryChecker-test/NNNN.umc` files of 1 GiB with
  `dio::create` + `dio::write_at` in `bench::CHUNK` (4 MiB) pieces, `dio::full_sync` per file, then reads every file
  back with `dio::open_read` + `dio::read_at` and `pattern::count_bad`. One `AlignedBuf`; `pattern::fill` and
  `count_bad` run on the I/O thread between calls. The test data position of a block is its offset in the whole test
  (file i starts at `i * FILE_SIZE`), so pattern data does not depend on chunk boundaries.
- `Outcome { tested, ok, bad, first_bad, total, used_before, real_estimate, write, read, write_error }` is sent as
  camelCase JSON. `main.rs::run_full` stores `min(real_estimate, disk.size) / 100 * 99` as the repair size when
  `bad > 0`; `index.html::taskDone` repeats that formula and `renderVerdict` builds the text from `fullOk` /
  `fullBad` / `fullWriteError`.
- Progress `{ op: "write"|"read", done, total, mbps }`: `run_full` maps it to fraction (write 0–0.5, read 0.5–1), stage
  (`prFill` / `prVerify`) and chart points. The window's `series` only has `write` and `read` arrays — any other `op`
  would break the chart.
- `bench::run` rounds the size up to a whole `CHUNK`; `Meter` measures only I/O call time (+ final flush via
  `add_time`) and closes 16 MiB windows for max/min.
- `probe.rs` already tests against a simulated drive (`Sim`: map of logical block → physical cell or nowhere,
  limbo byte, small write cache). The full check has no such abstraction: only a real-folder test.
- `cli.rs` already has `--cli full <disk> [limit MB]`.

## Goals / Non-Goals

**Goals:**
- A fake drive is detected within ~1–2 GiB of writing past its real size; total time ≈ writing + reading the real
  part instead of the reported size (17 GB card: ~40 min instead of 10 h at 15 MB/s).
- ≤ ~1.6% extra reads on a genuine drive; results on a genuine drive identical to today.
- Fake-drive behaviour of the full check covered by fast unit tests (simulated volumes), runnable in debug builds on
  the Windows CI runner.
- Keep the drive busy: no CPU work between consecutive I/O calls.

**Non-Goals:**
- No change to the quick probe, raw device access or the elevated helper.
- No overlapped/asynchronous I/O or several requests in flight: the I/O thread still issues one synchronous direct
  I/O call at a time.
- No new speed-test sizes, no UI redesign, no new settings.
- No attempt to protect the user's files on a wrap-around fake beyond stopping early (see Risks).

## Decisions

### D1. `Volume` trait under the full check

`fullcheck` gets a small trait for "the place test files go", with a real implementation over a folder and a
simulated one in tests:

```rust
pub trait Volume {
    fn space(&self) -> io::Result<(u64, u64)>;               // (free, total)
    fn prepare(&mut self) -> io::Result<()>;                  // empty test folder (old one removed)
    fn create(&mut self, file: usize) -> io::Result<()>;      // file 0 = "0001.umc"
    fn write(&mut self, file: usize, buf: &[u8], off: u64) -> io::Result<()>;
    fn finish(&mut self, file: usize) -> io::Result<()>;      // flush to the drive (F_FULLFSYNC) and close
    fn read(&mut self, file: usize, buf: &mut [u8], off: u64) -> io::Result<()>;
    fn cleanup(&mut self);                                    // remove the test folder, idempotent
}
```

- `DirVolume { mount, dir, writer: Option<(usize, File)>, reader: Option<(usize, File)> }`: `create` uses
  `dio::create`, `read` opens with `dio::open_read` and keeps the handle while the same file is read (sample reads
  and read-back), dropping a still-open writer first. `cleanup` drops both handles before `remove_dir_all` (Windows
  cannot delete open files); `Drop` calls `cleanup` too.
- `pub fn run(mount, limit, cancel, progress)` keeps its signature: it builds a `DirVolume` and calls
  `run_on(&mut vol, limit, Params::default(), cancel, progress)`, which always calls `vol.cleanup()` after the inner
  function returns (success, cancel, error). `main.rs` and `cli.rs` callers do not change.
- `Params { file_size, chunk, sample, guard }`, default `{ 1 GiB, 4 MiB, 4 MiB, 64 MiB }` (the full check uses 4 MiB calls, see D8), so tests can
  use `{ 8 MiB, 1 MiB, 256 KiB, 2 MiB }` and stay small. Checked with `debug_assert!`: `file_size % chunk == 0`, all
  values multiples of `BLOCK`, `guard + 2 * sample <= file_size`.
- *Alternative*: keep testing only on a real temp folder. Rejected: a real file system cannot simulate a lying
  controller, and the early-stop logic is exactly what needs tests.
- *Alternative*: reuse `probe::Device`. Rejected: the full check works with files on a mounted volume (no admin
  rights), not with device offsets; file create/flush/cleanup are part of the behaviour.

### D2. Sampling strategy (after each completed file)

At checkpoint k (file k, 0-based, just written and `finish`ed; written < planned; no write error / disk full):

| sample | where | catches |
|---|---|---|
| S1 | file k, offset 0 | lost writes / zeros / 0xFF / stale data in the new file |
| S2 | file k, pseudo-random offset in `[0, len_k − guard − sample]` | same, deeper in the file |
| S3 | file 0, offset 0 (k ≥ 1) | wrap-around: the first test data is the first to be overwritten |
| S4 | one earlier file j ∈ [1, k−1], pseudo-random offset (k ≥ 2) | wrap / damage when the file system did not lay files out in order |

- Sample size 4 MiB, so ≤ 16 MiB per 1 GiB = 1.56% of the written bytes; flash reads are usually faster than writes,
  so the time overhead on a genuine card is about 1%.
- Pseudo-random choices come from `splitmix(salt ^ k)` (made `pub(crate)` in `pattern.rs`), offsets aligned to
  `BLOCK` (direct I/O on Windows needs sector-aligned offsets). The plan is a pure function
  `sample_plan(k, starts/lens, salt, &Params) -> Vec<(file, off, len)>` so it can be unit-tested. Duplicates removed.
- **Controller cache**: the OS cache is bypassed (`F_NOCACHE` / `FILE_FLAG_NO_BUFFERING`) and the file is flushed
  with `F_FULLFSYNC` before sampling. A fake controller can still answer the last few MiB of writes from its own
  RAM (the probe pushes them out with a 16 MiB flush area). Here no sample of the new file lies in its last
  `guard` = 64 MiB, so ≥ 64 MiB of other data was written after every sampled block; S3/S4 are ≥ 1 GiB old.
- **Why it catches wrap-around at any real size R**: with sequential allocation, test position x sits at device
  address d₀ + x and a wrap modulo R makes position x + R overwrite position x. So position 0 (S3) is damaged as
  soon as more than R bytes were written — independent of `used_before` and of R being a power of two. Detection
  happens at the first checkpoint after `written > R`, i.e. ≤ 1 file late.
- **Lost writes past R**: the first file that lies fully past R fails S1 and S2; a file that straddles R may pass
  (S2 may land before R), so detection is ≤ 2 files late.
- Verification of samples runs inline on the I/O thread with the same `check_chunk` used by the read-back; samples
  are small.
- *Alternative*: sample every chunk right after its write (read-after-write). Rejected: the data would come straight
  from the controller cache, and it doubles the I/O.
- *Alternative*: sample the tail of the previous file instead of S2. Equivalent detection delay; kept S2 for
  locality and simplicity.

### D3. After an early stop: full read-back of everything written

When a sample fails, writing stops (`stopped_early = true`) and the normal read-back runs over everything written.

- Cost is bounded: written ≤ R + 2 files, so the read-back takes about as long as the write did (17 GB card:
  ~20 min). The total stays ≈ 2 × the real part.
- It keeps every outcome field meaningful (`tested` = bytes written and read back, `ok + bad = tested`, exact
  `first_bad`), it verifies the *real* part of the drive completely (a drive that also has bad blocks below R is
  reported correctly), and it gives the exact boundary for the estimate (D4) instead of the 1 GiB granularity of
  checkpoints.
- *Alternative*: stop right after the failed sample and report only the samples. Rejected: `tested/ok/bad` would
  lose meaning, the estimate would be ±1 GiB, and the real part would stay unverified — the user is about to
  partition the drive to that size.

### D4. Real-capacity estimate

The read-back tallies, in order: `bad`, `first_bad`, `period` and the damaged runs (below). The estimate has two
branches.

```text
first_bad = None                  → used_before + tested         (genuine, as today)
first_bad = Some(p), no wrap seen → used_before + p              (lost writes / damaged region / keeps nothing)
a wrap seen (period = Some(R))    → min(R, lowest damaged cell)  (wrap-around, below)
```

**Wrap proof and period.** A block read at test position `pos` that is an intact block of a *later* position `addr`
(its clear-text address is greater than `pos`, block-aligned, and the whole block matches that address and the
salt: `pattern::later_addr`) can only come from a drive that maps two addresses onto the same cell. The distance
`addr − pos` is then a multiple of the drive's period, and the smallest such distance over all blocks (`ChunkCheck.
min_delta`, folded into `Tally.period`) is the period itself: the real size R, counted from the start of the device
(test position q is at device address `used_before + q`, and the first R bytes of the device are the cells). A drive
without such a block is not treated as wrapping; zeros, 0xFF or stale data prove nothing.

**Damage.** Every damaged block that is *not* a later block (a dead or unreadable cell, a lost write) sits on cell
`(used_before + q) mod R`. `ChunkCheck` returns these blocks as runs, `Tally` merges them across chunks (at most
65,536 runs are kept; a drive with more separate damaged runs is garbage anyway) and `estimate` takes the lowest
cell at or above `used_before` over all runs (`lowest_cell`: a run is an interval of cells, possibly across the end
of the period). Cells below `used_before` are the user's / file-system area, rewritten by the system, and say
nothing about the flash, so they are ignored. The estimate is `min(R, lowest damaged cell)`: it never covers damage
and, because R is measured, never exceeds R. If `R ≤ used_before` (the user's files are bigger than the real size)
there is no cell at or above `used_before`, and the estimate is R. The periods are exact, so no safety margin for a
drive's write cache is needed: a cache can hide a part of the overwritten lead, but not all later blocks.
Damage is looked at over the whole read-back, not only the survivors, because a dead cell loses every write to it.

- `used_before` is not added in the wrap branch: R is already a size from the start of the device.
- The formula `used_before + first_bad` alone would give ≈ `used_before` for a wrap fake (first_bad = 0) — the bug the
  brief points out.
- *Alternative*: estimate from checkpoints (written at the last passing checkpoint). Rejected: up to 1 GiB too
  small, while the full read-back gives the boundary to 4 KiB.
- *Alternative (replaced)*: `tested − lead` with extra candidates for damage after the lead. Rejected: the lead's end
  is not reliably known (damage touching it, caches, files bigger than R), the measured period is.
- Pure function `estimate(used_before, tested, &Tally) -> u64`, unit-tested on all branches and fuzzed against the
  true usable size of simulated drives. `main.rs` and `index.html` keep the `min(estimate, reported) / 100 × 99` repair
  formula unchanged.
- When the estimate leaves no room (`real` after the 1% margin is 0), the window shows the drive as "keeps no data
  and cannot be repaired" (the repair would refuse it with `errBroken` anyway), not a full-size repair.

### D5. Errors during sampling and read-back

- Access errors (`dio::is_access_error`) on a sample or read-back read → `Error::Io` (project rule: a blocked volume
  must not look fake). Other read errors: a sample read is retried once (a USB hiccup is not damage); if it fails again the sample counts
  as damaged, a read-back chunk counts as damaged at once (as today).
- **Inconclusive result**: when writing stopped early but the read-back finds no damaged block, the samples' errors
  (wrong data or failed reads) were not confirmed, and only part of the free space was tested. The check then ends with
  `Error::Inconclusive` (window key `errFullInconclusive`) instead of reporting a clean result.
- Write path unchanged: "disk full" on create/write ends writing silently; another write error is stored in
  `write_error` and ends writing (without `finish`, as today); a create error that is not "disk full" →
  `Error::Io`.

### D6. Progress and statistics

- `Progress` gains `early: bool` (true for every read-back event after an early stop that was caused by a sample showing
  wrong data; false when the stop came from failed sample reads only). `run_full` maps
  `read && early` to the new stage `prVerifyEarly`; fraction mapping unchanged (the bar jumps forward from the write
  half to 0.5 after an early stop, which is fine — it never goes back).
- Sample reads emit no progress events and are not timed into the write `Meter`; only `vol.write` calls and
  `vol.finish` are timed, as today. The `read` meter times only read-back calls.
- After an early stop `run_full` sends the window a `rescale` message (factor planned / written) so the write curve of the chart spans the same width as the read-back curve.
- `cli.rs` prints one stderr line the first time it sees `early` ("data loss found after N MB: writing stopped,
  checking what was written"); when the run ends with `stoppedEarly` and that line was not printed (stop caused by
  read errors only) it prints a neutral line from the outcome.

### D7. I/O pipeline (`src/pipe.rs`, used by `fullcheck` and `bench`)

`std::thread::scope` + `std::sync::mpsc::sync_channel`, `DEPTH = 3` `AlignedBuf`s of `chunk` bytes per pipeline:

- **`Filler`** (write side): a scoped worker owns the buffers, fills chunk n = `[start + n·chunk, min(+chunk, end))`
  with `pattern::fill` and sends `Chunk { start, len, buf }` through a bounded channel; the I/O thread calls
  `next()`, writes `&buf[..len]`, then `recycle(buf)` back through a second channel. Chunks never cross a test-file
  boundary because `file_size % chunk == 0`.
- **`Checker`** (read side): the I/O thread takes a free buffer (`buffer()`), reads into it, and sends
  `(buf, start, len, read_ok)`; the worker runs `check_chunk` and folds it into a `Tally { bad, first_bad, period, runs }`
  in order, then returns the buffer. `finish()` closes the job channel and joins → `Tally`.
- A panic in a helper thread is caught in the thread: the `Filler` stream ends (the consumer reports an error), the
  `Checker` returns no tally (`finish() -> None`, reported as an error), so neither re-panics when the scope ends.
  `start_task` also catches a panic of a whole task and reports `errIo`, so the task slot is always released.
- Shutdown: dropping a `Filler`/`Checker` (cancel, error, early stop) closes its channels; the worker sees a send or
  receive error and returns, and the scope joins it — no deadlock, because at most `DEPTH` buffers exist and the
  return channels have capacity `DEPTH`.
- Only the I/O thread touches the volume, the progress callback and `cancel`; the worker only touches buffers and the
  salt, so `dyn Volume` and `dyn FnMut` need not be `Send`. `AlignedBuf` is already `Send`.
- Timing: `Instant` only around the I/O call. Waiting in `next()` / `buffer()` is not timed, so a slow CPU never
  lowers the reported speed (same semantics as today, where fill/verify were also untimed).
- *Alternative*: one helper thread per buffer / rayon. Rejected: new dependency, no gain — one core generates the
  pattern far faster than a USB drive moves data.
- *Alternative*: overlapped/async I/O with several requests in flight. Rejected for now (Non-Goal): needs
  platform-specific code (`aio`/IOCP) and makes timing per request ambiguous.

### D8. Chunk size 4 MiB, 16 MiB windows and exact speed-test size

- `bench::CHUNK` = 4 MiB for the speed test and the full check alike; speed windows (`WINDOW`) stay 16 MiB, so a
  window is four calls (`Meter` unchanged). A call cannot be interrupted: on a slow drive a 16 MiB call can outlast
  the window's 15 s quit wait (below ~1.1 MB/s) and leave the test file behind, a 4 MiB call lasts a quarter of that.
- `bench::run` rounds the size up to `BLOCK` (4 KiB) instead of `CHUNK`, and the last transfer is shorter; the
  existing test (`40 MiB` → `bytes == 40 MiB`) already checks it. The UI's "fits" check (size + 32 MiB) stays valid.
- The full check's "too little space" threshold stays at 4 MiB (`MIN_TEST` constant) instead of following `CHUNK`.
- Memory: 3 × 4 MiB per pipeline (12 MiB); the full check adds a 4 MiB sample buffer.

### D9. Expected gains (honest)

- Pattern generation/verification runs at GB/s on one core, so on a 15 MB/s card the pipeline saves well under 1%;
  the 10 h → ~40 min win comes from the early stop (D2/D3). The pipeline and bigger chunks matter on fast drives
  (USB 3 SSDs at 300–1000 MB/s), where fill + verify of a 4 MiB chunk is a noticeable share of a 4–14 ms call.
  Task 7.4 measures it on a RAM disk.

### D10. Check offered after a repair

- After a successful `fix` the Repair tab shows a card "Check the repaired drive" (full check of the new partition;
  with the early stop it takes about twice the time of writing the real size, and it shows when the size estimate
  was off). It is only an offer: nothing starts by itself. The card is hidden after a failed / declined repair
  (there is no `taskDone` with a result) and after the button was used.
- The new volume mounts a few seconds after the repair and the disk list refreshes by itself. The button stays
  disabled ("Waiting for the drive to mount…") until the disk lists a writable, mounted volume no bigger than the
  repaired size that is not the old one: the list showed the disk unmounted in between, or the volume looks different
  from the one before the repair (3 s), or 10 s have passed. It is also disabled while another task or an update
  download is running. After 45 s without it
  the status says to reconnect the drive and use the Real capacity tab.
- The offer is cleared when Rust accepts the request (the first `task` message), not when the button is pressed: on
  `errBusy` / `errNoDisk` it stays. It is dropped when the disk disappears from the list or changes size.
- The button sends the new request `verifyRepaired` with the disk id, which runs the normal `run_full` task (same
  progress, chart and verdict; it does not change the saved check mode). If the check finds bad data, the usual
  verdict and repair flow apply with the new estimate.

## Risks / Trade-offs

- [Known limit of the estimate] If writing stops before a wrap became visible (written W ≤ R + the drive's cache,
  so no block of a later position is read back), there is no period and the estimate is `used_before + first_bad`.
  When the damage then sits on cells below `used_before` (the user / file-system area, e.g. cells that the wrap
  reaches first), the estimate can exceed R by up to `used_before`. This needs a wrap and damage in that area and no
  visible wrap at the same time; the repair size is 99% of the estimate, and the check offered after the repair
  (D10) shows if the size was still too big.

- [Non-sequential file allocation (fragmented free space) breaks the "position 0 is overwritten first" argument] →
  S4 samples a random earlier file each checkpoint; the full read-back still finds all damage, only the stop may
  come later. The wrap estimate (measured period, damage cells) may then be less exact; `used_before + first_bad` cases are not
  affected.
- [A wrap-around fake already overwrites the user's files and file-system records at the start of the drive once
  writing passes R − used_before: test position x lands on device cell (used_before + x) mod R, so the wrap
  overwrites the start of the drive from position R − used_before on, while the stop comes only after position R
  plus up to two files] → inherent to any write test on such a fake, and the old check did it for the whole fake
  capacity. The early stop limits it to about `used_before` plus up to 2 GiB. README (both languages) and the
  window's full-check text warn that files already on a fake drive can be damaged and should be backed up first;
  the quick check restores everything it overwrites.
- [A genuine drive with a small damaged region now stops early and never writes the rest] → the result still says
  it loses data from `first_bad`, which is what the repair uses; the untested rest is mentioned in the verdict.
- [A huge controller cache (> 64 MiB of RAM, not flash) could serve S1/S2] → S3/S4 are ≥ 1 GiB old, and the full
  read-back reads everything after the stop anyway; at worst the stop is one file later.
- [12 MiB of buffers instead of 4 MiB] → acceptable for a desktop app; buffers are freed when the task ends.
- [Pipeline threads and channels add shutdown paths] → unit test that dropping a `Filler`/`Checker` early does not
  hang; cancel tests in both `bench` and `fullcheck`.
- [Debug-build test speed on the Windows CI runner] → simulated volumes use 8 MiB files and ≤ 256 MiB reported size.

## Migration Plan

No data migration. JSON gains `stoppedEarly`; older windows are never paired with a newer engine (same binary).
Rollback = revert the change.
