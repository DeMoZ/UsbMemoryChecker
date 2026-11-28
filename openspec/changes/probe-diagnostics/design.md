# Design

## Context

Motivation: see proposal.md (Why). The current code (v0.1.3, `main` at `2731810`), read for this plan. No drive was
touched.

- **Grid on the incident drive.** For 125 829 120 000 bytes, `search()` picks the step
  `prev_pow2((size − 16 MiB) / 1024)` = 64 MiB. That gives 1 875 coarse points from 64 MiB to `size − 4096`, plus 7
  wrap targets (1 882 blocks). The drive keeps ≈ 7 921 MiB (the repaired partition is 7 919 MiB =
  `partition_size(real)`). So 123 points lie in the real part, and the first point (64 MiB) lies inside the 8.3 GB
  exFAT partition. A working run should end as fake at ≈ 7.9 GB. Writes go from the highest address down, so about
  1 750 writes hit the fake region before the real part is touched.
- **`first_bad == 0` ⇒ Broken.** `search()` returns `Verdict::Broken, real 0` as soon as the first coarse point is
  bad, without refine rounds, and whatever the reason. A point is bad when `failed.contains(p)` (its write returned a
  non-access error) or its read-back returned a non-access error or wrong data. All three cases are one boolean.
- **`Undo::write`** reads the old content. On a non-access read error it stores `None` and **still writes**.
  `Undo::restore` skips `None` entries silently, so `restore_failures` counts only failed write-backs of saved
  blocks. The flush area (`0..16 MiB`: MBR / GPT, and the boot region of a partition at 1 MiB) goes through the same
  path, and its write result is ignored.
- **Nothing checks the restore.** After the write-back the device is closed with no cache flush (no
  `DKIOCSYNCHRONIZE`, no `FlushFileBuffers`) and no read-back. `run_probe` (macOS) runs `diskutil mountDisk` and
  ignores its result. Nothing looks at the partition table afterwards.
- **Errors.** `dio::is_access_error` lumps `EPERM, EACCES, EROFS, EBUSY, ENODEV, ENXIO` (Windows 5, 19, 21, 32, 33,
  433, 1167) into `Abort::Access(text)`. The helper turns that into the string
  `"the system refused access to the drive: {e}"`, and the window shows it as `errHelper`. An `open()` failure gives
  a different text: `"could not open /dev/rdiskN: {e}"`. `result.json` has only `ok` / `error` text, with no cause.
- **Persistence.** `helper::run` deletes the job folder. `finish_probe` records the check in memory, and in
  `capacity.json` only when the drive has a record of our repair. Roman's real config folder has no
  `capacity.json` (checked read-only), so the knowledge of this drive was in memory only.
- **The toast.** The window shows `diskutil list -plist did not answer in 10 s` only as `errRescan`, the answer to a
  Reload press that met a stuck scan (`scan_reply` with `asked > 0`). So someone pressed Reload after the check while
  `diskutil list` hung for 10 s. That fits a USB device that was not answering, though the time is unknown.
- **Root and files.** The helper runs as root (`do shell script … with administrator privileges`) in a job folder
  under the user's `$TMPDIR`. The folder is made with `create_dir_all` (default mode). `write_json` writes
  `progress.tmp` / `result.tmp` with `std::fs::write`, which follows a symlink.
- **Signature.** `codesign -d -r-` on the installed v0.1.3: `designated => cdhash H"f41b…"`, `Signature=adhoc`, no
  team. So every build has another designated requirement.

### What the code explains, and what it does not (hypotheses, not conclusions)

The user saw "keeps no data", 7 s, and no "could not be written back" line. The code produces exactly that when the
drive returns **errors** (not wrong data) for the first coarse point. For example, every operation fails fast after
the drive stopped answering. Then: every point is bad ⇒ `first_bad == 0` ⇒ Broken. Every old-content read failed ⇒
every undo entry is `None` ⇒ nothing is written back and `restore_failures = 0`.

7 s also fits failing commands better than real work. The coarse round alone is ≈ 1 900 random 4 KiB writes, 1 900
old-content reads, 1 875 read-backs and 1 900 write-backs, under 1 ms per operation on average. Cheap USB sticks
usually need several ms per random 4 KiB write. This is an inference: the real per-operation time is unknown.

Two chains fit the kernel log (MBR seen, then Uninitialized, then everything back after a replug):

- **A, the drive stopped answering.** Reads fail, so the kernel's re-probe of the partition scheme finds nothing
  (Uninitialized). Writes did not reach the flash, so the power cycle brings everything back. When macOS re-probes is
  an assumption: probably when the last writer of the whole-disk node closes it, or after a USB reset.
