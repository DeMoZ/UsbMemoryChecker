# Proposal

## Why

A full capacity check of a fake SD card that reports 250 GiB but holds only ~17 GB took more than 10 hours: the check
writes the whole fake capacity (268 GB) and only then reads it back, so damage that starts at 17 GB is found after
hours of useless writing — and on a wrap-around fake every byte written past the real size overwrites the real part
again (user files and file system records included). On top of that, I/O is strictly serial: the drive idles while
the CPU generates the next pattern buffer or verifies the last one.

## What Changes

- **Early stop in the full check.** After each 1 GiB test file is written and flushed, the check reads a few
  samples back (from the new file, from the first file and from one earlier file) and verifies them. When a sample
  is damaged, writing stops; everything written so far is then read back and verified as before.
- **Correct real-capacity estimate for wrap-around fakes.** When the start of the test data was overwritten by data
  written later, the estimate is the amount of test data that survived after the overwritten lead, instead of
  `used_before + first_bad` (which would give almost 0).
- **New outcome field** `stopped_early` (JSON `stoppedEarly`) in `fullcheck::Outcome`, shown in the window as an
  extra sentence of the full-check verdict; a new progress stage for the read-back after an early stop. New
  i18n keys in all 5 languages (`prVerifyEarly`, `fullStoppedEarly`, `errFullInconclusive`, the repair offer texts and
  the reworded full-check mode texts).
- **Pipelined I/O** for the full check and the speed test: pattern generation for the next buffer (write phase) and
  verification of the previous buffer (read phase) run on a helper thread while the current direct I/O call is in
  flight (three `AlignedBuf`s in rotation). Calls stay 4 MiB (a 16 MiB call can outlast the quit wait on a very slow drive). Timing still covers only the
  I/O calls (plus the final flush); `bench::WINDOW` (16 MiB) windows for max/min keep their meaning.
- **Speed test size stays exact**: the requested size is no longer rounded up to a whole chunk (100 MiB would otherwise
  become a multiple of the chunk); the last chunk is shorter instead.
- **Testable full check**: the full check runs over a small `Volume` trait (real folder implementation + simulated
  fake volumes in tests), with parameters (file size, chunk, sample size) that tests can shrink.
- README.md / README.ru.md: the full-check description mentions that it stops early on a fake drive.

Out of scope: the quick probe (`probe.rs`, `rawdev.rs`, `helper.rs`) — its write-back order is critical and it does
not use the streaming code. No multi-request I/O queues (overlapped / async I/O).

## Capabilities

### New Capabilities

- `full-capacity-check`: the file-based full check of a mounted volume — fill, in-flight sampling with early stop,
  read-back, outcome fields, real-capacity estimate, progress, cancel and cleanup, and how the window shows it.
- `speed-test`: the read/write speed test — exact test size, honest timing, speed windows, data verification, and
  pipelined buffer preparation.

### Modified Capabilities

None (the project has no specs yet).

## Impact

- Check offered after a repair (`run_fix` result, `verifyRepaired` request in `main.rs`, Repair tab in `index.html`),
  see design D10.
- Code: `src/fullcheck.rs` (rewrite of `run` over a `Volume` trait, sampling, estimate, new tests), `src/bench.rs`
  (pipeline, exact size), new `src/pipe.rs` (fill/verify helper thread), `src/pattern.rs` (`is_later_block`, `splitmix` visible), `src/main.rs` (`run_full`
  stage key, `mod pipe`), `src/cli.rs` (prints early stop), `src/ui/index.html` (verdict text), `src/ui/i18n.js`
  (new keys × 5 languages), `README.md`, `README.ru.md`.
- Outcome JSON gains `stoppedEarly`; existing fields keep their names and meaning (`tested` = bytes written and read
  back). The repair size still comes from `realEstimate` with the same 1% margin.
- Memory: three 4 MiB buffers per test (12 MiB) instead of one.
- No new dependencies; Windows paths (`cfg(windows)` in `dio.rs`) unchanged and must keep compiling.
