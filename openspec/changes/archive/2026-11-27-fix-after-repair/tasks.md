# Tasks

Specs: `specs/drive-capacity-knowledge`, `specs/drive-repair`, `specs/drive-list`, `specs/full-capacity-check`.
Design: `design.md` (D1–D10). Code comments go in English, except `update.rs`, `donations.rs` and `net.rs`, which
keep their Russian style (a new function in `net.rs` gets an English doc comment).

**Safety rules for every manual step.** The real fake drive (`disk6`, "Storage Device", 125.8 GB) may only be read:
`diskutil list/info -plist`, the drive list, and Reload in the window. Never erase, partition, unmount, check or repair
it. `diskutil eraseDisk/partitionDisk` may only run on a RAM disk you just attached, and only after
`diskutil info diskN` shows `Device / Media Name: Disk Image` and `Virtual`. Run manual app sessions with
`HOME=<scratchpad>/home` so the real `~/Library/Application Support/UsbMemoryChecker/` is not touched (except in
9.5, where reading is the point).

## 1. Drive scan: partitions, table id, time limits (`src/net.rs`, `src/disks.rs`)

- [x] 1.1 Add `pub fn output_timeout(cmd: &mut Command, limit: Duration) -> io::Result<Output>` to `src/net.rs`
  (design D6: piped stdout/stderr drained in two threads, `try_wait` every 20 ms, on the limit `kill()` + `wait()`
  and `Err(ErrorKind::TimedOut)`). Unix tests: `output_timeout_returns_output` (`/bin/echo hi` → stdout `hi\n`) and
  `output_timeout_kills_a_slow_child` (`/bin/sleep 5` with 200 ms → `TimedOut` in < 2 s). `cargo test net` passes.
- [x] 1.2 In `src/disks.rs` add `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)] pub struct Partition
  { offset: u64, size: u64, id: String }` (camelCase), and `partitions: Vec<Partition>` and `table_id: String` on
  `Disk`. Update the test `main_volume_is_the_biggest_writable` and every place that builds a `Disk` (helper and cli
  tests). `cargo build` and `cargo test` pass.
- [x] 1.3 Capture fixtures, read-only: `diskutil list -plist disk6`, `diskutil info -plist disk6`,
  `diskutil info -plist disk6s1` into `src/testdata/` (`disk6-list.plist`, `disk6-info.plist`,
  `disk6s1-info.plist`). Replace the `VolumeUUID` with `00000000-0000-4000-8000-000000000001`, and replace
  `DeviceTreePath` with `IODeviceTree:/test`. Check with `plutil -lint`.
