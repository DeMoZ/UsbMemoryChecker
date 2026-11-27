# Tasks

Specs: `specs/full-capacity-check/spec.md`, `specs/speed-test/spec.md`. Design: `design.md` (D1–D8, D9, D10). Code comments
in English. Do not touch `probe.rs`, `rawdev.rs`, `helper.rs`.

## 1. Pipeline module `src/pipe.rs`

- [x] 1.1 Add `mod pipe;` in `main.rs` and `src/pipe.rs` with a module doc comment (why: keep the drive busy while
  the CPU fills / verifies, see design D7). Make `pattern::splitmix` `pub(crate)` (needed by D2). Verify `cargo build`.
- [x] 1.2 Implement `pub struct ChunkCheck { bad: u64 /*bytes*/, first_bad: Option<u64>, first_good: Option<u64> }`
  (offsets inside the chunk) and `pub fn check_chunk(buf: &[u8], start: u64, salt: u64, read_ok: bool) -> ChunkCheck`
  (`read_ok == false` → whole chunk bad, `first_bad = Some(0)`, `first_good = None`). Move/replace
  `fullcheck::first_bad_block` with it. Tests `check_chunk_finds_bad_and_good_blocks` (port of
  `finds_the_first_damaged_block`: block 5 corrupted → bad = 4096, first_bad = 5·4096, first_good = 0; all blocks
  shifted → first_good = None; `read_ok = false`) pass.
- [x] 1.3 Implement `pub struct Tally { pub bad: u64, pub first_bad: Option<u64>, pub lead_bad: u64, lead_open: bool }`
  with `add(start, len, &ChunkCheck)` (chunks in order; `lead_bad` = length of the damaged run from the first checked
  position, design D4). Test `tally_tracks_the_damaged_lead`: chunks [all bad, all bad, bad then good at block 3,
  good, bad] → lead_bad = 2·chunk + 3·4096, first_bad = 0, bad = sum; and a tally whose first chunk is intact has
  lead_bad = 0.
- [x] 1.4 Implement `Filler` (design D7): `Filler::start(scope, salt, start, end, chunk) -> Filler`, `next() ->
  Option<Chunk { start, len, buf }>`, `recycle(buf)`; `DEPTH = 3` buffers of `chunk` bytes; last chunk shorter.
  Test `filler_matches_serial_fill`: range 0..(5 MiB + 12 KiB) with 1 MiB chunks; concatenated `buf[..len]` equals
  `pattern::fill` over the whole range, starts are consecutive, the last len is 12 KiB, then `next()`
  returns None.
- [x] 1.5 Implement `Checker`: `Checker::start(scope, salt, chunk)`, `buffer() -> AlignedBuf`,
  `check(buf, start, len, read_ok)`, `finish(self) -> Tally`. Test `checker_matches_serial_check`: a buffer sequence
  with corrupted blocks in chunks 1 and 3 and an unreadable chunk 4 gives the same `Tally` as calling `check_chunk` +
  `Tally::add` one by one.
- [x] 1.6 Test `dropping_early_does_not_hang`: inside `std::thread::scope`, take one chunk from a `Filler` over a 64 MiB
  range and drop it; send one buffer to a `Checker` and drop it without `finish`; the scope returns (test finishes).
  Verify `cargo test pipe` passes.

## 2. Speed test on the pipeline (`src/bench.rs`)

- [x] 2.1 Keep `CHUNK = 4 << 20` (doc comment: cancel latency, design D8; windows stay 16 MiB). Round the size up to `pattern::BLOCK`
  instead of `CHUNK`; transfer length = `min(CHUNK, bytes − off)`. Keep the `NoSpace(bytes + RESERVE, free)` check.
- [x] 2.2 Write phase via `Filler` inside `std::thread::scope`: check cancel before each chunk, time only
  `dio::write_at`, then `recycle`; final `full_sync` timed with `meter.add_time` (unchanged). Read phase via
  `Checker`: time only `dio::read_at` (a read error still aborts with `Error::Io` → `check(.., true)` only on
  success); `bad_blocks = tally.bad / BLOCK`. Points as before (one per completed window).
- [x] 2.3 Tests: existing `meter_windows` and `small_test_in_a_folder` pass unchanged (the latter now proves the size
  is not rounded up to 48 MiB). Add `odd_size_is_exact`: `run(dir, 20 MiB + 4 KiB)` → `bytes == 20 MiB + 4 KiB`,
  `bad_blocks == 0`, exactly one write point and one read point (16 MiB window of four 4 MiB calls; the 4 MiB + 4 KiB tail is below half a
  window). Verify `cargo test bench` passes.

