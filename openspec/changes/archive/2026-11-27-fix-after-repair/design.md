# Design

## Context

Motivation: see proposal.md (Why). The current code, read and measured on v0.1.2:

- **Two copies of the truth.** Rust keeps `State.capacity: HashMap<diskN, Capacity{reported, real}>`, but only
  for a quick check that is not genuine and for a full check with `bad > 0`. The window keeps its own
  `results[id].capacity`, worked out in `taskDone` from the task result. For a passing full check it sets
  `real = reported`, while Rust stores nothing (finding B). `repairPlan()` in the window and `run_fix` in Rust each
  decide the repair size on their own.
- **Bug 1, root cause.** `run_fix` ends with `capacity.remove(&disk.id)`, and the window's `taskDone('fix')` sets
  `r.capacity = null`. After a repair neither side knows anything. The header falls back to "Not checked". The
  Repair tab falls back to `repTextFull` at `d.size` (126 GB). `run_fix` with no record sends `real = 0`, and the
  helper turns that into `eraseDisk` over the whole fake drive. Nothing in `disks.rs` looks at the partition table
  (only mounted volumes), so the app cannot even see that the drive is limited.
- **Bug 3, root cause.** Everything lives in memory, keyed by `diskN`: the Rust `HashMap` and the JS `results`
  object. Nothing is written to disk, and `diskN` changes on a replug anyway. The settings file holds only UI settings.
- **Bug 2 (Reload "hangs"), root cause.** The `rescan` IPC handler only pokes the watcher channel. `disk_watcher`
  sends a `disks` message only when `st.disks != list`. On the repaired drive nothing in the list changes, so Reload
  gets no answer at all: the icon turns once (0.8 s CSS animation) and the window stays as it was. The wrong "Not
  checked" state stays too, because that knowledge was dropped and no rescan can bring it back. There is no busy
  state and no timeout. A slow scan pass gives no sign of life either, and passes do get slow: each `diskutil` call
  is serialised behind DiskArbitration / diskmanagementd, which is busy while a freshly repaired volume is
  partitioned, mounted or fsck'ed, or while a stalled USB device is being probed. Measured on Roman's Mac with
  `disk6` connected: one `--cli list` takes ~1.0 s (7 whole disks × one `diskutil info` each, 0.1–0.17 s per call),
  and statvfs on `/Volumes/USB` takes < 1 ms. A `sample` of the running v0.1.2 app (pid 84210) showed the main
  thread idle in the run loop, the watcher thread cycling normally (posix_spawn/poll of diskutil), and the WebContent
  process at 0% CPU. So there is no deadlock and no JS busy loop. The "hang" is a Reload with no reply and no busy
  state, made worse by passes that can block on `diskutil` with no time limit. This was not reproduced as a real
  freeze; task 9.3 tries to reproduce it on a RAM disk before the fix is called done.
- **What the system tells us about disk6** (read-only `diskutil list/info -plist`): FDisk scheme, TotalSize
  125 829 120 000. Partition `disk6s1`: Size 8 303 673 344, `PartitionMapPartitionOffset` 1 048 576, Content
  `Windows_NTFS`, ExFAT, `VolumeUUID` present. The whole disk has no `DiskUUID`, no serial, no media UUID. In
  IORegistry the flash drive's own `IOUSBHostDevice` is VID 0x14CD / PID 0x1212 with **no "USB Serial Number"**.
- Constraints: WebView only on the main thread. One task at a time. `cfg(windows)` must keep compiling (clippy for
  `x86_64-pc-windows-msvc`). New UI keys go into all 5 languages. No new crates without need.

## Goals / Non-Goals

**Goals:**
- Rust is the only place that decides the capacity state and the repair plan. The window only displays them.
- Knowledge about a drive survives a repair, a restart and a replug, and a repaired drive is recognised by its layout
  even with no saved data.
- Reload always gets a reply, and one stuck system tool cannot freeze the list.

**Non-Goals:**
- Recognising a drive after it was reformatted elsewhere (its layout and volume id changed): it shows as limited or
  not checked, as the layout says.
