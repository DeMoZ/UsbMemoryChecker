# Proposal

## Why

On v0.1.2 a real fake drive (declares 125.8 GB, holds ~8.3 GB) was checked and repaired, and then the app forgot
everything it had found. The header says "Not checked", and the Repair tab offers to format the drive at its full
fake 126 GB. One more press of Repair would undo the repair and the drive would lose data again. After a restart
nothing is known either. The Reload button seems to hang. The last code review found more ways to get a wrong
answer: a drive that keeps nothing gets "Repair to 0 B", and a passing full check of the repaired partition calls the
fake drive genuine at its full declared size. The app exists to make fake drives safe, so these are safety bugs.

## What Changes

- **One source of truth for "what we know about a drive"** (Rust, new module): the declared size, the current
  partition layout, the last check (quick or full, whole device or only the partition, verdict, real size, date) and
  the last repair. Rust works out the state shown in the window (not checked / genuine / fake / failing / keeps
  nothing / repaired / repaired and verified / partition verified / limited partition, not checked) and the repair
  plan, and sends them with every drive list. The window no longer works out its own verdict from task results.
- **The repair is remembered.** `run_fix` no longer drops what is known. The drive shows "Repaired" with its partition
  size, and the Repair tab keeps the real size.
- **Remembered across restarts and replugs**: a `capacity.json` in the config folder. Each record is keyed by the
  declared size plus the partition layout (offset, size and the volume UUID on macOS; the MBR signature or GPT GUID on
  Windows). The `diskN` number is not stable, so it is not part of the key.
- **A repaired drive is recognised even without a record**: a partition table whose partitions end well before the
  declared size (one 8.3 GB partition, then 117.5 GB unallocated) shows as "Partition 8.3 GB of 126 GB declared",
  not "Not checked".
- **The repair never formats a known or detected fake at its full declared size by default.** The full size needs an
  explicit choice in the Repair tab, with a warning. Rust recomputes the plan and refuses a request that does not
  match it.
- **Keeps nothing = repair size 0** (`helper::partition_size(real, reported) == 0`), in the window and in `run_fix`.
  The elevated helper refuses a partition size of 0 before it touches the drive.
- **A full check of a partition much smaller than the drive** counts as "partition verified" (real = the partition),
  never as "genuine at the declared size".
- **Reload always answers.** Rust replies to every rescan, even when nothing changed. The button stays busy until the
  reply comes, and the window says so when the system is slow. Every `diskutil` / PowerShell call of the drive scan
  has a timeout, so one drive that does not answer cannot block the whole list.
- **Drive scan gets the partition table**: on macOS every partition (offset, size, volume UUID) from `diskutil`, on
  Windows `Get-Partition` offset/size plus the disk signature/GUID.
- Smaller fixes: during the full check, a missing test file together with a volume that is gone too (its free space
  cannot be read) ends the check with an error and does not count as data loss; on a mounted volume it counts as damage. The "Check the repaired drive" button is disabled while an update downloads.
- New window texts in all 5 languages. README.md and README.ru.md describe the remembered results and the repair-size
  choice.

## Capabilities

### New Capabilities
- `drive-capacity-knowledge`: what the app knows about a drive's real capacity. Covers the record of checks and repairs,
  saving it across restarts and replugs, recognising a partition-limited (repaired) layout, and the state the window
  shows in the header and on the Real capacity tab.
- `drive-repair`: the repair plan and its safety rules. Covers which size is used by default, choosing the full declared
  size explicitly, drives that keep nothing, the helper refusing a size of 0, and Rust checking the plan before the
  helper runs.
- `drive-list`: finding the removable drives. Covers the partition table in the drive list, Reload always getting a
  reply, and timeouts on the system tools.

### Modified Capabilities
- `full-capacity-check`: "Access errors abort the run" gains the missing-test-file case. "A drive that keeps nothing
  cannot be repaired" gets the repair-size-0 rule (the old `real == 0` test never fired). "Check offered after a
  repair" disables the button while an update downloads. A new requirement makes a full check of a partition much
  smaller than the drive "partition verified", not "genuine".

## Impact

- Code: `src/main.rs` (State, `run_probe`/`run_full`/`run_fix`, the `rescan` / `fix` handlers, `disk_watcher`,
  `disks` messages), new `src/capacity.rs` (knowledge model, view, store), `src/disks.rs` (partitions, table id,
  timeouts), `src/net.rs` (command output with a timeout), `src/helper.rs` (`Job.partition`, refusal of size 0),
  `src/fullcheck.rs` (missing test file), `src/cli.rs` (prints the new error), `src/ui/index.html`, `src/ui/i18n.js`,
  `README.md`, `README.ru.md`, `CLAUDE.md`.
- New file in the user's config folder: `capacity.json` (macOS `~/Library/Application Support/UsbMemoryChecker/`,
  Windows `%APPDATA%\UsbMemoryChecker\`). No new crates.
- IPC: `rescan` gains `seq`; `fix` gains `plan` and `size`; the `disks` message gains `capacity` (a view per disk) and
  `rescanned`. Window and Rust ship together, so there are no compatibility concerns.
- Windows: the PowerShell drive script changes (partitions, signature). It must keep passing
  `cargo clippy --all-targets --target x86_64-pc-windows-msvc -- -D warnings`. CI tests it on Windows.