## 3. Full check over a `Volume` trait (no sampling yet)

- [x] 3.1 In `src/fullcheck.rs` add `pub trait Volume` (design D1), `Params { file_size, chunk, sample, guard }` with
  `Default` = `{ FILE_SIZE, bench::CHUNK, 4 MiB, 64 MiB }` and `debug_assert!` checks, `const MIN_TEST: u64 = 4 << 20`
  for the "no space" threshold (replaces `planned < CHUNK`).
- [x] 3.2 Implement `DirVolume` (folder `FOLDER` under the mount; `prepare` removes an old folder and creates it;
  `create` → `dio::create("{:04}.umc", file + 1)`; `write`/`finish` (`dio::full_sync`, then close); `read` keeps one
  `dio::open_read` handle per file and drops an open writer first; `cleanup` drops handles then `remove_dir_all`;
  `Drop` → `cleanup`). Replaces the `Cleanup` guard.
- [x] 3.3 Split `run` into `run(mount, limit, cancel, progress)` (builds `DirVolume`, calls `run_on` with
  `Params::default()`, signature unchanged for `main.rs`/`cli.rs`) and `run_on(vol: &mut dyn Volume, limit, params,
  cancel, progress)`, which calls the inner check and then always `vol.cleanup()`. Write phase uses `Filler`, read
  phase uses `Checker` (design D7); all existing error / disk-full / `write_error` behaviour kept (design D5).
  `Outcome` gets `stopped_early: bool` (always false in this group); `Progress` gets `early: bool` (false).
- [x] 3.4 Add the test harness `SimVolume` in `fullcheck::tests` (modelled on `probe::tests::Sim`): `total`, `used`
  (= used_before; file i byte o lives at device address `used + i·file_size + o`), `map: Fn(dev_addr) ->
  Option<cell>`, `cells: HashMap<u64, Vec<u8>>`, `limbo`, optional write cache of N blocks that answers reads first,
  `read_error: Fn(dev_addr) -> Option<io::ErrorKind>` (e.g. `Other` = media error, `PermissionDenied` = access),
  `slow_reads: Option<(len, Duration)>` (sleep on reads of exactly that length = sample reads), `cancel_at_read:
  Option<(n, Arc<AtomicBool>)>` (sets the flag on the n-th read), `create_error`, `disk_full_at` (bytes), an op log
  `Vec<Op { Create(i), Write(i, off, len), Finish(i), Read(i, off, len) }>`, and `cleaned: bool`. Test params
  `{ file_size: 8 MiB, chunk: 1 MiB, sample: 256 KiB, guard: 2 MiB }`; volumes ≤ 256 MiB (fast in debug on Windows CI).
- [x] 3.5 Tests in this group: `fills_and_checks_a_folder` (real folder, add `assert!(!out.stopped_early)`) passes;
  `genuine_sim_volume` (64 MiB, used 1 MiB): `tested` = planned (55 MiB), bad 0, `stopped_early` false,
  `real_estimate = used + tested`, `cleaned`; `sim_cells_hold_the_pattern` (genuine, cleanup recorded but cells kept):
  every written block's first 8 bytes equal its test position (pattern's clear-text address) and the read-back found
  0 bad → pipelined data is laid out exactly as serial; `disk_full_mid_write` (`disk_full_at` below planned): Ok,
  `write_error` None, `tested` = bytes accepted; `create_error_cleans_up`: `Err(Io)`, `cleaned`. Verify
  `cargo test fullcheck` passes.

## 4. Sampling, early stop and the estimate (`src/fullcheck.rs`)

- [x] 4.1 Implement `fn sample_plan(k, lens: &[u64], salt, &Params) -> Vec<(usize, u64, usize)>` (design D2: S1 file k
  offset 0; S2 file k random offset in `[0, len_k − guard − sample]` if it fits; S3 file 0 offset 0 for k ≥ 1; S4 one
  random file in 1..k−1 at a random offset for k ≥ 2; offsets `BLOCK`-aligned, lengths `min(sample, len)`, no
  duplicates; randomness from `pattern::splitmix(salt ^ k)`). Tests `sample_plan_positions`: k = 0 → only file-0
  samples; k = 1 → files 1 and 0; k = 5 → S1..S4 with S4 in files 1–4; every sample lies inside its file, is aligned,
  and no new-file sample overlaps the last `guard` bytes; `default_sampling_cost`: with `Params::default()` the plan
  reads ≤ `FILE_SIZE / 64` bytes per checkpoint.
