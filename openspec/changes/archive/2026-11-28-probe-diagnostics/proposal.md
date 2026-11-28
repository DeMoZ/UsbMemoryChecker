# Proposal

## Why

On 2026-11-28 a quick check with v0.1.3 (macOS, Apple Silicon) of Roman's repaired fake stick (`disk6`, 125.8 GB
declared, MBR, one 8.3 GB exFAT partition at 1 MiB, the rest unallocated, empty) went wrong twice:

1. The first run failed with only "the system refused access to the drive". The kernel log had
   `System Policy: usbmemorychecker deny(1) file-read-data /dev/rdisk6` and a denied
   `kTCCServiceSystemPolicyAllFiles`, although the administrator password had been accepted. After Full Disk Access
   was granted and the app was started again, there was no denial. The app is ad-hoc signed: its designated
   requirement is `cdhash H"…"` (seen with `codesign -d -r-` on the installed v0.1.3), so every new build is a
   different program for TCC.
2. The second run said "The drive keeps no data … It cannot be repaired", "Checked in 7 s". The drive is known to
   keep about 7.9 GB, so the right verdict was "fake". The kernel log showed the volume unmounted, the MBR still
   seen a few seconds later, and then `disk6` as Uninitialized with no partition table. After a physical replug the
   MBR, the partition and the volume were back. Nobody checked that the restore was byte-exact.
3. Nothing was left behind to find out what failed: the helper's job folder is deleted, and a check of a drive that
   we did not repair (in this session's state) is not written to `capacity.json`.

Reading the code (see design.md, "What the code explains"): the probe returns Broken whenever the first coarse point
(64 MiB on this drive) fails, whatever the reason. A write or read that returns an I/O error counts the same as data
that came back different. A block whose old content could not be read is still overwritten, but it is never written
back and never counted. The probe also never checks that its restore worked. A drive that stops answering in the
middle of a run therefore gets the final "cannot be repaired" verdict, with no error count and no restore warning,
which is what the user saw.

## What Changes

- **Privacy block named clearly (macOS).** When raw access to `/dev/rdiskN` fails with `EPERM`, the quick check ends
  with a Full Disk Access message. It says what is needed, where to turn it on, that the app must be started again,
  and that after an update the entry may have to be removed and added again. It has a button that opens the Full
  Disk Access pane. Other access errors (volume busy, write-protect switch, drive unplugged, anything on Windows)
  keep their own messages. Both READMEs explain it.
- **No overwrite without a saved copy.** The probe no longer writes a block whose old content it could not read. The
  block counts as an I/O error point (`unsaved`). If any block of the 16 MiB at the start cannot be read before the
  first write, the run stops before writing anything.
- **Restore is checked.** Before the first write the probe keeps a digest of the start of the drive (the first 16 MiB,
  which hold the partition table and the first volume's boot area), of the last 1 MiB (backup GPT) and of every
  coarse-round block. After the restore it asks the drive to flush its cache, reads those blocks again and compares.
  After the helper has closed the device and remounted the volumes, it looks the drive up again and compares the
  partition layout with the one from the job. A mismatch, an unreadable block, a failed write-back or a vanished or
  changed partition table makes the result untrusted. The window then tells the user to unplug and replug the drive
  before anything else, and not to format, initialize or repair it (also not when macOS offers to). The application
  refuses every task on that drive until it has left the drive list. A re-read that matches is not called proof that
  the data is on the flash: only a power cycle proves that, and the texts say so.
- **I/O errors are not data loss.** The probe counts points whose data came back different (`mismatches`) apart from
  points where a write or read returned an error (`ioErrors`). When the drive stops answering (the first point or the
  start of the drive fails with an I/O error, or the start cannot be read again after a round), the check ends as
  "the drive stopped answering", with the counts, instead of with a verdict. The knowledge and the repair plan stay
  as they were, and the window suggests a replug and a new check. "Keeps no data" (Broken) needs data that came back
  different. Fake and damaged verdicts show both counts.
- **A report for every check.** The quick check, the full check and the repair each leave a JSON report in a logs
  folder (macOS `~/Library/Logs/UsbMemoryChecker/checks/`, Windows `%LOCALAPPDATA%\UsbMemoryChecker\logs\checks\`).
  A report holds the stages, the error codes and offsets, the counts, the restore check, the layout before and after,
  and the outcome. It never holds file contents, drive data or digests of them, volume labels, mount paths, volume
  UUIDs or serial numbers. The newest 50 reports are kept, none older than 90 days. The window process writes the
  report as the user. The elevated helper only appends lines to a file in the job folder, which it opens without
  following symlinks. A "Show report" link in the Real capacity tab reveals the last check report of the drive.
- The helper's existing `progress.json` / `result.json` writes, which run as root, also stop following symlinks.

## Capabilities

### New Capabilities
- `quick-capacity-check`: the quick (raw-device) probe: what it may write, how it tells I/O errors from data loss,
  when it gives a verdict or ends as "stopped answering", how it checks its restore and the layout afterwards, how
  access errors are classified (Full Disk Access on macOS), and what the window shows for each case.
- `check-report`: the persistent per-check diagnostic report: where it is written, by whom, what it contains and
  must not contain, retention, and how the user finds it.

### Modified Capabilities
- `drive-capacity-knowledge`: the knowledge record keeps the two new counts. A quick check that ends as "stopped
  answering", or whose restore is not confirmed, changes nothing.
- `drive-repair`: a drive waiting for a replug after an unconfirmed restore cannot be repaired.

## Impact

- Code: `src/probe.rs` (point classes, unsaved blocks, baseline and restore check, stopped-answering rule, counts),
  `src/dio.rs` (`access_cause`, keeping `is_access_error`), `src/rawdev.rs` (device cache flush: `DKIOCSYNCHRONIZE` /
  `FlushFileBuffers`), `src/helper.rs` (structured errors, layout check afterwards, event log, no-follow writes,
  job folder `0700`), new `src/report.rs`, `src/net.rs` (`logs_dir`, `reveal`, opening the privacy pane),
  `src/capacity.rs` (`CheckRecord` counts), `src/main.rs` (finish_probe rules, notices, the replug block, report
  writing, new IPC actions `open_privacy` and `reveal_report`), `src/cli.rs` (prints the report path), `src/ui/index.html`
  and `src/ui/i18n.js` (notice panel, counts, links; new keys in all 5 languages), `scripts/ui-check/harness.js`
  (new scenarios), `README.md`, `README.ru.md`, `CLAUDE.md`.
- Data: `probe::Outcome` and `CheckRecord` gain fields with serde defaults, so `capacity.json` stays compatible both
  ways. The helper's `result.json` gains `cause` / `details` on errors.
- Platforms: macOS and Windows. Full Disk Access is macOS-only. On Windows the cache flush is `FlushFileBuffers` on
  the `\\.\PhysicalDriveN` handle, and the layout check afterwards uses the existing PowerShell scan.
- Not in scope: a Full Disk Access check before the password prompt (open question in design.md), notarization or
  a stable signing identity, a report for the speed test, and changes to the grid or the boundary search of the probe.