- **B, the start was unreadable but writable.** The old-content read of the flush chunk at 0 failed, the pattern
  write went into the drive's volatile cache, and the block was never written back (a `None` entry). The kernel then
  reads the pattern (Uninitialized). The power cycle drops the cache and the original MBR is back.

Both chains run through the defects fixed here (items 2, 3 and 5 of the request). The code cannot explain why the
drive failed. A wear-out or firmware hang of a fake controller after many writes past its real size is possible but
unproven. The fake was found with the same algorithm earlier (the 7 919 MiB repair). Restore order: the reverse
write-back is correct for every saved block (the existing simulation tests prove it), so the order is not the cause.
Remount: `mountDisk` failing silently explains why nobody saw an error, not the vanished table.

A less likely chain: DiskArbitration remounting `disk6s1` during the probe while `/dev/rdisk6` is open for writing.
The report records the volume list at each stage, so a future case can rule this in or out.

## Goals / Non-Goals

**Goals:** the probe never overwrites what it could not save; it tells "lost data" from "not answering"; it proves
its restore as far as a re-read can; the user gets an actionable message for each failure; every run leaves a private,
bounded report; Windows keeps working with the same rules.

**Non-Goals:** changing the grid, the boundary search or the 1 MiB resolution; reading back after a power cycle (it
needs the user to replug and a second elevated run); a stable signing identity or notarization; per-round restore;
a report for the speed test.

## Decisions

### D1. Point classes and counts (`probe.rs`)
`test()` returns a class per point: `Good`, `Mismatch` (read back fine, data wrong) or `IoError` (the old-content
read, the write or the read-back returned a non-access error). The boundary search still treats every non-`Good`
point as bad, so a fake whose reads fail past its real size stays "fake" (existing test
`read_errors_count_as_lost_data`). The run keeps the counters `mismatches`, `io_errors`, `unsaved`, and a list of
the first 32 failures `{offset, op: read-old|write|read-back|baseline|verify, errno, class}` for the report.
`Outcome` gains these fields with `#[serde(default)]`.

### D2. No write without a saved copy
`Undo::write` returns `Saved` / `Unreadable` / `WriteFailed` / an access `Abort`. On `Unreadable` it neither pushes an
entry nor writes, and the point is `IoError`. The entry is still pushed **before** the write, so a write that fails
half-way is put back.

*Why not keep writing?* The probe's promise is "nothing is lost". A write over content we could not read can only
break that promise. It gains nothing: the address already fails to read, so the point is bad anyway, and an
unreadable high address cannot show a wrap-around in its read-back. For the start of the drive the risk is the worst
(the partition table), which is chain B above. *Alternative:* write anyway and count the block as "not restored".
Rejected, because it reports damage it could have avoided.

### D3. Baseline before the first write; the start must be readable
Before the coarse round writes anything, `Run` reads three regions. Region **S** is `0..16 MiB` in 1 MiB chunks; one
chunk that fails is read again as 256 single blocks to find the exact ones. Region **T** is the last 1 MiB, as 256
blocks. Region **C** is every block of the coarse round's set (points and wrap targets). For each 4 KiB block it keeps
a SHA-256 digest (`sha2` is already a dependency), in memory only. Any failed block in S ends the run as
`Abort::Unresponsive` **before any write**. A failed block in T or C is marked "no baseline", excluded from the
check D5 and counted. This costs ≈ 1 900 extra 4 KiB reads (a few seconds) and ≈ 70 KiB of digests on the incident
drive.

*Why a separate baseline instead of comparing with the undo copies?* On an aliasing drive the undo copy of an
address can be another address's pattern: a higher alias wrote the shared cell first. The reverse write-back still
leaves every cell at its **original** content, so only a read taken before the first write is a correct expectation.
Refine-round blocks are first touched after the coarse writes, so no clean baseline exists for them. They are put
back by the same reverse order and reported as `notReread`. *Alternative:* a full write-back after every round, so
every round starts clean. Rejected for now: it doubles the write-backs and changes the run's structure more than the
incident needs (open question 3).

### D4. "Stopped answering" rule
`Abort::Unresponsive` ends the run (followed by the write-back and D5 as usual) when:
(a) a block of S fails in the baseline;
(b) the first coarse point's class is `IoError`;
(c) the liveness read after each round's read-back fails with an error. The liveness read covers the first 1 MiB of
the flush area, which this round just wrote with a known pattern. A wrong pattern there is a `Mismatch` and does not
abort: it is evidence for "keeps nothing";
(d) 64 operations in a row failed with an error. Then the liveness read runs at once, so a hung drive with slow
timeouts does not take hours;
(e) the old-content read of a flush chunk fails in any round. The chunk is then not written (D2), so the liveness read
could not tell anything.