- [x] 4.2 Run the sample checks after each `finish` when `written < planned` and writing did not end on disk full /
  write error: check cancel before every sample read; read into a separate `AlignedBuf(sample)` on the I/O thread
  (untimed, no progress event); verify with `pipe::check_chunk` at test position `starts[file] + off`. Access error
  (`dio::is_access_error`) → `Error::Io`; other read error or any bad block → stop writing, `stopped_early = true`.
- [x] 4.3 Read-back: abort with `Error::Io` on access errors (design D5), other errors count the chunk as bad; set
  `Progress.early = stopped_early` on every read event. Add `fn estimate(used_before, tested, &Tally, guard) ->
  u64` (design D4) and use it for `real_estimate`. Update the module doc comment (sampling + early stop) and the
  `Outcome` field docs (`stopped_early`, new `real_estimate` meaning). Test `estimate_cases`: genuine, lost writes
  (p > 0), wrap (lead < tested → tested − lead), all bad (→ used_before).
- [x] 4.4 Fake-volume tests (all on `SimVolume`, 256 MiB, used 1 MiB; R = real size in device bytes; each also asserts
  `cleaned` and that every progress op is `write`/`read` with non-decreasing `done` per phase):
  - `genuine_volume_is_not_stopped`: `stopped_early` false, sample reads present in the log before the last write,
    no sample read after the last file's `Finish` and before the read-back;
  - `wrap_around_fake_stops_early`: `map = d % R` with R = 40 MiB (not a power of two) and R = 32 MiB →
    `stopped_early`, `tested ≤ R + 2·file_size`, `real_estimate ≤ R` and `R − real_estimate ≤ 1 MiB`;
  - `lost_writes_fake_stops_early`: `map = (d < R).then_some(d)`, limbo 0x00 and 0xFF, R = 40 MiB → `stopped_early`,
    `tested ≤ R + 2·file_size`, `real_estimate` within 1 MiB below R;
  - `stale_reads_fake_stops_early`: writes past R lost, reads past R return the cell `d % R` → `stopped_early`;
  - `cache_does_not_hide_lost_writes`: lost writes + 1 MiB write cache (< guard) → `stopped_early`;
  - `partial_chip_then_wrap`: address space 64 MiB, usable 40 MiB, the rest lost, above it wraps → `stopped_early`,
    estimate within 1 MiB below 40 MiB;
  - `failing_flash_in_the_middle`: device range 20–44 MiB lost (≥ 2 files) → `stopped_early`, `first_bad` = 19 MiB,
    `real_estimate = used + first_bad` = 20 MiB;
  - `small_damaged_region_found_by_read_back`: device range 20–21 MiB lost → whether or not it stopped early,
    `bad` ≥ 1 MiB and `first_bad` = 19 MiB;
  - `drive_that_keeps_nothing`: `map = None` → stops at the first checkpoint, `real_estimate = used`;
  - `sample_media_error_stops_early`: `read_error = Other` on one sampled address → `stopped_early`;
  - `access_error_aborts`: `read_error = PermissionDenied` past some address, once during sampling and once only
    during read-back → `Err(Io)`, `cleaned`;
  - `cancel_during_sample_reads`: `cancel_at_read = (1, flag)` on a wrap fake → `Err(Cancelled)`, no op after that
    read in the log, `cleaned`;
  - `sample_time_not_in_write_stats`: genuine 64 MiB, `slow_reads = (sample, 100 ms)` → `write.seconds` < half the
    total sample delay (number of sample reads in the log × 100 ms);
  - `early_flag_in_read_progress`: on a wrap fake every `read` progress event has `early == true`; on a genuine one
    none.
  Verify `cargo test fullcheck` passes.

## 5. Window, CLI and texts

- [x] 5.1 `src/main.rs::run_full`: stage `prVerifyEarly` for read events with `p.early`, else as before; the repair
  size formula unchanged. Verify by `cargo build` and the RAM-disk run in 7.3.
- [x] 5.2 `src/cli.rs` `full`: print once to stderr `data loss found after {written} MB: writing stopped, checking
  what was written` at the first `early` read event (the JSON already carries `stoppedEarly`). Verify in 7.3.
