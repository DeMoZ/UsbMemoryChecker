# Tasks

Specs: `specs/quick-capacity-check`, `specs/check-report`, `specs/drive-capacity-knowledge`, `specs/drive-repair`.
Design: `design.md` (D1–D11). Code comments are in English, except `update.rs`, `donations.rs` and `net.rs`, which
keep their Russian style (a new function in `net.rs` gets an English doc comment). Every new window string goes
into all 5 languages (`en`, `ru`, `es`, `de`, `zh`) in the same task that first uses it.

**Safety rules for every step.** Real drives are connected (Roman's fake stick as `disk6`, "Storage Device",
125.8 GB, and a 4 TB NVMe `NVME_4TB`). Never run a speed test, a check, a repair, `unmountDisk`, or any raw or volume
write on them. A write test may run only on a RAM disk you attached yourself (`hdiutil attach -nomount ram://…`) and
checked with `diskutil info diskN` (`Device / Media Name: Disk Image`, `Virtual`, the exact size). `diskN` numbers
change, so never reuse a number from an earlier run. Every manual app or CLI run uses `HOME=<scratchpad>/home`, so
nothing is written under the real `~/Library/Application Support/UsbMemoryChecker/` or `~/Library/Logs/`. Do not kill
a running `UsbMemoryChecker.app` instance. Leave no background processes (`pgrep -fl usbmemorychecker`; also
`pgrep -x yes`). No commit and no push.

## 1. Access causes (`src/dio.rs`)

- [x] 1.1 Add `pub enum AccessCause { Privacy, Busy, ReadOnly, Gone, Denied }` (serde camelCase) and
  `pub fn access_cause(e: &io::Error, elevated_raw: bool) -> Option<AccessCause>` with the mapping table of design
  D7. `Privacy` only under `cfg(target_os = "macos")` for `EPERM`. `EACCES` is `ReadOnly` when `elevated_raw`, else
  `Denied`. Build it on a pure `fn cause_of(code: Option<i32>, kind: ErrorKind, os: Os, elevated_raw: bool)`, so that
  both tables are tested on every host. Keep `is_access_error(e)` with the same result as today for every code
  (`access_cause(e, false).is_some()`). Tests: `access_causes_macos` (1 → Privacy, 13 → ReadOnly / Denied, 30 EROFS →
  ReadOnly, 16 EBUSY → Busy, 6 ENXIO and 19 ENODEV → Gone, 5 EIO → None), `access_causes_windows` (5 → Denied,
  19 → ReadOnly, 21/433/1167 → Gone, 32/33 → Busy, and no code ever gives Privacy), `is_access_error_unchanged` (the
  old list gives true, EIO gives false). `cargo test dio` passes.

## 2. Probe core (`src/probe.rs`)

- [x] 2.1 Add `fn flush(&mut self) -> io::Result<()> { Ok(()) }` as a default method of `trait Device`. Add
  `enum PointClass { Good, Mismatch, IoError }`, `struct Failure { offset, op, errno: Option<i32>, class }` (op:
  `readOld | write | readBack | baseline | verify | liveness`), and counters `mismatches`, `io_errors`, `unsaved`,
  `failures: Vec<Failure>` (first 32) plus `failures_not_listed` on `Run`. Extend `Outcome` with `mismatches`,
  `io_errors`, `unsaved`, `restore_check: RestoreCheck`, `flush: FlushResult`, all `#[serde(default)]`. Existing
  tests still pass (`cargo test probe`).
- [x] 2.2 `Undo::write` (design D2): read the old content; on a non-access error return `Unreadable` **without** pushing
  an entry or writing; otherwise push the entry and then write (`Saved` / `WriteFailed`). Access errors still abort
  (now `Abort::Access(AccessFailure { cause, op, errno, text })`, cause from `dio::access_cause(e, true)`). In `test()`
  an `Unreadable` point is `IoError` and counts `unsaved`. An `Unreadable` flush chunk ends the round with
  `Abort::Unresponsive` (D4 e). Tests: `unreadable_old_content_is_not_overwritten` (a Sim where `read` fails at one
  point address while writes would succeed: the address is never written, its cell is unchanged, `unsaved == 1`, the
  point is an I/O error), `write_failure_half_way_is_restored` (a Sim that writes the first 4 KiB of a 1 MiB flush
  chunk and then fails: afterwards `intact()`).