`Verdict::Broken` stays only for `first point == Mismatch` with the liveness read passing. The decision sits in a
pure function `decide(classes, liveness) -> Decision`, tested on its own.

### D5. Cache flush, read-back check, layout check
After `Undo::restore` the probe calls `Device::flush()`. The trait method defaults to `Ok(())`. `RawDevice` uses
macOS `ioctl(DKIOCSYNCHRONIZE, &dk_synchronize_t{0})`: `_IOW('d', 22, dk_synchronize_t)`, where
`dk_synchronize_t = {u64 offset, u64 length, u32 options, u8 reserved[4]}` is 24 bytes, so the value is `0x80186416`
(from the SDK `sys/disk.h`; assert the size in a test). On Windows it uses `FlushFileBuffers(handle)`.
`ENOTTY` / `ENOTSUP` / Windows `ERROR_INVALID_FUNCTION` give `FlushResult::Unsupported`, not an error. Then the
probe re-reads S, T and C and compares the digests. It stops early after 16 failed reads in a row (the rest count as
unreadable). Result: `RestoreCheck {checked, mismatched, unreadable, noBaseline, notReread, tableArea: bool (any bad
block in 0..1 MiB or in T), first: [..32], flush}`.

The helper then closes the device and remounts (macOS `mountDisk`, keeping its exit status and a redacted text;
Windows drops `VolumeLocks`). Then it polls `disks::find(id, size)` every 1 s for up to 10 s, until
`Fingerprint::of(found) == expected`. `expected` is `job.layout`, else the layout found at the start. The result is
`LayoutAfter: Same | Changed | TableGone | DriveGone | Unreadable`, where TableGone means no partitions where there
were some, and Unreadable means `table_unreadable` or a scan error / timeout.

*Honesty.* A matching re-read proves only that the drive **answers** with the old data now. It may come from the
controller's RAM. Only a replug followed by a new read proves what is on the flash. The spec and the texts make no
stronger claim. The note shown is "restored data read back unchanged", never "verified".

### D6. Trust and notices (`main.rs`)
A probe result is **trusted** when the run ended with an outcome, `restore_failures == 0`, the `RestoreCheck` has no
mismatched or unreadable block, and `LayoutAfter == Same`. Only a trusted result reaches `record()`; `CheckRecord`
gains `io_errors`, `mismatches`, `unsaved` and `restore_checked: bool`, all serde-default.

Everything else changes no knowledge. It sets a notice: `State.notices: HashMap<diskN, Notice{declared, kind}>`,
with `kind = FullDiskAccess | Replug{reason, counts}`. The reasons, in order of precedence: `unresponsive`,
`restoreFailed`, `tableGone`, `mismatch`, `unreadable`, `layoutChanged`, `driveGone`, `layoutUnknown`.

An access abort before any write has an empty restore check and needs no replug. One with an unconfirmed check gets
both the access message and the replug notice. `send_disks` adds `notices` and `reports: {diskN: true}` to the
`disks` message. Clearing: `FullDiskAccess` goes when the next task of that drive starts. `Replug` goes in
`apply_scan` when a **successful** scan no longer lists `(id, declared)`. Notices live in memory, so a restart clears
them too (documented limit: a replug faster than one scan interval, 2 s / 4 s, under the same `diskN` is missed).

`start_task` refuses every kind for a drive with a `Replug` notice with `errReplugFirst`. `plan_repair` refuses too,
for the IPC path and for safety. *Why block checks too?* Another write run on a drive in an unknown state can only
add risk, and the user's next step is the replug anyway.

### D7. Access causes (`dio.rs`)
`pub enum AccessCause { Privacy, Busy, ReadOnly, Gone, Denied }` and `pub fn access_cause(e) -> Option<AccessCause>`.
`is_access_error(e)` stays as `access_cause(e).is_some()`, so the full check is unchanged. Mapping:

| errno / code | macOS | Windows |
|---|---|---|
| `EPERM` (1) | Privacy | – |
| `EACCES` (13) | ReadOnly when root on a raw device, else Denied | – |
| `EROFS` | ReadOnly | – |
| `EBUSY` | Busy | – |
| `ENXIO`, `ENODEV` | Gone | – |
| 5 ACCESS_DENIED | – | Denied |
| 19 WRITE_PROTECT | – | ReadOnly |
| 21 NOT_READY, 433, 1167 | – | Gone |
| 32, 33 | – | Busy |