- [x] 5.3 `src/ui/index.html`: in `taskDone` (kind `full`) store `stoppedEarly: o.stoppedEarly` in `r.capacity`; in
  `renderVerdict` append `' ' + t('fullStoppedEarly')` after the `fullBad` text when `c.stoppedEarly`. Verify with
  `UMC_DEV_SCRIPT` that injects a fake `taskDone` message with `stoppedEarly: true` (screenshot or eyeball) — the
  verdict shows the extra sentence.
- [x] 5.4 `src/ui/i18n.js`: add `prVerifyEarly` (next to `prVerify`) and `fullStoppedEarly` (next to `fullBad`) in
  all 5 languages:
  - en: `Data loss found, checking what was written…` / `The check stopped early: the drive was already losing
    data, so the rest of the free space was not written.`
  - ru: `Найдена потеря данных, проверка записанного…` / `Проверка остановилась раньше: носитель уже терял данные,
    поэтому остальное свободное место не записывалось.`
  - es: `Se encontró pérdida de datos, comprobando lo escrito…` / `La comprobación se detuvo antes: la unidad ya
    estaba perdiendo datos, así que el resto del espacio libre no se escribió.`
  - de: `Datenverlust gefunden, Geschriebenes wird geprüft…` / `Die Prüfung wurde früher beendet: das Laufwerk
    verlor bereits Daten, daher wurde der restliche freie Platz nicht beschrieben.`
  - zh: `发现数据丢失，正在校验已写入的数据…` / `检测提前结束：设备已经在丢失数据，因此剩余的可用空间没有写入。`
  Verify `node scripts/check-i18n.mjs` passes (it also sees `"prVerifyEarly"` used in `main.rs`).

## 6. Documentation

- [x] 6.1 `README.md`: Full check bullet — on a fake drive it checks samples after every 1 GB and stops soon after the
  real size, then verifies what was written (so it takes about as long as writing and reading the real part); the
  "catches everything" paragraph — it writes the free space and reads it back, stopping as soon as data is lost.
  `README.ru.md`: the same two places in Russian. Verify both files describe the same behaviour.
- [x] 6.2 Project `CLAUDE.md` "Устройство": add a `pipe.rs` line (fill / verify on a helper thread, 3 buffers of
  one call each, 4 MiB) and extend the `fullcheck.rs` line (samples after every file, early stop, `Volume` trait with a simulated
  volume in tests); `bench.rs` line: chunks of 4 MiB. Written in Russian like the rest of the file. Verify by
  reading the section.

## 7. Integration checks

- [x] 7.1 `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`,
  `node scripts/check-i18n.mjs` — all pass.
- [x] 7.2 Windows from the Mac: `cargo clippy --all-targets --target x86_64-pc-windows-msvc -- -D warnings` passes
  (no new `cfg(unix)`-only code in `pipe.rs` / `fullcheck.rs`).
- [x] 7.3 Manual on a macOS RAM disk (CLAUDE.md): `hdiutil attach -nomount ram://4194304`,
  `diskutil eraseDisk FAT32 UMCTEST MBR diskN`, `cargo build --release`, then
  `UMC_SHOW_VIRTUAL=1 target/release/usbmemorychecker --cli full diskN` → JSON with `"bad": 0`,
  `"stoppedEarly": false`, no test folder left on the volume; `--cli speed diskN 100` → `"bytes": 104857600`,
  `badBlocks` 0. Detach the RAM disk afterwards (`hdiutil detach diskN`).
- [x] 7.4 Measure the pipeline on the RAM disk: `--cli speed diskN 1000` wall-clock time (`time`) before (on `main`
  build) and after; report both numbers and the reported avg speeds in the final summary (no code change expected).

## 8. Review round 1 fixes

- [x] 8.1 Sample read retried once; early stop with a clean read-back returns `Error::Inconclusive` (`errFullInconclusive`
  in all 5 languages, mapped in `main.rs`); `fullStoppedEarly` only for a non-genuine verdict. Tests
  `a_failed_sample_read_is_tried_again`, `early_stop_with_a_clean_read_back_is_inconclusive`.
- [x] 8.2 Test `dir_volume_with_several_files` runs `run_on` over a real folder with small params (several files, readers
  open on earlier files, cancel with open readers, cleanup).
- [x] 8.3 `Checker::finish` resumes a panic of the verify thread; write loop finishes the file when the filler ends and
  asserts `c.start == written`.
- [x] 8.4 Full check uses 4 MiB calls (`Params::default().chunk`); (round 3: the speed test uses 4 MiB too).
- [x] 8.5 `rescale` message to the window after an early stop so the write curve spans the read-back width.
- [x] 8.6 README.md / README.ru.md wording ("shortly after", only the written part is checked after an early stop).
- [x] 8.7 Added tests: too little space, write error without sampling, unreadable chunk in the read-back, exact `tested` /
  `bad` after an early stop, two-file bound for a large damaged region, an unsampled small damaged region, sample plan
  for the fifth file, speed test 40 MiB points and mid-run cancel, other user files untouched.