- [x] 1.4 macOS (design D6): move the per-disk parsing of `mac::list` into pure functions, e.g.
  `fn partition_from(entry: &Dictionary, info: Option<&Dictionary>) -> Partition` (offset from
  `PartitionMapPartitionOffset`, size from the entry's `Size`, id = `VolumeUUID`, else `DiskUUID`, else empty).
  Call `diskutil info -plist <part>` for every partition of a listed disk (mounted or not) and reuse that dictionary
  for `volume()`. Run every `diskutil` call of the scan through `net::output_timeout(.., 10 s)`; a timeout makes
  `list()` return `Err`. macOS-only test `parses_repaired_disk6_fixture`: from the 1.3 fixtures, disk size
  125 829 120 000, one partition {1 048 576, 8 303 673 344, the fixture UUID}, `table_id` empty. Test
  `unmounted_partition_still_listed` (the same entry without `MountPoint` and with info → listed, no volume).
- [x] 1.5 Windows (design D6): extend `SCRIPT`: `$parts` = every `Get-Partition -DiskNumber` as `{offset=[uint64]$p.Offset;
  size=[uint64]$p.Size}`, plus `sig=[uint32]$d.Signature` and `guid=[string]$d.Guid` on the disk object. Make
  `one_or_many` generic (`one_or_many<T>`) and use it for `vols` and `parts`. `table_id` = the GUID (trimmed, braces
  removed) when it is set, else `format!("{sig:08X}")` when `sig != 0`, else empty. Windows partitions get an empty
  `id`. Compile `win::parse` (and the structs it needs) for `any(windows, test)`, so its tests run on the Mac too. Run
  PowerShell through `net::output_timeout(.., 30 s)`. Tests (all platforms): `powershell_json` (existing, extended),
  `powershell_repaired_mbr_disk` (one partition object, no letter, `sig` 0x1A2B3C4D → `table_id` "1A2B3C4D",
  `volumes` empty, partition {1048576, 8303673344}), `powershell_gpt_guid` (guid "{…}" → `table_id` without braces)
  and `powershell_parts_one_or_many` (an array and a bare object). `cargo test disks` passes and
  `cargo clippy --all-targets --target x86_64-pc-windows-msvc -- -D warnings` is clean.

## 2. Helper: exact partition size and refusing size 0 (`src/helper.rs`)

- [x] 2.1 Add `#[serde(default)] pub partition: u64` to `Job` (doc: exact bytes; 0 = derive from `real`). Add
  `pub fn fix_size(job: &Job) -> Result<u64, String>` (design D5). With `partition > 0`: it must be a whole MiB,
  ≥ 1 MiB and ≤ `size`, else Err. Otherwise `partition_size(real, size)`, and 0 is Err ("the drive keeps no usable
  space; nothing to partition"). Update `job_json_round_trip` (old JSON without `partition` → 0). Test `fix_sizes`:
  real 0 → `size`; real 8 GiB → 8 GiB − 2 MiB; real 1 MiB and real 2 MiB + 5 → Err; partition 8 303 673 344 →
  Ok(same); partition 8 303 673 345 → Err; partition `size + 1 MiB` → Err.
- [x] 2.2 Change `mac_fix_args(disk, job, size)` and `diskpart_script(number, job, size)` to take the size from
  `fix_size`. In the helper-side `run_fix`, call `fix_size` first, before the FAT32 check, `unmountDisk` or diskpart,
  and return its error. Update `mac_commands` / `diskpart_commands`, and add `exact_partition_commands`: partition
  8 303 673 344 on a 125 829 120 000-byte job → mac args end with `"8303673344", "FREE", "gap", "R"`, diskpart has
  `create partition primary size=7919`. Test `run_fix_refuses_size_zero_before_touching_the_drive`: make the
  refusal reachable without a drive (e.g. a `fn plan_fix(job) -> Result<(u64, Vec<String>), String>` used by
  `run_fix`) and assert Err for real = 1 MiB. `cargo test helper` passes. Check that `--cli fix <disk> 0` still
  builds the whole-disk job (read `cli.rs`; `partition` = 0).

## 3. Knowledge model and view (`src/capacity.rs`)

- [x] 3.1 Create `src/capacity.rs` (`mod capacity;` in main.rs) with a module doc and the types of design D1:
  `Knowledge`, `Loss`, `CheckRecord`, `RepairRecord`, enums `Mode`, `Scope`, `Verdict` (serde camelCase, Clone,
  Debug, PartialEq), `const MIB`, `fn limit_margin(declared) -> u64` (max(64 MiB, declared/10)),
  `fn loss_repair_size(real, declared) -> u64` (0 for real 0, else `helper::partition_size`), and
  `fn layout_limit(disk: &Disk) -> Option<u64>` (D2). Tests: `margin_values` (126 GB → 12.58 GB; 300 MB → 64 MiB),
  `layout_limit_of_repaired_disk6` (built from the 1.3 fixture numbers → Some(8 303 673 344)),
  `layout_not_limited_when_full`, `layout_not_limited_without_partitions`, `small_card_limited` (1 GB/300 MB),
  and `loss_repair_size_zero_for_zero_and_tiny` (0, 1 MiB, 2 MiB + 1 → 0; 8 GiB → 8 GiB − 2 MiB).
- [x] 3.2 Implement `Knowledge::new(declared)`, `apply_quick(&mut self, rec: CheckRecord)` and
  `apply_full(&mut self, rec: CheckRecord)` (scope comes from the caller, see 6.3), and
  `apply_repair(&mut self, Repair::Partition(size) | Repair::Whole, now)`, with the rules of the spec requirement
  "Knowledge record per drive" (a device-scope result replaces the loss; a partition-scope loss takes the min; a
  partition-scope pass leaves the loss alone; a whole repair removes `repair`). One test per spec scenario:
  `quick_fake_records_loss`, `device_pass_clears_loss`, `full_loss_uses_99_percent`, `partition_loss_takes_min`,
  `partition_pass_keeps_loss`, `repair_keeps_knowledge`, `whole_repair_drops_repair_entry`.
- [x] 3.3 Implement `fn view(k: Option<&Knowledge>, disk: &Disk, session_start: u64) -> View` and the
  `RepairPlan { default: Limit|Whole|None, limit: Option<u64>, whole: u64 }` inside it, with the state order of the
  spec requirement "Displayed capacity state" and the plan of design D2 (`value`, `partition`, `unused`,
  `verified`, `remembered = check.at < session_start`). Tests, one per scenario of "Displayed capacity state" and
  "Default repair size follows the drive's state": `state_after_repair_is_repaired` (the bug report: 125.8 GB,
  loss 8.3 GB, partition 8 303 673 344 → repaired, plan Limit 8 303 673 344), `repaired_then_verified`,
  `reformatted_full_size_is_fake_again`, `limited_without_record` (plan Limit = partition rounded down to MiB),
  `broken_by_tiny_loss`, `broken_by_quick_real_zero` (plan None, never the whole size), `genuine_with_small_partition`,
  `unknown_full_partition_plans_whole`, `repeated_repair_same_size` (loss_repair_size of the repaired drive equals its
  partition), and `view_json_shape` (serde_json of a view has the camelCase keys the window uses).
- [x] 3.4 Implement `fn check_fix(view: &View, plan: &str, size: u64) -> Result<u64 /* Job.partition, 0 = whole
  drive */, &'static str>` (design D5: `errBroken` for broken whatever the plan; `errRepairChanged` for a limit that
  does not match, a limit plan with no limit, a whole plan with `size != declared`, or an unknown plan).
  Tests: `fix_limit_ok`, `fix_limit_mismatch_refused`, `fix_limit_without_limit_refused`, `fix_whole_ok`,
  `fix_broken_refused_for_both_plans`. `cargo test capacity` passes.

## 4. Persistence and record matching (`src/capacity.rs`)

- [x] 4.1 Implement `Fingerprint::of(disk)` and `usable()`, plus `Store { version, drives: Vec<Record> }` with
  `load_from(path)` (missing / invalid → empty), `save_to(path)` (temp file + rename, creating the folder),
  `find(&Fingerprint) -> Option<&Knowledge>` (marks `seen`), `put(old: Option<&Fingerprint>, new: Fingerprint,
  knowledge, now)` (replaces the record with `old` or with `new`, else inserts; keeps the 100 most recently seen) and
  `fn path() -> PathBuf` (`net::config_dir().join("capacity.json")`). Tests in a temp folder
  (`std::env::temp_dir()` + a unique name, removed at the end): `store_round_trip`, `broken_file_gives_empty_store`
  (writes `not json`; the next save writes valid JSON), `fingerprint_without_ids_never_matches`,
  `match_needs_size_layout_and_id` (a different size, offset or UUID → no match), `put_rekeys_instead_of_duplicating`,
  `evicts_oldest_over_100`.
- [x] (replaced by 13.7) 4.2 Implement the watcher sync as a pure function `fn sync(entries: &mut HashMap<String, Entry>, store: &mut
  Store, disks: &[Disk], now) -> bool /* store changed */` (design D4 steps 1–3; `Entry { knowledge, saved:
  Option<Fingerprint> }`). Tests: `sync_loads_record_on_replug_with_new_id` (record for disk6's layout, disk listed
  as `disk7` → entry for disk7), `sync_drops_gone_disk_keeps_record`, `sync_rekeys_after_layout_change` (an entry
  whose `saved` is the old layout; the disk now has a new partition/UUID → the store has one record with the new
  key), `sync_skips_unusable_fingerprint` (a disk with no partitions during a repair → no re-key). `cargo test
  capacity` passes.

## 5. Full check: missing test file (`src/fullcheck.rs`)

- [x] 5.1 Add `Error::Gone`. In `run_inner`, a `vol.create`, sample `vol.read` or read-back `vol.read` error with
  `kind() == ErrorKind::NotFound` returns `Err(Error::Gone)` (before the "count as damage" branches). Update the
  `match` in `src/cli.rs` if needed (it prints `{e:?}`). Tests with `SimVolume` (add a switch that makes reads of
  file k fail with NotFound): `missing_file_in_sample_is_gone` and `missing_file_in_readback_is_gone`. Both return
  `Err(Error::Gone)`, not an `Outcome` with `bad > 0`, and the test folder is cleaned up. The existing
  `fullcheck` tests still pass.

## 6. Application wiring (`src/main.rs`)

- [x] 6.1 State: replace `capacity: HashMap<String, Capacity>` with `HashMap<String, capacity::Entry>`, add `store:
  capacity::Store` (loaded with `Store::load_from(Store::path())` at start) and `started: u64`. Change the rescan
  channel to `Sender<u64>`. Add `fn send_disks(st: &State, ui: &Ui, extra: Value)`, which sends `{type: "disks",
  disks, scanned, capacity: {id: view}}` merged with `extra`. Use it in `ready`. `cargo build` passes.
- [x] 6.2 `disk_watcher` (design D7, D4): drain the channel into `asked = max(seq)`. On `Ok(list)`: run
  `capacity::sync`, save the store if it changed, and `send_disks` when the list changed or `asked > 0` (extra
  `{"rescanned": asked}`). On `Err(e)` with `asked > 0`: `send_disks` with `{"rescanned": asked, "stale": true,
  "error": e}`. The `rescan` IPC handler reads `seq` (new optional `Request.seq: Option<u64>`, 0 when absent) and
  sends it. `start_task`: after the `taskDone` message, call `send_disks(&st, &ui, json!({}))` while holding the
  lock, then poke the watcher with 0.
- [x] (`run_full` / `run_probe` replaced by `finish_full` / `finish_probe` and `full_record`, see 11.8, 13.4) 6.3 `run_probe` / `run_full`: build a `CheckRecord`. Quick: verdict, real (genuine → declared, else
  `o.real`), points, seconds, restore_failures. Full: tested, bad, stopped_early, write_error, seconds,
  volume = `o.total`, scope = `Partition` when `declared − o.total ≥ limit_margin(declared)` (design D8),
  real = `min(E, declared) / 100 × 99` when `bad > 0`, `o.total` for a partition pass and declared for a device
  pass. Verdict: genuine when `bad == 0`, broken when `loss_repair_size(real) == 0`, else fake.
  Apply it to the entry (create one if none), then save under `Fingerprint::of(&disk)` if usable (`store.put(saved,
  fp, …)`, set `saved`). `run_full` maps `fullcheck::Error::Gone` → `Err(("errDriveGone", {}))`.
- [x] (replaced by `plan_repair` / `finish_repair`, see 11.2, 13.1, 13.8) 6.4 `fix` request: add `plan: Option<String>` and `size: Option<u64>` to `Request`. `run_fix(ctx, fs, label,
  plan, size)` computes the view from the entry and `ctx.disk`, calls `capacity::check_fix` (on Err returns
  `(key, {})` with no helper), runs the job with `partition`, then `apply_repair`. Then `disks::find(&id, size)`
  refreshes the layout; save under the new fingerprint if usable, else under the old one. It no longer removes the
  entry. Returns `{"size": partition or declared}`.
- [x] 6.5 Verify the Rust side as a whole: `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo clippy --all-targets --target x86_64-pc-windows-msvc -- -D warnings` and `cargo fmt --check` are all clean.
  `grep -n "capacity.remove" src/main.rs` finds nothing.

## 7. Window (`src/ui/index.html`, `src/ui/i18n.js`)

- [x] 7.1 Add the keys of design D10 to `I18N` in en, ru, es, de and zh (same `{params}` in every language; the
  Russian and other texts are natural, not word-for-word). `node scripts/check-i18n.mjs` passes and its key count
  grows by the number of new keys.
- [x] 7.2 Data flow (design D1): keep `capacity = {}` from every `disks` message (`msg.capacity`) and add
  `function cap(id)`. Remove `results[id].capacity`, the verdict computation in `taskDone` for `probe` / `full`
  (keep the toasts, now chosen from `cap(msg.disk)` after the following `disks` message, or from the task result's
  verdict), the `capacity.reported !== d.size` reset, and `repairSize()`. `taskDone('fix')` keeps
  `repaired`/`offer`/timers, no longer touches capacity, and unticks `repWhole`.
- [x] 7.3 Header and Real capacity tab: `renderDriveView` fills `mReal`/`mRealHint`/the box class per state, as in
  the spec requirement "Header real-capacity box" (`realUnknown`, `realOk`, `realFake`, `realDamaged`,
  `verdictBroken`, `realRepaired`/`realRepairedOk`, `realPartOk`, `realLimited`). The tab badge follows the spec
  scenario "Capacity tab badge". `renderVerdict` reads `cap().check`: a partition-scope pass uses
  `verdictPartitionOk` + `fullPartOk`; a remembered check adds `capRemembered` with a localised date
  (`toLocaleDateString(LOCALE[lang])`). A limited layout shows the `capLimitedNote` block. The "Repair to …" button
  shows only for fake/damaged with `repair.limit > 0` and uses `repair.limit`.
- [x] 7.4 Repair tab (spec `drive-repair`): `repairPlan()` returns the view's plan, with `size = repWhole ticked ?
  whole : (default == limit ? limit : whole)`. Lead texts per state: `repText` (fake/damaged), `repTextLimit`
  (repaired/partitionOk/limited), `repTextFull` (unknown/genuine), `errBroken` (broken). Notes: `repNeedCheck`
  (unknown), `repLimitedNote` (limited). Add a `repWhole` checkbox + `repWholeWarn` note, shown only when
  `default == limit`, reset on drive selection change and after a repair. `doRepair` sends `{disk, fs, label,
  plan: whole ticked or default whole ? 'whole' : 'limit', size}`. The button stays disabled for broken.
  `errRepairChanged` shows as an error toast through the normal `taskDone` error path.
- [x] 7.5 Reload (design D7): `rescan()` does nothing while `rescanPending`. Otherwise it increments `rescanSeq`,
  sets `rescanPending`, adds a `busy` class (infinite spin, `disabled`), sends `rescan {seq}` and starts a 20 s timer
  (clear + toast `rescanSlow`). In `case 'disks'`: when `msg.rescanned >= rescanPending`, clear it. When
  `msg.stale`, toast `errRescan {err: msg.error}`.
- [x] 7.6 `case 'update'`: also call `renderProgress()` (spec scenario "Update download starts while the offer is
  shown").
- [x] 7.7 Check the window: `node scripts/check-i18n.mjs` passes, and `cargo build --release` embeds the page. Then
  drive it on a RAM disk (recipe in CLAUDE.md, `UMC_SHOW_VIRTUAL=1 UMC_NO_ELEVATE=1`,
  `HOME=<scratchpad>/home`) and take screenshots (`screencapture -x`) for these: (a) a fresh RAM disk erased to a
  full-size FAT32 → "Not checked", Repair plans the whole size; (b) the same RAM disk partitioned as
  `diskutil partitionDisk diskN 2 MBR ExFAT UMCTEST 300M FREE gap R` → "limited" header (amber, 300 MB), the
  `capLimitedNote`, and the Repair default 300 MB with the full-size option off; (c) a full check of that volume
  → "partition verified", Repair still 300 MB; (d) `UMC_DEV_SCRIPT` sending an `update` message with
  `downloading: true` while the post-repair offer is shown → button disabled (use a repair on the RAM disk, which is
  allowed: `UMC_NO_ELEVATE=1`). Detach the RAM disk at the end (`hdiutil detach diskN`).

## 8. Documentation

  Result: (a) a full-size FAT32 RAM disk showed "Not checked" and the repair plan was the whole 419 MB; (b) the 300M layout showed "limited" (amber, 300 MB), the unused-space note and a 300 MB default with the full-size box off; (c) a full check of that volume gave "The partition is real", "Partition verified", and the repair stayed at 300 MB; (d) with the offer shown, an `update` message with `downloading: true` disabled "Check the repaired drive".
- [x] 8.1 README.md and README.ru.md, together: in "What it does" → Repair, say that results are remembered per drive
  (`capacity.json` next to the settings), that a repaired drive is recognised by its partition layout, and that the
  Repair tab keeps the real size unless "use the full declared size" is ticked. In "How to use", step 4: the
  new-size choice. Both files say the same things.
- [x] 8.2 CLAUDE.md (Russian, like the rest of the file): add `capacity.rs` to "Устройство" (knowledge, view, plan,
  `capacity.json`, key = size + layout + volume UUID / MBR signature), note in "Важно" that Rust is the only source of
  the state and the repair plan (the window does not compute `repairSize` any more), the `Job.partition` /
  `fix_size` rule, and the `HOME=<folder>` trick for manual runs. Fix the existing line about `repairSize`.

## 9. Integration checks

- [x] 9.1 Full set: `cargo test`, `cargo clippy --all-targets -- -D warnings`,
  `cargo clippy --all-targets --target x86_64-pc-windows-msvc -- -D warnings`, `cargo fmt --check` and
  `node scripts/check-i18n.mjs` are all green.
- [x] 9.2 Restart and replug persistence on a RAM disk (`HOME=<scratchpad>/home`): partition the RAM disk as in 7.7(b),
  start the app, quit. Write a `capacity.json` record by hand for its fingerprint (size, offset/size/VolumeUUID from
  `diskutil info -plist`) with loss {real: 302 MiB + 2 MiB, fake} and a quick check record, then start the app → the
  header shows "Repaired" and the Real capacity tab shows the remembered check with its date. Press Repair with the
  default size → the confirmation names the partition size, and after the repair (allowed on the RAM disk with
  `UMC_NO_ELEVATE=1`) the drive is still "Repaired", the offer is shown, and after a restart it is still recognised
  (re-keyed to the new UUID). Detach and attach a new RAM disk with the same layout (new UUID) → no match, state
  limited. Record the results in the task notes.
  Result: with a hand-written record the header showed "Repaired" and the Real capacity tab showed the remembered
  check with its date; the confirmation named 317 MB (302 MiB); after the repair the state was still "Repaired", the
  offer was shown and `capacity.json` held the new volume UUID (re-keyed); a repeated repair gave the same 317 MB; a
  new RAM disk with the same layout and a new UUID showed "limited" (no match).
- [x] 9.3 Reproduce Reload before and after: with the installed v0.1.2 binary
  (`/Applications/UsbMemoryCheker.app/Contents/MacOS/usbmemorychecker`, a second instance, same env vars;
  `UMC_DEV_SCRIPT` and `UMC_SHOW_VIRTUAL` work in release builds) and with the new build, on the 7.7(b) RAM disk,
  press Reload through
  `UMC_DEV_SCRIPT` (`document.getElementById('rescanBtn').click()` every 3 s, logging `Date.now()` for each `disks`
  message to `document.title`). Old build: no `disks` message when nothing changed. New build: one reply per press,
  < 3 s. If the old build really freezes (window does not respond), stop and report the stack (`sample <pid> 3`)
  instead of finishing.
  Result: old v0.1.2 build, 5 presses 3 s apart on an unchanged RAM disk: no `disks` message at all. New build: one
  reply per press, 1.8-2.0 s each. The old window stayed responsive (its timers and clicks worked), so no real freeze
  was reproduced: the "hang" is a Reload with no answer.
- [x] 9.4 Time limit in the app: in a debug build only, let `UMC_DISKUTIL=<path>` replace `/usr/sbin/diskutil` in
  the drive scan (`#[cfg(debug_assertions)]`, documented in CLAUDE.md next to the other `UMC_*` variables). Point it at
  a scratchpad script that runs the real diskutil but sleeps 15 s on `info`. Expect Reload to show `errRescan` after
  ~10 s while the window keeps the old list, and the next normal scan to work again.
  Result: with the slow `info` flag set, Reload got a `stale` answer after 11.7 s ("Couldn't refresh the drive list: diskutil info -plist disk0 did not answer in 10 s"), the window kept the 3-drive list; the next scan after the flag was removed answered in 2.9 s.
- [x] 9.5 Read-only look at the real drive: start the new build with the normal `HOME` and do nothing in it except
  select `disk6` and press Reload. Expect: `disk6` shows "limited" (amber, 8.3 GB of 126 GB), the Real capacity tab
  shows the limited note, the Repair tab defaults to 8.3 GB with the full-size option off, and Reload answers.
  Confirm `capacity.json` was not created by just looking (`ls ~/Library/Application\ Support/UsbMemoryChecker/`).
  Take a screenshot. Do not press Check, Repair or Speed.

## 10. Review round 1 (fixes)

  Result: `disk6` showed "limited" (amber 8.3 GB of 126 GB), the limited note, a Repair default of 8.3 GB with the full-size box off, and Reload answered. `capacity.json` was not created. No Check, Repair or Speed was pressed.
- [x] 10.1 Scope of the full check and the repaired state: `capacity::full_scope` / `full_record` (scope Partition for
  a volume within a known loss or any volume with a recorded repair), `view` derives "repaired" also from the repair
  record; `run_full` uses `full_record`. Tests `passing_check_of_a_repaired_fake`, `passing_check_of_a_full_size_volume`,
  `partition_check_finds_loss`, `estimate_that_is_only_the_used_space_keeps_nothing`,
  `repaired_near_full_fake_stays_repaired`, `volume_within_a_known_loss_is_a_partition_check`.
- [x] 10.2 A record moves only after our own repair: `sync` drops the association on another layout change and looks
  the new fingerprint up; no sync during a task; `record_change` (`rekey` only for the repair) and `finish_repair`.
  Tests `sync_does_not_move_a_record_on_a_layout_change`, `swap_between_scans_keeps_both_records`,
  `finish_repair_rekeys_and_shows_repaired`. Spec and design updated.
- [x] 10.3 Window: `doRepair` returns without a dialog; sends the disk the dialog was opened for; a vanished disk gives
  `errNoDisk`, a changed size `errRepairChanged`.
- [x] 10.4 `disks.rs`: a failed `diskutil info` of a partition leaves the disk without a partition table
  (`failed_partition_info_leaves_no_table`, `timed_out_partition_info_fails_the_scan`).
- [x] 10.5 The new layout goes into `st.disks` at the end of the repair (`finish_repair`), tested.
- [x] 10.6 Store: per-record parsing with raw JSON for unreadable records, `serde(default)` on the record types,
  `find` ignores records about another declared size (`foreign_record_does_not_flap`, `store_keeps_unreadable_records`),
  atomic flushed write.
- [x] 10.7 `record_change` loads an older record before creating an empty one (`record_change_loads_the_older_record_first`).
- [x] 10.8 Texts in all 5 languages: `repTextLimited`, `repWholeWarnLimited` (for limited without a check), a neutral
  `capLimitedNote`; README.md and README.ru.md say precisely which size is proposed.
- [x] 10.9 `net::output_timeout` kills and waits for the child when `try_wait` fails.
- [x] 10.10 Nits: dead `.icon-btn.spin` CSS and the stale `cfg_attr` on `mac_fix_args` removed. Caching of
  `diskutil info` between scans was not done: the list output does not carry the `Writable` flag of a volume, so a
  cache could show a stale read-only state. Tests added: `create_not_found_is_gone`, `reload_is_always_answered`
  (`scan_reply` extracted in `main.rs`).
- [x] 10.11 Results of 7.7, 9.4, 9.5 recorded below; GUI re-check on a RAM disk (repair, "Repaired", the offered check
  gives "partition verified", the plan stays; a 95% layout from a recorded loss).
  Result: (1) record loss 304 MiB, layout 300M; default repair 317 MB (302 MiB); after the repair the list at the end of
  the task already showed "Repaired" (317 MB); the offered check gave "The partition is real", header "Repaired and
  verified", plan still 317 MB. (2) a 380 MiB partition on the 400 MiB RAM disk with a recorded loss of 382 MiB and a
  repair of 380 MiB (tail below the margin): header "Repaired", plan 398 MB; after a full check "Repaired and
  verified", never "Verified: all real", plan 398 MB.

## 11. Review round 2 (fixes)

- [x] 11.1 Blocker: a full check ended by a write error (or with clearly less written than the free space) is a
  partition check with real = used before + written and never clears or raises a known loss
  (`FullResult.used_before`, `write_error_pass_never_clears_a_known_loss`, `write_error_pass_without_a_record_is_not_genuine`,
  `a_test_that_wrote_much_less_than_the_free_space_is_partial`, `a_full_check_that_ended_by_a_write_error_keeps_the_known_loss`).
- [x] 11.2 `scan_reply(asked, Ok(changed || synced))`; the window sends `whole` (explicit tick) and `check_fix` accepts the
  full size only as the default plan or explicitly (`stale_whole_request_for_a_late_loaded_record_is_refused`,
  `a_repair_request_that_does_not_match_never_reaches_the_helper`).
- [x] 11.3 Scope Device only for a volume missing at most max(64 MiB, 2%) with partitions ending within 16 MiB of the
  end; `PartitionOk` without a limited layout (`a_volume_a_bit_smaller_than_the_drive_is_a_partition_check`).
- [x] 11.4 Only records made by our repair are matched (`Record.from_repair`); check-only knowledge stays in memory
  (`only_records_made_by_our_repair_are_matched`, `a_check_without_a_repair_stays_in_memory`). README texts say it.
- [x] 11.5 (replaced in round 4, see 13.1) Pending re-key for 60 s after a repair whose new layout cannot name the drive
  (`rekey_after_repair_waits_for_a_usable_layout`).
- [x] 11.6 `State.task_gen`: a scan that overlapped a task start or end is not applied.
- [x] 11.7 `Job.layout`, checked by the helper before it touches the drive (`a_swapped_drive_is_refused_before_anything_is_touched`).
- [x] 11.8 `run_full` builds the record inside `record_change` (from the entry or the store); `plan_repair` uses
  `entry_for` (entry, else store).
- [x] 11.9 `verified` needs a check made at or after the repair (`verified_needs_a_check_made_after_the_repair`).
- [x] 11.10 An entry without a key never attaches to another drive (`an_entry_without_a_key_does_not_attach_to_another_drive`).
- [x] 11.11 `save_to` merges with the file on disk (`save_merges_what_another_instance_wrote`).
- [x] 11.12 Nits: `repWholeWarn` gets the real size of the loss (`View.loss_real`); the full-size tick resets when the
  state or plan of the drive changes; `UMC_DISKUTIL` is ignored in the elevated helper.
- [x] 11.13 Tests: `output_timeout_kills_a_slow_child` bound 4.5 s, `sample_time_not_in_write_stats` compares with the summed
  sample time; `failed_cancelled_or_gone_checks_leave_the_record_unchanged` (also the `Gone` mapping),
  `a_drive_that_keeps_nothing_is_refused_for_every_plan`. A speed test never touches the record (`run_speed` has no
  knowledge access); a declined repair or check returns before any record call.
- [x] 11.14 GUI re-check on a RAM disk: repair, "Repaired", the offered check, "partition verified"; a restart with the
  scratch `HOME` shows the repaired drive remembered.
  Result: record of a repaired fake (key from our repair) on the 300M layout; the repair at the default 317 MB, the header
  "Repaired", the offered check gave "Repaired and verified" (partition scope, `capacity.json` re-keyed with
  `fromRepair: true`); after a restart the header, Repair tab (317 MB, full-size box off) and the Real capacity tab
  ("The partition is real", "checked on 11/27/2026") show the drive remembered.

## 12. Review round 3 (fixes)

- [x] 12.1 A failed `disks::find` after a repair is taken as a drive without a partition table; `record_change` never
  stores under the pre-repair layout (`review3_find_failure_keys_record_under_pre_repair_layout`).
- [x] 12.2 Unconfirmed knowledge: `Entry::confirmed_by`, `view_entry`, `State::Unconfirmed`, `errUnconfirmed`; window
  text in 5 languages (`review3_unusable_swap_inherits`, `unconfirmed_also_before_the_first_sync_with_a_usable_other_layout`).
- [x] 12.3 Cut-short check: `CheckRecord.cut_short`, never `verified`, partition verified shows the written size,
  `fullPartOk` names the verified amount, `fullCutShort` text (`review3_partial_pass_on_limited_layout`).
- [x] 12.4 (removed in round 4, see 13.1) Pending re-key: same `diskN` first, drives connected at the end of the repair excluded, a whole-drive repair needs
  a partition that reaches the end (`pending_prefers_the_same_disk_and_skips_drives_that_were_connected`,
  `a_full_size_repair_waits_for_a_partition_that_reaches_the_end`).
- [x] 12.5 `Record.changed` and tombstones in `capacity.json` (`merge_goes_by_the_time_of_the_change_and_keeps_removals_removed`).
- [x] 12.6 `whole` carries the shown state and limit; refused when they differ (`whole_needs_the_state_the_window_showed`).
- [x] 12.7 README (false positives, layout check), CLAUDE.md (scope rule, `Job.layout`), German `errRepairChanged` formal; a
  full-size repair of an unknown drive stores nothing (`a_full_size_repair_of_an_unknown_drive_stores_nothing`).
- [x] 12.8 Tests: `apply_scan` extracted (`a_scan_that_overlapped_a_task_is_not_applied`,
  `a_scan_that_changed_only_the_knowledge_is_sent`), `finish_probe` extracted
  (`a_declined_or_failed_quick_check_leaves_the_record_unchanged`), `cli_fix_builds_a_job_without_a_plan_or_layout`.
- [x] 12.9 GUI re-check on a RAM disk: repair, "Repaired", the offered check, verified; a restart shows it remembered.
  Result: seeded record of a repaired fake (key from our repair) on the 300M layout; repair at the default 317 MB, header
  "Repaired", the offered check (partition scope, not cut short) gave "Repaired and verified"; after a restart the header, the
  Repair tab (317 MB, full-size box off) and the Real capacity tab ("The partition is real", "checked on 11/27/2026") show it
  remembered. The machine was under heavy load from other processes (load average above 90) during the runs, so the repair took
  longer than before; the script waited for the offer instead of using fixed times.

## 13. Review round 4 (simplification and fixes)

- [x] 13.1 The pending re-key is removed (`Store.pending`, `Pending`, `awaiting_key`, `pending_matches`, the 60 s window and its
  tests). After our repair the record moves only when `disks::find` gives a usable layout that differs from the pre-repair
  one; otherwise nothing is stored under any key, the entry stays in memory with `unconfirmed = true`, and the stored record
  is untouched (`review3_find_failure_keys_record_under_pre_repair_layout`).
- [x] 13.2 Tests never touch the real config folder: `Store` keeps its path (`Store::open`), a store without a path (every test
  store) never writes (`save_job` returns None). Verified: no `capacity.json` in the real config folder after the tests.
- [x] 13.3 The store is saved outside the state lock (`SaveJob`, ordered by a number); `Store::put` clears an old tombstone
  of its key.
- [x] 13.4 A layout that cannot be read at the end of a task does not replace the readable one the task started with
  (`latest`; `a_transient_unreadable_layout_at_the_end_of_a_check_keeps_the_result`).
- [x] 13.5 `Gone` only when the volume itself is gone (`space()` fails); a missing test file on a mounted volume is damage
  (`a_missing_test_file_on_a_mounted_volume_is_damage`, the `Gone` tests set `volume_gone`).
- [x] 13.6 A `diskutil info` of a partition that fails or times out lists that drive without a partition table and does not
  fail the scan (`timed_out_partition_info_lists_the_drive_without_a_table`).
- [x] 13.7 A known fake whose layout changes to another usable layout while connected is shown as unconfirmed (no plan)
  (`sync_does_not_move_a_record_on_a_layout_change`).
- [x] 13.8 The repair request carries the key of the layout the window showed (`layout`); Rust refuses a different one
  (`a_repair_request_for_another_layout_than_the_window_showed_is_refused`).
- [x] 13.9 Nits: spec and README mention `unconfirmed`; design D4 and the entry-without-a-key scenario reworded; CLAUDE.md
  `disks.rs` line; this section replaces the pending text in design and specs.
- [x] 13.10 GUI re-check on a RAM disk (scratch HOME): repair, "Repaired", the offered check, verified; restart, remembered.
  Result: seeded record of a repaired fake on the 300M layout; default repair 317 MB, then the offered check gave "Repaired and
  verified" (partition scope, not cut short, `capacity.json` re-keyed with `fromRepair`); after a restart the Real capacity tab shows
  "The partition is real", "checked on 11/27/2026".

## 14. Review round 5 (fixes)

- [x] 14.1 A computed size of 0 never becomes the whole drive: `view` gives no plan (`PlanDefault::None`, `errNoPlan`, the
  window asks for a check) or the layout's limit; a Limited drive with a sub-MiB largest partition has no plan; a cut-short pass
  that wrote less than 1 MiB is an `errIo` error and changes nothing (`wrote_too_little`). Other places where 0 could become
  Whole: the PartitionOk plan (value rounds to 0), the Limited plan, `check_fix` accepting `whole` for a plan of None;
  `loss_repair_size` and the helper's `fix_size` already refuse 0 (the CLI's `real 0` is the explicit whole drive).
  Tests: `early_write_error_on_a_limited_layout_keeps_the_limit_plan`, `early_write_error_with_a_repair_record_and_no_loss_keeps_the_plan`,
  `nothing_written_is_a_failed_check_not_a_record`, `a_limited_drive_with_a_sub_mib_partition_has_no_plan`,
  `a_partition_pass_that_verified_nothing_falls_back_to_the_layout_limit_or_no_plan`,
  `a_full_check_that_wrote_almost_nothing_is_an_error_and_changes_nothing`.
- [x] 14.2 Spec and design text: the known limit of a near-full repaired fake without a stored record; "Drive reformatted or
  swapped while connected" says unconfirmed; `Gone` only when the volume is gone (spec, D8, proposal); D3 says re-keyed only by
  our repair; "diskutil does not answer" names the list and whole-disk calls; D4 numbering.
- [x] 14.3 Nits: the dead branch in `record_change` is removed; `build_disk` doc; the Windows script calls `Get-Partition` once per
  disk; `Job.layout` doc; `realUnconfirmedHint` and the `repWholeWarn` choice (a partition pass without a loss uses the
  "current partition" wording), `errNoPlan` in 5 languages.

## 15. Review round 6 (fixes)

- [x] 15.1 The full declared size is never a default except for a genuine drive: `view` (Unknown -> no plan, a plan size not
  below the declared size -> no plan), `check_fix` (the tick for every non-default state), the window (the tick offered for no-plan
  states with neutral wording, leads that name the state), `Job.whole_confirmed` and the helper refusal
  (`full_size_is_a_default_only_for_a_genuine_drive`, `a_full_size_format_needs_the_confirmation`).
- [x] 15.2 A full check of a nearly full volume is a partition check, not verified (`CheckRecord.nearly_full`,
  `zz_used_volume_pass_clears_known_loss`, `zz_sweep_full_results_never_whole_for_known_or_limited`).
- [x] 15.3 `Disk.table_unreadable` (macOS and Windows) gives unconfirmed and a helper refusal
  (`zz_table_unreadable_hides_a_stored_repaired_fake`, `powershell_unreadable_table_is_flagged`). Also fixed: an empty `parts` array was read
  as one partition (the untagged `one_or_many` order).
- [x] 15.4 Minor: `repLimitedNote` hidden without a plan; spec drive-repair default size text; design D8/D6; a record with a loss of the
  whole size is ignored; `sample_time_not_in_write_stats` has a larger delay; README (5 languages of window text).
- [x] 15.5 GUI check on a RAM disk: unknown drive -> no plan and the tick; repaired drive -> the partition size, the full size only with the tick.

## 16. Review round 7 (fixes)

- [x] 16.1 Windows: `Get-Partition` "not found" is not a failure (a blank disk is not unreadable); the knowledge of an unreadable table
  shows again once it reads fine (`a_blank_disk_is_not_unreadable_and_offers_the_tick`, `powershell_unreadable_table_is_flagged`).
- [x] 16.2 During a task the scan still drops knowledge of drives that vanished or changed, except the task's own drive
  (`sync(.., skip)`, `a_vanished_drive_loses_its_knowledge_during_a_task_of_another_drive`).
- [x] 16.3 `--cli fix` needs a numeric size; only `0` (or a size not below the drive's) confirms the whole drive
  (`cli_fix_builds_a_job_without_a_plan_or_layout`).
- [x] 16.4 Doc drift (design D2, D4 numbering, D9, Risks; specs; CLAUDE.md); a partition-verified drive with the tick gets a matching
  lead (`repWholeLeadVerified`, 5 languages); the tick resets when the layout key changes.

## 17. Final small round

- [x] 17.1 Docs: D4 item 4 and the spec say every drive except the task's own is synced during a task (new scenario); `--cli fix` rule in
  drive-repair and CLAUDE.md; `TAIL_SLACK` wording in the full-check spec.
- [x] 17.2 Windows script: `partsfail` also when no partitions came back although `NumberOfPartitions` > 0; one comment in `apply_scan`;
  `--cli probe` never parses a size.

## Workflow follow-up

- Review (Opus, high effort) of the diff against these specs, iterate until clean, then `/opsx:archive`.
- Commit on `fix-after-repair`, push, PR to `main` (merging to main is Roman's step).
- Release notes and a version bump are a separate step (not in this change).