- [x] 2.3 Baseline (design D3): before the coarse round's first write, read S (`0..16 MiB` in 1 MiB chunks; a failed
  chunk is read again as single 4 KiB blocks), T (last 1 MiB as 4 KiB blocks) and C (every address of the coarse set),
  and keep a SHA-256 digest per 4 KiB block in a `BTreeMap<u64, [u8; 32]>`. Any failed block in S →
  `Abort::Unresponsive` before any write. Failed blocks in T / C → `no_baseline` (counted, excluded later). Factor the
  coarse-set computation out of `test()` so the baseline can use it. Tests: `unreadable_start_writes_nothing` (Sim
  with `bad_reads` at 8 MiB: `Abort::Unresponsive`, the Sim's write counter is 0), `baseline_costs_no_writes` (genuine
  Sim: the number of writes equals the number before this change, counted by a write counter on `Sim`).
- [x] 2.4 "Stopped answering" (design D4): add `fn liveness(&mut self, salt) -> Liveness { Ok, Mismatch, Error }`
  (re-read the first 1 MiB of the flush area and compare with the round's flush pattern), call it after every round's
  read-back and after 64 failed operations in a row. Add a pure `fn decide(first: PointClass, liveness: Liveness,
  …) -> Decision`: an I/O error on the first coarse point or a liveness `Error` → `Abort::Unresponsive`;
  `Mismatch` on the first point with liveness not `Error` → `Verdict::Broken`. The rest stays as today. Sim
  additions: `fail_after: Option<usize>` (every operation after the Nth returns `io::Error::other("I/O error")`),
  `fail_writes_at: Box<dyn Fn(u64) -> bool>`, an operation counter. Tests: `drive_that_fails_everything_is_not_broken`
  (every read and write fails → `Unresponsive`, writes attempted = 0), `drive_that_stops_mid_round`
  (`fail_after` = half of the coarse round's operations, never Fake / Broken, two cases: (1) the drive never answers
  again → `Unresponsive`, the write-back is tried for every written block and its failures are counted, the restore
  is not confirmed; (2) `heals_after`: it answers again after the run gave up → `Unresponsive`, `intact()`, no
  restore failure, the restore is confirmed), `one_transient_error_on_the_first_point` (only the write at the first point
  fails → `Unresponsive`), `drive_that_keeps_nothing` (unchanged expectation: Broken, now also `mismatches > 0`,
  `io_errors == 0`), `read_errors_count_as_lost_data` (still Fake, now `io_errors > 0`), `lost_writes_read_as_ff`
  (`mismatches > 0`, `io_errors == 0`), `decide_table` (every combination of first class × liveness).
- [x] 2.5 Write-back and restore check (design D5): `Undo::restore` unchanged in order; count failures as now. Then
  call `dev.flush()` → `FlushResult { Ok, Unsupported, Failed(String) }` (`ENOTTY`, `ENOTSUP`/`EOPNOTSUPP`,
  Windows 1 → Unsupported). Then re-read every block with a baseline digest and compare; after 16 failed reads in a
  row mark the rest unreadable. Fill `RestoreCheck { checked, mismatched, unreadable, no_baseline, not_reread,
  table_area, first: Vec<{offset, kind}> }` (`table_area`: a bad block below 1 MiB or in T; `not_reread`: refine-round
  blocks without a baseline). The check runs on every path (Ok, Cancelled, Access after writes, Unresponsive), and
  `probe()` returns it in both arms (change its return to a `ProbeRun { result: Result<Found, Abort>, stats,
  restore_check, flush }` or equivalent). Tests: `restore_check_genuine` (all matched, `checked > 4096`),
  `restore_check_wraparound_has_no_false_alarm` (2 TB modulo 8 GiB Sim: no mismatch), `restore_check_catches_a_lost_write_back`
  (a Sim that silently ignores writes to block 0 after the first one: `mismatched == 1`, `table_area`, first offset
  0), `restore_check_unreadable` (reads of 4 MiB fail after the restore: unreadable counted), `refine_blocks_not_reread`
  (a fake: `not_reread > 0`), `cancel_restores_the_data` (still `intact()`, and the check ran), `flush_unsupported_is_not_an_error`.
  Update the module doc comment (the "Nothing is lost" paragraph: no write without a saved copy, the check after the
  restore, what a re-read cannot prove). `cargo test probe` passes.

## 3. Device cache flush (`src/rawdev.rs`)

- [x] 3.1 `impl Device for RawDevice { fn flush }`: macOS `ioctl(fd, DKIOCSYNCHRONIZE, &dk_synchronize_t)`, a
  `#[repr(C)] struct DkSynchronize { offset: u64, length: u64, options: u32, reserved: [u8; 4] }` zeroed, with
  `const DKIOCSYNCHRONIZE: c_ulong = 0x8018_6416` (`_IOW('d', 22, 24 bytes)`). Windows: `FlushFileBuffers` from
  `windows_sys::Win32::Storage::FileSystem` (add the feature to `Cargo.toml` if it is missing). Other systems:
  `file.sync_all()`. Test `dk_synchronize_layout` (macOS: `size_of::<DkSynchronize>() == 24`, and the constant equals
  `0x8000_0000 | (24 << 16) | (b'd' as u64) << 8 | 22`). `cargo test rawdev` passes, and
  `cargo clippy --all-targets --target x86_64-pc-windows-msvc -- -D warnings` is clean.

## 4. Elevated helper (`src/helper.rs`)

- [x] 4.1 Safe files (design D10): `fn create_new_nofollow(path) -> io::Result<File>` (`create_new(true)`; Unix
  `custom_flags(libc::O_NOFOLLOW)`, Windows `FILE_FLAG_OPEN_REPARSE_POINT`). `write_json` removes an old tmp file
  (`remove_file`, which removes a link and not its target), creates the tmp file with it, writes and renames. At the
  start of `main(dir)`: `symlink_metadata(dir)` must be a directory, not a link, else write nothing and exit 1.
  Window side: `job_dir()` is created with `std::fs::DirBuilder` + `create` (not `create_all`) and on Unix
  `.mode(0o700)`. Tests (Unix): `write_json_does_not_follow_a_link` (in a temp folder, make `progress.tmp` a link to
  `target.txt`; after `write_json` `target.txt` is unchanged and `progress.json` holds the JSON),
  `helper_refuses_a_linked_job_folder`, `job_dir_is_private` (mode `0o700`). Tests use their own temp folders under
  `std::env::temp_dir()` and remove them.
- [x] 4.2 Event log: `struct EventLog` (opens `<dir>/log.jsonl` once with `create_new_nofollow`; `fn event(&mut self,
  stage: &str, ok: bool, info: Value)` writes one JSON line `{t_ms, stage, ok, info}` and flushes). If it cannot be
  created (already exists, link), the job fails before `disks::find` with "the job folder is not safe". The probe
  passes events through a callback, so `probe.rs` has no file I/O: stages `baseline`, `round`, `liveness`, `restore`,
  `flush`, `verify`, plus the first failures. Test `log_refuses_an_existing_file` and `log_lines_are_json` (3 events →
  3 lines that parse).
- [x] 4.3 Structured result (design D8): `result.json` on error = `{"ok": false, "error", "cause", "details"}`.
  `probe_result` maps `Abort::Access` → its cause, `Unresponsive` → `"unresponsive"`, `TooSmall`, `Cancelled`; an
  `open()` failure goes through `dio::access_cause` as well (it is `Abort::Access` with op `open`, so
  `EPERM` on open is `privacy`). `details` holds the counts, `restoreCheck`, `flush` and `layoutAfter`.
  `RunError::Failed` becomes `Failed { message, cause: Option<String>, details: Value }`; `run_in` fills it from
  `result.json`. Update `elevated()` and `cli.rs` to the new shape. Tests: `probe_result_causes` (each `Abort` → the
  JSON `cause`), `run_in_reads_cause_and_details` (a hand-written `result.json` in a temp folder, without starting a
  process: factor the result parsing into `fn parse_result(&[u8]) -> Result<Value, RunError>`).
- [x] 4.4 Layout afterwards (design D5): after the device is closed and remounted (macOS: keep `mountDisk`'s exit
  status and its redacted text as an event; Windows: after `drop(locks)`), poll `disks::find(id, size)` every 1 s for
  up to 10 s until `Fingerprint::of(found) == expected` (`job.layout`, else `Fingerprint::of(&disk)` taken at the
  start). Classify with a pure `fn layout_after(expected, found: Result<Disk, String>) -> LayoutAfter` (`Same`,
  `Changed`, `TableGone` = expected had partitions and found has none, `DriveGone` = "not connected" / size changed,
  `Unreadable` = `table_unreadable` or another error). Add it to the result (`layoutAfter`), also on error. Tests:
  `layout_after_cases` (the five cases built from `Disk` values, including the incident: expected one partition at
  1 MiB, found none → `TableGone`).
- [x] 4.5 Volumes per stage: events `found` (number of mounted volumes, partitions as offset / size / fs),
  `unmounted`, `remounted`, with counts only (no labels or mount paths). Check by unit test `stage_info_has_no_labels`
  (the JSON of a `found` event for a disk with label `PHOTOS` and mount `/Volumes/PHOTOS` has neither string).
  `cargo test helper` passes.

## 5. Reports (`src/report.rs`, `src/net.rs`)

- [x] 5.1 `net.rs`: `pub fn logs_dir() -> PathBuf` (macOS `$HOME/Library/Logs/UsbMemoryChecker`, Windows
  `%LOCALAPPDATA%\UsbMemoryChecker\logs`, else the cache folder), `pub fn reveal(path: &Path) -> io::Result<()>`
  (`/usr/bin/open -R`, `explorer.exe /select,<path>`), `pub fn open_privacy_settings() -> io::Result<()>` (macOS:
  `open "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"`, falling back to
  `open -b com.apple.systempreferences`; other systems: `Err(Unsupported)`). Test `logs_dir_follows_home` (macOS: with
  `HOME` set for the test process only through a helper that takes the home as a parameter, so the test does not
  change the process environment for other tests).
- [x] 5.2 `report.rs`: `struct Report` (format 1, fields of design D10) with a builder used by probe / full / fix,
  `fn redact(text: &str, disk: &Disk, home: &Path) -> String`, `fn file_name(now, kind, disk_id, taken: &dyn
  Fn(&str) -> bool) -> String`, `fn write(dir: &Path, report: &Report) -> io::Result<PathBuf>` (tmp + rename, Unix
  mode `0600`, folder `0700`; then `prune`), `fn prune(dir, now)` (pattern of D10, older than 90 days by mtime first,
  then the oldest until 50 remain), and the 32-item caps plus the 256 KiB fallback. Tests: `redact_replaces_mount_label_and_home`
  (`/Volumes/PHOTOS/UsbMemoryChecker-test/00001.bin` → `<volume>/UsbMemoryChecker-test/00001.bin`; the label as a
  whole word only; `/Users/x/…` → `<home>/…`), `report_has_no_private_fields` (serialize a full probe report for a
  disk with label, mount, UUID and table id set: none of those strings appears), `file_names_are_unique`,
  `prune_keeps_50_and_unrelated_files` (60 report files with stepped mtimes plus `notes.txt` → 50 newest plus
  `notes.txt`), `prune_drops_older_than_90_days`, `failures_are_capped` (1 875 failures → 32 listed,
  `failuresNotListed: 1843`), `report_is_small` (< 256 KiB for the worst case), `report_file_modes` (Unix). All tests
  write only to their own temp folders.
- [x] 5.3 Build the probe report from the window's view: the `Disk` at the start, the helper's `log.jsonl` lines (read
  even when the helper "stopped without a result"), `result.json` (outcome or `cause` / `details`), the window's
  decision (trusted, notice reason, error key). `helper::run` returns the log lines with the result (read before the
  job folder is removed). Test `probe_report_from_helper_files` (a temp job folder with a sample `log.jsonl` and
  `result.json` of the incident → the report has the stages, counts, `layoutAfter: tableGone` and outcome
  `errUnresponsive`).

## 6. Knowledge record (`src/capacity.rs`)

- [x] 6.1 `CheckRecord` gains `io_errors: usize`, `mismatches: usize`, `unsaved: usize` and `restore_checked: bool`
  (`#[serde(default)]` via the struct's existing `default`). `view()` and `check_fix()` are unchanged. Tests:
  `old_record_without_counts_loads` (JSON from the current format → counts 0, `restore_checked` false),
  `counts_survive_the_store` (save and load a repaired record with counts in a temp path). `cargo test capacity` passes.

## 7. Application state and tasks (`src/main.rs`)

- [x] 7.1 Notices (design D6): `State.notices: HashMap<String, Notice>` and `State.reports: HashMap<String,
  (u64 declared, PathBuf)>`. `send_disks` adds `"notices": {diskN: {kind, reason, counts}}` and `"reports": {diskN:
  true}`. `apply_scan` drops a `Replug` notice when a successful scan has no drive with that id and declared size
  (failed scans never reach it). `start_task` drops a `FullDiskAccess` notice of that drive when a check (quick or
  full) of it starts, and
  refuses any task on a drive with a `Replug` notice with `ui.error("errReplugFirst")`. Tests: `replug_notice_blocks_tasks`
  (start_task is factored so the refusal is a pure check `fn may_start(st, disk) -> Result<(), &str>`),
  `replug_notice_survives_a_failed_scan_and_ends_on_a_good_one`, `fda_notice_ends_when_the_next_check_starts`.
- [x] 7.2 `finish_probe` (design D6, D8): keep the key for each cause (`errFullDiskAccess`, `errAccessBusy`,
  `errAccessReadOnly`, `errDriveGone`, `errAccess{err}`, `errUnresponsive{io, mismatches}`). A successful run is
  trusted only when `restore_failures == 0`, `restore_check` has no mismatched or unreadable block and `layoutAfter ==
  Same`; otherwise `errNotRestored{reason, n}` and no `record()`. Set the notice from the outcome (both access and
  replug when needed). The trusted record carries the counts and `restore_checked: true`. Tests (extend the existing
  `finish_probe` tests): `incident_replay_changes_nothing` (a repaired drive's knowledge, helper error
  `cause: unresponsive` with `layoutAfter: tableGone` → `errUnresponsive`, the knowledge is equal to before, a `Replug`
  notice is set), `fake_with_unconfirmed_restore_is_not_recorded`, `trusted_fake_records_counts`,
  `privacy_cause_sets_the_fda_notice` (and no `Replug` notice when the restore check is empty), `busy_is_not_fda`.
- [x] 7.3 `plan_repair` refuses with `errReplugFirst` while the drive has a `Replug` notice, before any other check.
  Test `repair_refused_while_replug_pending` (whatever plan is named).
- [x] 7.4 IPC: `open_privacy` → `net::open_privacy_settings()` (error → `errBrowser`); `reveal_report {disk}` → the
  path from `State.reports` if its declared size matches the drive, else nothing. No path or URL is taken from the
  window. Test `reveal_report_ignores_unknown_disks` (pure lookup function).
- [x] 7.5 Reports for every run: after `run_probe`, `run_full` and `run_fix` (also on error, cancel and decline), build
  and write the report (5.2 / 5.3), store its path in `State.reports`, and `eprintln!` a failure. Full: drive, stages
  (start, end), the `FullResult` fields, the outcome key. Fix: planned size, fs, the tool's exit status and its
  redacted output (300 characters). Test `report_written_for_a_declined_probe` (call the report step with
  `RunError::Declined` and a temp logs folder: one file, outcome `errDeclined`). `cargo test` passes.

## 8. Command line (`src/cli.rs`)

- [x] 8.1 `--cli probe` / `full` / `fix`: print the error `cause` with the text, write the report the same way as the
  window, and print `report: <path>` as the last stderr line. Test `cli_error_names_the_cause` (format function).
  Manual check on a RAM disk in 12.1.

## 9. Window (`src/ui/index.html`, `src/ui/i18n.js`, `scripts/ui-check/harness.js`)

- [x] 9.1 New i18n keys in all 5 languages (`en` first, then `ru`, `es`, `de`, `zh`; same `{params}`):
  `errFullDiskAccess`, `fdaTitle`, `fdaSteps`, `fdaUpdate`, `fdaButton`, `errUnresponsive` (`{io}`, `{mismatches}`),
  `errNotRestored`, `replugTitle`, `replugText`, `replugUnresponsive`, `replugRestoreFailed` (`{n}`), `replugTableGone`,
  `replugMismatch` (`{n}`), `replugUnreadable` (`{n}`), `replugLayoutChanged`, `replugDriveGone`, `replugLayoutUnknown`,
  `errReplugFirst`, `errAccessBusy`, `errAccessReadOnly`, `errAccess` (`{err}`), `countMismatch` (`{n}`), `countIo`
  (`{n}`), `countUnsaved` (`{n}`), `restoreVerified`, `showReport`. The texts follow the specs: the FDA steps name
  System Settings → Privacy & Security → Full Disk Access, the restart, and remove-and-add-again after an update; the
  replug text says not to format, initialize or repair, even when the system offers to; no text says a re-read proves
  what is on the flash. `node scripts/check-i18n.mjs` passes.
- [x] 9.2 Notice block `#capNotice` in the Real capacity tab's outcome column (design D11): the FDA variant with the
  `open_privacy` button (only with `platform-macos`) and the replug variant with the reason line. The verdict is hidden
  while a replug notice is shown. Disable the check, speed and repair buttons of that drive with `errReplugFirst` as
  the hint; the Repair tab lead shows `errReplugFirst`. The faint line of the verdict gets the counts,
  `restoreVerified`, and a `showReport` link (`send('reveal_report', {disk})`) when `reports[disk]`; the link also
  appears under the notice. The toast for the new error keys uses `fmtArgs`.
- [x] 9.3 Harness scenarios in `scripts/ui-check/harness.js` (add `notices` / `reports` to the `disks` message the
  harness sends): `capFda` (macOS FDA notice), `capFdaWindows` (a `capReplug…` state with `platform: 'windows'`, no FDA
  button anywhere), `capReplugUnresponsive` (incident: no verdict, counts `io: 1882`), `capReplugTableGone`,
  `capReplugMismatch`, `capFakeCounts` (fake verdict with `ioErrors: 1700`, `mismatches: 50`, `restoreChecked`,
  report link), `capBrokenCounts` (broken with `mismatches: 1882`), `repReplugBlocked`, `speedReplugBlocked`,
  `toastFda`, `toastUnresponsive`, `toastReplugFirst`. Then
  `node scripts/ui-check/run.mjs --sizes 1180x760,1600x1000,880x600 --engine both` exits 0 (all states × 5 languages ×
  both themes: no page scroll, nothing cut, every ellipsis has a hint), `node scripts/ui-check/contrast.mjs`,
  `node scripts/ui-check/keys.mjs` and `node scripts/ui-check/ipc.mjs` pass. `ipc.mjs` gets a step: a click on the FDA
  button sends exactly `{action: 'open_privacy'}`, and a click on "Show report" sends `{action: 'reveal_report', disk}`.

## 10. Documentation

- [x] 10.1 `README.md` and `README.ru.md` (same content): in "What it does" / "How to use", a Full Disk Access note
  for the quick check on macOS (where to turn it on, restart, remove and add again after an update). In "How the quick
  check works": no write without a saved copy, the read-back check of the start of the drive and of every coarse
  block after the write-back, "stopped answering" vs "keeps no data", and the replug advice (with the honest limit of
  a re-read). A short "Reports" paragraph: the folder on each system, what is in them and what never is, 50 files /
  90 days. Check: both files have the same sections (`grep -c '^## '`, the same number in each).
- [x] 10.2 `CLAUDE.md`: the module list gets `report.rs` and the new helper and probe behaviour (baseline, restore
  check, layout check afterwards, event log, no-follow writes). "Важно" gets the rules: no write without a saved copy;
  Broken only from data that came back different; "stopped answering" and untrusted results change no knowledge; the
  replug block and its limit (a replug faster than one scan); Full Disk Access is macOS `EPERM` only; reports are
  written by the window, never by root. The manual-run note gets the logs folder under `HOME`. Check: the text names
  every new file and key decision.

## 11. Integration checks

- [x] 11.1 `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`,
  `cargo clippy --all-targets --target x86_64-pc-windows-msvc -- -D warnings`, `node scripts/check-i18n.mjs` all pass.
  `ls ~/Library/Application\ Support/UsbMemoryChecker/` shows no `capacity.json`, and `~/Library/Logs/UsbMemoryChecker`
  does not exist (or is unchanged): the tests wrote nothing there.

## 12. Manual checks

- [x] 12.1 RAM disk, by the agent (safety rules above): `hdiutil attach -nomount ram://4194304` → check with
  `diskutil info`, `diskutil eraseDisk FAT32 UMCTEST MBR diskN`, then
  `HOME=<scratchpad>/home UMC_SHOW_VIRTUAL=1 UMC_NO_ELEVATE=1 target/release/usbmemorychecker --cli probe diskN`. Expect
  a genuine verdict, a restore check with 0 mismatched and 0 unreadable, `layoutAfter: same`, and the report path as
  the last stderr line. The report under `<scratchpad>/home/Library/Logs/UsbMemoryChecker/checks/` contains neither
  `UMCTEST` nor the volume UUID (`grep -c`), and has mode `0600`. Detach the RAM disk (`hdiutil detach diskN`) and check
  that no `usbmemorychecker` process is left.
- [x] 12.2 Same RAM disk in the window (`HOME=<scratchpad>/home UMC_SHOW_VIRTUAL=1 UMC_NO_ELEVATE=1`, with
  `UMC_DEV_SCRIPT` for a screenshot): the verdict shows `restoreVerified` and "Show report", and pressing it reveals the
  file in Finder. Quit the app instance you started (only that one).
- [ ] 12.3 For Roman (needs the password and a change in System Settings; RAM disk only): remove UsbMemoryChecker from
  Full Disk Access, start the built app with `HOME=<scratch> UMC_SHOW_VIRTUAL=1` (elevated, without
  `UMC_NO_ELEVATE`), and run a quick check of a RAM disk. Expect the Full Disk Access notice (not a generic error), a
  report with `cause: privacy`, the `op` and `errno: 1`. This confirms the `EPERM` assumption of design D7. If the
  errno differs, record it and adjust task 1.1.
- [ ] 12.4 For Roman: press "Open Full Disk Access settings" on macOS 27 and confirm that System Settings opens at
  Privacy & Security → Full Disk Access (or at least System Settings, through the fallback).

## 13. Review fixes, round 1

- [x] 13.1 `ProbeRun.wrote` / `details.wrote`: a failed run that wrote nothing asks for no replug because of the layout.
  Test `a_failed_run_that_wrote_nothing_asks_for_no_replug_because_of_the_layout`.
- [x] 13.2 `LayoutAfter::NotCompared` when the table could not be read before the check. Tests
  `an_unreadable_start_layout_is_not_compared`, `an_unreadable_start_layout_leaves_trust_to_the_restore_check`.
- [x] 13.3 The notice is stored by `end_task`, under the lock that ends the task. Test
  `a_scan_taken_during_the_task_cannot_drop_the_new_notice`.
- [x] 13.4 `JobFolder`: the helper opens the job folder once and uses that handle for every file. Test
  `a_job_folder_swapped_for_a_link_later_gets_no_writes`.
- [x] 13.5 The Full Disk Access notice lasts until the next check; "Show report" opens a check report;
  `restoreChecked` needs `checked > 0`; an untrusted `--cli probe` exits non-zero. Tests
  `fda_notice_ends_when_the_next_check_starts`, `a_repair_report_is_not_offered_as_the_check_report`,
  `restore_checked_needs_a_block_that_was_read_again`, `cli_probe_with_an_untrusted_result_fails`.
- [x] 13.6 Redaction leaves other words and its own placeholders alone. Test
  `redact_leaves_other_words_and_its_own_placeholders_alone`.

## Workflow follow-up

- Review (Opus, high effort) of the diff against these specs; findings go back to an executor, then a new review,
  until clean.
- Release notes for the next version (`docs/releases/v<version>.md`, en + ru) mention Full Disk Access and the
  reports folder; the version and tag are Roman's release step.
- Archive the change (`/opsx:archive`) after the review is clean, then commit, push the branch and open the PR.