- [x] 8.8 Visual check of the early-stop verdict done with `UMC_DEV_SCRIPT` (screenshot in the session scratchpad).

## 9. Review round 2 fixes

- [x] 9.1 Estimate: wrap branch only with proof (`pattern::is_later_block`) and capped at `used_before + q` (`after_lead`);
  tests `estimate_cases`, `partial_chip_small_hole_then_wrap`, `wrap_with_user_data`, `one_bad_block_at_the_start_is_no_wrap`,
  `wrap_proof_and_damage_after_the_lead`; spec scenarios and design D4 updated.
- [x] 9.2 `modeFullText` / `modeFullTag` / `errFullInconclusive` reworded in 5 languages; doc comments fixed.
- [x] 9.3 README.md / README.ru.md and design Risks: files already on a wrap-around fake can be damaged (back up first).
- [x] 9.4 CLAUDE.md pipe.rs / fullcheck.rs lines; cache test checks no sample is answered from the cache; `disk_full_mid_write`
  checks no sample after the last file.

## 10. Review round 3 fixes

- [x] 10.1 Wrap estimate maps damaged positions to cells `(U + q) mod real` (candidates `after_lead` and `beyond_real`),
  ignores cells below U, subtracts the guard (clamped at U); `Tally::new(total)`; tests for dead cells near the start, an
  unreadable cell, a hole in the middle, wrap + cache, updated `estimate_cases`. Spec / design D4 updated; the non-wrap
  limit is documented in Risks.
- [x] 10.2 Speed test uses 4 MiB calls (16 MiB windows kept); docs and CLAUDE.md updated.
- [x] 10.3 Nits: filler failure returns an error at once; `Progress.early` only for wrong sample data; the old test folder
  is removed before the free space is read (test); `Params::default().chunk` test; doc drift fixed.

## 11. Review round 4 fixes and the check after a repair

- [x] 11.1 Wrap estimate: `lead_later_end` / `explained_end` (wrapped part of the lead ends at the last later-position
  block), `real = tested − explained_end`; `real <= used_before` returns `real − guard`. Tests: hole at the end of the lead,
  unreadable block there, files bigger than the real size, seeded "no estimate covers a hole". Spec and D4 updated.
- [x] 11.2 CLI prints a neutral line from the outcome when the stop came from read errors only; the chart rescales whenever
  the write phase ended before the plan.
- [x] 11.3 `bench.rs` returns an error when the filler stops; test that `DirVolume::prepare` removes a leftover folder.
- [x] 11.4 Drift fixed (tasks 2.1 / 6.2, proposal i18n keys and Impact).
- [x] 11.5 Check after a repair: `verifyRepaired` request, offer card in the Repair tab waiting for the new mounted volume,
  4 i18n keys × 5 languages, spec requirement, design D10, README sentences. Verified in the window with
  `UMC_DEV_SCRIPT` and end to end on a RAM disk (repair, offer, button, full check).

## 12. Review round 5: new wrap estimate and robustness

- [x] 12.1 Wrap estimate replaced: `pattern::later_addr`, `ChunkCheck.min_delta` / `runs`, `Tally.period` / `runs`,
  `estimate` = `min(period, lowest damaged cell ≥ used_before)`, no guard subtraction, old lead logic removed.
  Tests: `estimate_cases`, `lowest_cell_cases`, pipe tests, regressions (dead cells with a cache, two holes at the
  start, small period, files bigger than the real size + cache), seeded `estimate_fuzz`. D4 and the spec rewritten.
- [x] 12.2 Helper panics become errors (`Filler`, `Checker::finish() -> Option`), `start_task` catches a task panic;
  tests at `pipe`, `fullcheck::run_on` level.
- [x] 12.3 Offer state: kept until Rust accepts, cleared when the disk goes or changes size, new-volume detection,
  disabled during tasks / update download. Real 0 shows "cannot be repaired". Nits (design wording, README wraps,
  `check-i18n` ternary keys, inconclusive-after-wrong-sample test).

## Workflow follow-up

- Review (Opus, high effort) against the specs and design; iterate fixes until clean.
- `/opsx:archive`, commit, push the `faster-full-check` branch, open the PR (merge to main is Roman's).