`ErrorKind::PermissionDenied` without a raw code maps to Denied. The probe classifies errors from **open, read and
write**. `RawDevice::open` errors come back as `Abort::Access` too, not as a free-text error. `Abort::Access` carries
`{cause, op, errno, text}`. Privacy is produced only under `cfg(target_os = "macos")`.

**Verified vs assumed:**
- Verified (incident): a System Policy / TCC denial of `file-read-data /dev/rdisk6` happened for the helper, and a
  Full Disk Access grant plus a new start stopped it.
- Verified (code): the user's wording "refused access" comes only from a read or write error, not from `open()`. So
  either the denial hit the first read, or the wording was paraphrased. Both paths are handled.
- Verified: the ad-hoc designated requirement is a cdhash. That a TCC grant then stops matching after an update is
  inferred from it, not tested.
- Assumed: a sandbox / TCC denial returns `EPERM`. MAC policy denials do so by convention, and root is not stopped by
  file permissions, so a DAC failure cannot give `EPERM` here. This is to be confirmed by manual test 12.3.
- Assumed: a write-protected medium returns `EACCES` or `EROFS` when opened for writing, a mounted volume gives
  `EBUSY`, and an unplug gives `ENXIO` / `ENODEV`. An unplug may also give `EIO`, which then lands in "stopped
  answering" with the replug advice: an acceptable outcome. Not testable without the hardware; the report keeps the
  raw errno so a wrong mapping can be seen later.

### D8. Structured helper errors
`result.json` on failure: `{"ok": false, "error": "<text>", "cause": "privacy|busy|readOnly|gone|denied|unresponsive|
tooSmall|cancelled|other", "details": {counts, restoreCheck, layoutAfter}}`. On success, `result` holds the
`Outcome` with `restoreCheck` and `layoutAfter`. `RunError::Failed` becomes `Failed{message, cause, details}`.
`elevated()` maps `cause` to window keys: `errFullDiskAccess`, `errAccessBusy`, `errAccessReadOnly`, `errDriveGone`
(existing), `errAccess{err}`, `errUnresponsive{io, mismatches}`. A successful but untrusted result maps to
`errNotRestored{…}`. `errHelper` stays for the rest. The CLI prints `cause` and the text.

### D9. Privacy settings button and report reveal (`net.rs`)
New IPC actions take no free data:
- `open_privacy`: macOS `open "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"`. If that
  spawn fails, `open -b com.apple.systempreferences`. On Windows the action does nothing. The URL is the one known for
  macOS 13–15; it is not verified on macOS 27 (manual test 12.4).
- `reveal_report {disk}`: Rust looks up `State.reports[disk]` (a path it wrote itself). macOS uses `open -R <path>`;
  Windows uses `explorer.exe /select,<path>`.

`net::open_url` keeps its https-only rule; these are separate functions with fixed commands.

### D10. Reports (`report.rs`, new)
- **Who writes.** The helper appends JSON lines to `<job>/log.jsonl`, one per stage or failure, flushed per line.
  It opens the file once with `create_new` and, on Unix, `O_NOFOLLOW` (on Windows `FILE_FLAG_OPEN_REPARSE_POINT`).
  Before anything else the helper checks with `symlink_metadata` that the job folder is a real directory; it refuses
  the job otherwise, or when `log.jsonl` already exists.
  `write_json` writes its tmp file the same way: it removes any old tmp file (that removes a link, not its target),
  creates it new and renames it. The window reads `log.jsonl` and `result.json` after the helper exits (also when it
  "stopped without a result") and builds the report **as the user**. The job folder is created with
  `DirBuilder::new().mode(0o700)` and `create` (not `create_all`), so an existing folder or link is refused.
- **Where.** `net::logs_dir()`: macOS `$HOME/Library/Logs/UsbMemoryChecker`, Windows
  `%LOCALAPPDATA%\UsbMemoryChecker\logs`, other systems the cache folder. Reports go to `checks/` inside it, as
  `YYYYMMDD-HHMMSS-<kind>-<diskN>.json` in UTC, with `-2`, `-3` … on a clash. They are written through a tmp file plus
  a rename, with mode `0600` and the folder `0700` on Unix.