- Identifying drive hardware (USB serial, IORegistry walk): rejected, see D3.
- Syncing records between computers, a "forget this drive" action, release notes and a version bump.
- Changing how the checks themselves measure (probe, full-check estimate, 1% margin).

## Decisions

### D1. One knowledge model in Rust (`src/capacity.rs`), the window only renders it
A new module with plain data and pure functions, so all the logic is unit-tested without drives:

- `Knowledge { declared, loss: Option<Loss{real, verdict: Fake|Damaged|Broken}>, last: Option<CheckRecord>,
  repair: Option<RepairRecord{partition, at}> }`.
- `CheckRecord { mode: Quick|Full, scope: Device|Partition, verdict: Genuine|Fake|Damaged|Broken, real, declared,
  volume (checked volume size, full only), tested, bad, stopped_early, write_error, points, seconds,
  restore_failures, at }`. It is serialised camelCase and holds everything the verdict panel shows today, so the
  panel renders the same after a restart.
- `apply_quick(&mut self, CheckInput)`, `apply_full(..)` and `apply_repair(partition | Whole, now)` implement the
  rules of the `drive-capacity-knowledge` spec. A whole-device check replaces the loss (the latest wins, like today,
  where a genuine probe drops `Capacity`). A partition-scope full check with loss takes the min of the old and new
  loss. A partition-scope pass never touches the loss.
- `view(k: Option<&Knowledge>, disk: &Disk, session_start) -> View`, serialised camelCase:
  `{ state: unknown|genuine|fake|damaged|broken|repaired|partitionOk|limited, verified, value, declared,
  partition, unused, check, remembered, repair: { default: limit|whole|none, limit, whole } }`.
  `plan` is part of the view, so the window and `run_fix` read the same numbers.

Why: findings B and the post-repair "Not checked" both come from the window and Rust deciding separately.
Alternative: patch the window to keep `r.capacity` after a repair. Rejected: Rust would still plan a full-size
repair, and nothing would survive a restart.

The window drops `results[id].capacity`, the verdict computation in `taskDone`, and its copy of `repairSize()`. Every
`disks` message now has `capacity: {diskId: View}`. `taskDone` keeps only toasts, the speed history and the session
offer after a repair (`repaired`, `offer`, …).

### D2. Partition-limited layout and the derived state
`limit_margin(declared) = max(64 MiB, declared / 10)` (the margin of a *limited layout*; the scope of a full check uses
max(64 MiB, 2%), see D8). A layout is limited when it has partitions and
`declared − max(offset + size) ≥ margin`. Its limit is the largest partition's size. The state order is the one in
the spec ("Displayed capacity state"). The "repaired" test is `end ≤ loss.real + 1 MiB` together with a limited layout or a repair record: our repair puts a partition
of `partition_size(real)` = real − 2 MiB at offset 1 MiB, so it ends at real − 1 MiB.

The "repair size of a loss" is `loss_repair_size(real, declared) = if real == 0 { 0 } else
{ partition_size(real, declared) }`. The special case matters: `partition_size(0, ..)` means "whole drive", and a quick
probe that says Broken reports `real = 0`. State `broken` ⟺ `loss_repair_size == 0`. That covers the probe's Broken
(real 0) and a full check whose estimate is only `used_before` (finding A).

The repair plan is: fake, damaged, repaired → `loss_repair_size(loss.real, declared)` (stable: a repeated repair gives
the same size); partitionOk, limited → `limit / MiB × MiB`; genuine → whole (the only default full size, see D4 item 11);
unknown, broken, unconfirmed → none (a full-size repair of an unknown drive only by the explicit tick).

Why a margin and not "matches our formula exactly": it also recognises drives limited by f3fix, by other tools or by
older versions, and the cost of a false positive is only a safer default. Why 10% / 64 MiB: a genuine drive formatted
by any OS leaves at most a few MiB at the end, a fake's unused tail is tens of percent, and 64 MiB still catches a
small card (1 GB → 300 MB).