- **Content (format 1).** `{format, app, os, arch, kind, startedAt, endedAt, drive{id, bus, kind, model, declared,
  blockSize, partitions[{offset, size, fs}], tableUnreadable, volumesMounted: n}, stages[{name, at, ok, info}],
  probe{grid{step, points, coarseBlocks}, counts, failures[..32], failuresNotListed, restoreCheck, flush,
  layoutBefore{n, ends}, layoutAfter}, full{…}, fix{size, fs, toolExit}, outcome{key, cause, verdict?, real?}}`.
  It holds no labels, mount paths, UUIDs, table ids, serials, contents or digests. Model and name come from the
  `Disk` the user already sees.
- **Redaction.** `redact(text, &disk, home)` replaces each mount path, each volume label (whole words, case
  sensitive) and the home folder with `<volume>` / `<home>`. It runs on every free text (OS error strings, `diskutil`
  / `diskpart` output, truncated to 300 characters). The function is pure and unit-tested.
- **Retention.** After each write, files matching `^\d{8}-\d{6}-(probe|full|fix)-[A-Za-z0-9]+(-\d+)?\.json$` are
  pruned: those older than 90 days (by mtime) go first, then the oldest until 50 remain.
- **Size.** Detailed lists stop at 32 items plus a count of the rest. A final check that the JSON is < 256 KiB drops
  `stages[].info` if needed.
- **Who calls it.** `run_probe` / `run_full` / `run_fix` in `main.rs` and the matching `--cli` paths. A report
  failure is an `eprintln!` only.

### D11. Window
The capacity tab's outcome column gets a notice block (`#capNotice`) above the verdict. The Full Disk Access variant
has a title, the steps, the update note and the button `open_privacy` (macOS only). The replug variant has a title,
the reason with its counts, and the warning "do not format, initialize or repair, even if the system offers to; then
check again". When the replug notice is shown, the verdict is hidden (the knowledge was not changed, but the last
recorded verdict could mislead). The check, speed and repair buttons of that drive are disabled, with
`errReplugFirst` as their hint. The Repair tab lead shows the replug text.

The verdict's faint line adds `countMismatch`, `countIo`, `countUnsaved` (when > 0) and `restoreVerified` (when
`restoreChecked`). `restoreFailures` stays. "Show report" appears as a small link button in the faint line, or under
the notice, when `reports[disk]`. Every new string goes into all 5 languages. Window texts follow the style of
`i18n.js`. No size is computed in the window.

## Risks / Trade-offs

- [A wrong errno mapping shows the Full Disk Access text for something else] → Privacy only for `EPERM` on macOS;
  the report keeps the raw errno; the text says that macOS blocked the access, which stays true for any MAC denial.
- [The baseline adds a few seconds and ~2 000 reads] → acceptable next to the coarse round; no extra writes.
- [A drive with slow timeouts makes the write-back take very long] → the write-back still tries every saved block
  (each one may be user data); D4(d) and the read-back cutoff stop the *diagnostic* reads early. Open question 4.
- [The re-read is served from the drive's cache] → said plainly in the texts and the report; no stronger claim.
- [A replug inside one scan interval under the same `diskN` keeps the notice] → the user can restart the app; the
  limit is documented in the spec and in `CLAUDE.md`.
- [A fake that errors on reads past its real size, but whose start then also errors once] → it ends as "stopped
  answering" instead of fake; the user re-checks after a replug. A false "stopped answering" costs a re-check; a false
  "keeps nothing" cost a usable drive.
- [More helper output in result.json] → same binary on both sides; every new field is serde-default.

## Migration Plan

No data migration: new `CheckRecord` / `Outcome` fields default to 0 / false, and older versions ignore them
(serde ignores unknown fields; `Record` parsing keeps unreadable records as `foreign`). The reports folder is new.
Rollback is the previous release; reports stay on disk and are harmless. Release notes for the next version
(`docs/releases/v<version>.md`, en + ru) mention Full Disk Access and the reports folder; that is part of the
release, not of this change.

## Open Questions

1. Full Disk Access check before the password prompt (for example, try to read a TCC-protected file from the window
   process). It would save a useless password prompt, but it is a heuristic and may break on a future macOS. Left out;
   decide after the first real reports.
2. Retention numbers (50 files, 90 days) and the "block every task until replug" choice: easy to tune later, no
   change to the approach.
3. Per-round write-back (so the refine-round blocks can be re-read too): possible follow-up if the reports show
   `notReread` blocks mattering.
4. A cap on the write-back after many failures in a row (for example, stop after 64 and count the rest as restore
   failures). Today every saved block is tried. To decide after a report from a drive with slow timeouts.
5. Turning a matching re-read into proof: after a replug, a second elevated "verify" run could compare the start of
   the drive with the digests kept in the report. Not now: the report deliberately holds no digests.