### D3. Persistence: `capacity.json`, keyed by declared size + layout fingerprint
`Store { version: 1, drives: Vec<Record{ fingerprint, knowledge, seen }> }`, stored in `net::config_dir()` next to
`settings.json`. The load is forgiving (a missing or broken file gives an empty store). The save writes a temp file
and renames it, after every knowledge change and every re-key, and keeps the 100 most recently seen records.
`load_from/save_to(path)` exist for tests.

`Fingerprint { declared, table_id, partitions: Vec<{offset, size, id}> }`, where `id` is the volume UUID on macOS
(DiskUUID when there is no VolumeUUID) and empty on Windows, and `table_id` is the Windows MBR signature or GPT GUID
(empty on macOS). A fingerprint is usable only when it has a non-empty partition `id` or `table_id`. A match needs
exact equality.

Alternatives considered:
- `diskN` / Windows disk number: changes on replug. Rejected.
- USB serial from IORegistry (`ioreg -a -r -c IOUSBHostDevice -l`, ~0.18 s, deepest `IOUSBHostDevice` above the
  `IOMedia` with that BSD name), or `Get-Disk SerialNumber`: the bug's own drive (14CD:1212) has no serial, and fakes
  from one batch often share one. It cannot tell two units apart, and two fakes of different real sizes sharing a
  record could make the repair use the wrong size. Rejected as a key and not collected.
- `DeviceTreePath`: identifies the USB port, not the drive. Rejected.
- A marker file on the repaired volume: it writes to the user's drive, the user can delete it, and it needs a mount.
  Rejected.

The layout + UUID describes "this drive in this state", which is what the knowledge is about. When the state changes
by our own repair, the record is re-keyed (D4, only there). When it changes any other way, the stored record stays
as it is and the in-memory knowledge is shown as unconfirmed until a new check.

### D4. Watcher flow and record re-keying (revised after review rounds 1 and 2)
`State.capacity: HashMap<diskN, Entry{ knowledge, saved: Option<Fingerprint> }>`, `State.store: Store`,
`State.started: u64`. On every scan whose list changed (or a forced send):
1. Drop entries whose disk is gone or changed size (as today's `retain`). The saved records stay.
2. For a present disk without an entry: if its fingerprint is usable and a record matches, create the entry from it
   and update `seen`.
3. For a present disk with an entry whose usable fingerprint differs from `saved`: the new fingerprint is looked up
   in the store afresh (a record made by our repair replaces the entry). The stored record is NOT moved: a layout
   change we did not make may be another drive that got the same `diskN` between two scans. Without such a record
   the entry stays in memory but the view shows it as unconfirmed (no plan until a new check). The same holds for an
   entry with `saved == None` (a check made while the layout could not identify the drive) and for a current layout
   that cannot be read.
4. During a task every drive except the task's own is still synced (`skip`: its layout belongs to the task), and a scan that overlapped a
   task start or end (`State.task_gen` changed) is not applied at all. A sync that created or dropped an entry makes
   the watcher send `disks` even if the list is the same.
5. Only records made by our own repair (`Record.from_repair`) are matched for a newly connected drive. Check-only
   knowledge lives in the entry (memory) while the drive stays connected: a factory image gives drives of one batch the
   same volume UUID / MBR signature, so such a key does not name one drive. After our repair whose new layout cannot
   be used as a key (the listing failed, no id, or it equals the old layout) nothing is moved or stored under any key:
   the entry stays in memory with `unconfirmed = true` and the stored record (if any) stays as it is. Safety then rests
   on the layout-based "limited" detection and on a new check. (A pending re-key window was tried and removed: it
   could overwrite a newer check and keep an entry awaiting forever.)
6. `save_to` re-reads the file and merges by key. A record has `changed` (content written) next to `seen` (lookups);
   the one changed last wins. Removed keys are kept as tombstones with a time (`removed` in the file, merged), so a
   key one instance removed is not brought back by another.
7. Unconfirmed knowledge (round 3): `view_entry` compares the entry's layout with the drive's current one
   (`Entry::confirmed_by`); a mismatch, including a current layout that cannot be read, gives state `unconfirmed`
   (no plan, `errUnconfirmed`). The entry is kept (a failed repair can leave a disk without a table) and is replaced
   by the next check. While a task of the drive runs it is taken as it is. `record_change` never stores under the
   pre-repair layout (which a factory image shares); a failed `disks::find` is taken as a drive without a partition
   table, so the entry becomes unconfirmed (see 5). A whole-drive repair of a drive that knows nothing stores nothing.
8. A full check cut short (`CheckRecord.cut_short`) never sets `verified`; partition verified shows the smaller of the
   limit and what was written. `whole: true` carries the state and limit the window showed (`Shown`), and Rust
   refuses it when its own view differs. A cut-short pass that wrote less than 1 MiB is reported as an error and
   changes nothing (`wrote_too_little`). A computed plan size of 0 never becomes the whole drive: `view` gives
   `PlanDefault::None` (`errNoPlan`, the window asks for a check) or falls back to the layout's limit.
9. At the end of a task, a layout that cannot be read does not replace the readable one the task started with
   (`latest`), so the result is recorded for the drive that was checked. `diskutil info` of a partition that fails or
   times out leaves the drive without a partition table (unconfirmed knowledge) and does not fail the scan.
10. The store is saved outside the state lock: `Store::save_job` copies it under the lock (with an ordering number) and
   `SaveJob::run` writes it later; an older job never overwrites a newer one. A store without a path (tests) never
   writes, so no test touches the real config folder.
11. Plan invariant (round 6): `PlanDefault::Whole` exists only for state `genuine` (a whole-device quick check, or a
   full check of a volume with at most max(64 MiB, 2%) in use before); every other state has `Limit` or `None`, and the
   full size needs the explicit tick (matching shown state, limit and layout key). The helper refuses a full-size format
   without `Job.whole_confirmed`, which Rust sets only for such a plan.

A check or repair updates `Entry.knowledge` and saves at once under the current fingerprint (if usable). Right after
a successful repair, `run_fix` calls `disks::find` once and `capacity::finish_repair` puts the new layout into
`st.disks` (the `disks` message at the end of the task shows "repaired"), restores the entry if it was dropped, and
re-keys the record: the only place a record moves. `record_change` (checks and the repair) first loads a missing
entry from the store, so an older loss is not overwritten by an empty record. `Store::find` ignores records whose
knowledge names another declared size. The store is parsed per record (unreadable ones are kept as raw JSON) and
written atomically (temporary file, flushed, renamed). While the drive is unmounted during a repair its fingerprint
is not usable, so nothing is re-keyed to a half-done layout.

### D5. Repair request and helper contract
(Rounds 2-4: the window also sends `whole: true` when the user ticked the full size, with the state and limit it showed,
and the key of the layout it showed (`layout`, `Fingerprint::key`); Rust refuses the request with `errRepairChanged`
when its own current layout has another key, and accepts `whole` only as its own default plan or for the shown state.
`Job.layout` is Rust's own `Fingerprint` of the drive at that moment; the elevated helper compares it with the drive it
finds before touching it.)
The window sends `fix {disk, fs, label, plan: "limit"|"whole", size}`. `run_fix`:
1. Works out the view from Rust's own entry and the current disk.
2. `broken` → `errBroken`.
3. `limit` needs `repair.limit == Some(size)`, and `whole` needs `size == declared`. Otherwise it returns
   `errRepairChanged` (new key) without starting the helper.
4. Sends `Job { real: 0, partition: limit }` for limit and `Job { real: 0, partition: 0 }` for whole.
5. On success: `apply_repair`, re-key (D4), save, return `{size}`.

`Job` gains `#[serde(default)] partition: u64` (exact bytes; 0 = derive from `real` as before, so `--cli fix` and
old job files keep working). The new `helper::fix_size(job) -> Result<u64, String>`: with `partition > 0` it needs a
whole MiB, ≥ 1 MiB and ≤ `size`. Otherwise it uses `partition_size(real, size)`, which must be > 0. It runs first
in `run_fix` (helper side), before `unmountDisk`, `clean` or anything else, and feeds the FAT32 check,
`mac_fix_args` and `diskpart_script` (both now take the size, so they cannot disagree).

Why the window sends the size it showed: the bug class here is "window and Rust disagree". A mismatch now stops the
repair instead of erasing the drive at a size the user never saw.

### D6. Drive scan: partitions, table id, timeouts
- `Disk` gains `partitions: Vec<Partition{offset, size, id}>` and `table_id: String` (both serialised; the window
  uses neither directly).
- macOS: the partitions come from the `diskutil list -plist` entry (`Partitions[]`: `DeviceIdentifier`, `Size`,
  `VolumeUUID`). The offset comes from `diskutil info -plist <part>`
  (`PartitionMapPartitionOffset`), now called for every partition of a listed disk, not only mounted ones. The same
  info dictionary also feeds `volume()` as today. Parsing moves into pure functions (`partition_from(entry, info)`)
  tested on fixture plists. APFS containers are unchanged.
- Windows: the script adds `$parts` (every `Get-Partition`: `offset`, `size`) and `sig` / `guid` from `Get-Disk`
  (`Signature`, `Guid`). `table_id` = the GUID if it is set, else the signature as 8 hex digits if it is not 0. A generic
  `one_or_many<T>` handles one-element arrays. `win::parse` is compiled for `any(windows, test)`, so its fixture tests
  also run on the Mac.
- `net::output_timeout(cmd, limit) -> io::Result<Output>`: spawn with piped stdout/stderr, drain both in threads,
  `try_wait` every 20 ms, and on the limit `kill` + `wait` and return `ErrorKind::TimedOut`. The drive scan uses it
  for `diskutil` (10 s per call) and PowerShell (30 s). The helper's own `diskutil partitionDisk` / `diskpart` keep
  running without a limit, because formatting can take long. A timed-out list or whole-disk call → `list()` returns `Err`
  (the previous list stays); a failed or timed-out `info` of one partition lists that drive without partitions and with
  `Disk.table_unreadable` (Windows: `Get-Partition` failed, or volumes without partitions), so its knowledge is
  unconfirmed and the helper refuses to repair it.

### D7. Reload request/answer
The rescan channel becomes `Sender<u64>` (0 = internal wake-up). The window sends `rescan {seq}` with an increasing
counter. The watcher drains the channel before a pass and keeps `asked = max(seq)`. After the pass:
- `Ok(list)`: send `disks` when the list changed **or** `asked > 0`, with `rescanned: asked`.
- `Err(e)`: if `asked > 0`, send the current list with `rescanned: asked, stale: true, error: e`.
The window keeps `rescanPending`. While it is set, the button has a `busy` class (spinner animation, `disabled`), and
`rescan()` does nothing. It clears when `msg.rescanned ≥ rescanPending`. A 20 s timer clears it too and toasts
`rescanSlow`. `stale` toasts `errRescan` with the error.

One helper, `send_disks(&State, &Ui, extra)`, builds `{type: disks, disks, scanned, capacity: views, …extra}`. It is
used by `ready`, the watcher, and the end of every task (inside `start_task`, right after the `taskDone` message
while the lock is held), so the header updates without waiting for a change.

### D8. Full check: partition scope and a missing test file
`capacity::full_record` decides the scope: `Partition` when the volume misses more than max(64 MiB, 2%) of the drive, when
more than that was in use before the test (`CheckRecord.nearly_full`: the little free space written wraps onto the cells
of the old files on a fake, so such a pass is not even "partition verified"), or when the
knowledge has a repair record, or when the volume does not exceed the known loss plus 1 MiB (a fake that keeps 95%
has a tail below the margin and would otherwise pass as genuine). `Device` needs the volume to miss at most
max(64 MiB, 2%) of the drive and the partitions to end within 16 MiB of the end; anything else is `Partition`. A test
that ended by a write error or wrote clearly less than the free space is always `Partition` with real = used before +
written, and a known loss is never cleared or raised by it. `PartitionOk` (partition verified) no longer needs a limited
layout, and `verified` needs a check made after the repair.
`apply_full` records it. The verdict is worked out in Rust (`bad == 0` → genuine; else loss =
`min(E, declared)/100×99`, broken if `partition_size(loss) == 0`). `taskDone` only shows a toast whose kind comes
from the view (`check.verdict`, `check.scope`).

`fullcheck::Error::Gone` is new. A `vol.read` or `vol.create` error with `ErrorKind::NotFound` in the sample loop, in
the read-back or when creating a file returns `Gone` only when `vol.space()` fails too (the volume itself is gone); on a
mounted volume a missing file counts as damage (a wrap-around fake can overwrite the directory). Main maps it to `errDriveGone`, and the CLI prints it.
`dio::is_access_error` stays unchanged (it is also used by the probe and the speed test).

### D9. Small window fixes
- `case 'update'`: `update = msg; renderUpdate(); renderProgress();` (renderProgress → renderRepair →
  renderRepairOffer, which reads `update.downloading`).
- The "Use the full declared size" checkbox (`repWhole`, off) appears whenever the default is not the full size, except for
  broken and unconfirmed drives (neutral wording when there is no plan). It resets in
  `taskDone('fix')` and when the selection changes. `askRepair`/`doRepair` send `plan` + `size` from the view.

### D10. New window texts (all of en, ru, es, de, zh)
`realRepaired`, `realRepairedOk`, `realPartOk`, `realLimited` (hint, `{size}` declared), `verdictPartitionOk`,
`fullPartOk` (`{tested}`, `{part}`, `{size}`), `capLimitedNote` (`{part}`, `{free}`), `capRemembered` (`{date}`),
`repTextLimit` (`{size}`), `repLimitedNote`, `repWhole` (`{size}`), `repWholeWarn` (`{real}`),
`errRepairChanged`, `errDriveGone`, `errRescan` (`{err}`), `rescanSlow`. The checker already finds `t('…')` and Rust
`"err…"` keys. The new `real…` / `cap…` keys must be used through literal `t('key')` calls (no computed keys), so
`check-i18n.mjs` sees them.

## Risks / Trade-offs

- [A repaired fake that kept almost its whole declared size (unused tail below 10%) is recognised only by a stored
  record. If the record could not be stored (the listing failed, or no volume UUID), it shows "not checked" with
  no plan (the full size only by the tick) after a restart] → Known limit, not fixed: such a drive has no layout evidence. The same drive
  within one session stays unconfirmed (no plan) until a check, and any quick check settles it.

- [A genuine drive that the user keeps with a small partition shows "limited, not checked" in amber, and its Repair
  default keeps the small size] → This is the safe direction. A quick check makes it genuine, and the full size is
  one explicit tick away.
- [A record matches the wrong drive (cloned image: same size, layout and volume UUID)] → Very unlikely. The Real
  capacity tab shows "checked on <date>", and any new check replaces the record.
- [The app is closed between the end of the repair and the new volume appearing] → `run_fix` re-keys right away from
  `disks::find`. If the volume is not there yet, the layout detection still shows "limited" with the right size at
  the next start. Only the check details are lost.
- [Extra `diskutil info` per unmounted partition makes a scan slower] → Only partitions of removable disks are
  queried (usually 1–2, ~0.1 s each), and each call is time-limited.
- [Reload "hang" not reproduced as a real freeze] → The fix covers both explanations found (no reply, unbounded
  `diskutil`). Task 9.3 reproduces it on a RAM disk and records the result. If a real freeze shows up there, stop and
  report it before calling the change done.
- [Windows paths are not run on real hardware] → `win::parse` fixture tests run on both platforms, and CI runs clippy
  and tests on Windows.

## Migration Plan

Nothing to migrate: `capacity.json` is new. An older version ignores it, and deleting it only forgets the records.
The window and Rust ship in one binary, so the IPC changes need no compatibility layer. Rollback = the previous
release.
