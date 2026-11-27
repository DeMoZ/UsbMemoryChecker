# full-capacity-check Specification

## Purpose
The full capacity check finds out, without administrator rights and without touching the user's files, how much of a
mounted volume really keeps data: it fills the free space with verifiable test files and reads them back, stopping
early when the drive is already losing data.

## Requirements

### Requirement: Fill the free space with verifiable test data
The full check SHALL write test files of 1 GiB into a test folder on the volume until the planned amount is written:
the free space minus an 8 MiB reserve, rounded down to 1 MiB (or a smaller limit given by the caller). Every 4 KiB
block of test data SHALL depend on its position in the whole test and on a random salt of the run.

#### Scenario: Genuine volume is filled completely
- **WHEN** the full check runs on a volume that keeps all data
- **THEN** the outcome reports `tested` equal to the planned amount, `bad` = 0, `firstBad` = null and
  `stoppedEarly` = false

#### Scenario: Too little free space
- **WHEN** the planned amount is below 4 MiB
- **THEN** the check fails with the "no space" error and writes nothing

#### Scenario: Volume fills up before the plan
- **WHEN** the volume reports "disk full" while writing
- **THEN** writing stops without a write error and everything written so far is read back and checked

### Requirement: Sample check after every completed test file
After each 1 GiB test file is written and flushed, the full check SHALL read back and verify samples of 4 MiB: the
start of the new file, one more position of the new file outside its last 64 MiB, the start of the first file, and
one position in one other earlier file. No sampling SHALL happen after the last file of the write phase.

#### Scenario: Samples cover the new file, the first file and an earlier file
- **WHEN** the fifth test file has just been written and flushed
- **THEN** the check reads 4 MiB at offset 0 of file 5, 4 MiB of file 5 that ends at least 64 MiB before its end,
  4 MiB at offset 0 of file 1, and 4 MiB of one of the files 2–4

#### Scenario: Sampling cost on a genuine drive
- **WHEN** the full check runs on a genuine volume of any size
- **THEN** the bytes read by sample checks are at most 16 MiB per 1 GiB written (about 1.6%)

#### Scenario: No sampling after the last file
- **WHEN** the last planned byte has been written, or writing ended because the volume is full or a write failed
- **THEN** no sample check is made and the read-back starts

### Requirement: Stop writing when a sample is damaged
When any sample comes back different from what was written (or cannot be read even after one repeat), the full check SHALL stop writing, mark the outcome
`stoppedEarly` = true and continue with the read-back of everything already written.

#### Scenario: Drive loses writes past its real size
- **WHEN** the volume keeps data only up to a real size R and returns zeros, 0xFF or stale data past it
- **THEN** writing stops at most two test files after the test data passes R, and `stoppedEarly` = true

#### Scenario: Wrap-around fake
- **WHEN** writes past the real size R land on the start of the drive again (address modulo R, also when R is not a
  power of two)
- **THEN** writing stops at most two test files after the total test data written passes R, and
  `stoppedEarly` = true

#### Scenario: Controller cache does not hide lost writes
- **WHEN** the drive answers reads of its last few MiB of writes from a cache, and loses the data past R
- **THEN** the check still stops early, because no sample of the new file lies in its last 64 MiB

#### Scenario: Large damaged region in the middle of a genuine drive
- **WHEN** a region at least two test files long in the middle of the volume loses data and the rest works
- **THEN** writing stops at most two test files after the start of the region and `stoppedEarly` = true

#### Scenario: Small damaged region that no sample hits
- **WHEN** a region smaller than one test file loses data and no sample falls into it
- **THEN** writing goes on, and the read-back still reports the damage with the right `firstBad`

#### Scenario: A sample read fails once
- **WHEN** a sample read fails with an I/O error that is not an access error, and the repeated read succeeds
- **THEN** the sample counts as intact and writing goes on

#### Scenario: Early stop with a clean read-back is inconclusive
- **WHEN** writing stopped early because a sample showed wrong data or failed twice, but the read-back finds no damaged block
- **THEN** the check ends with an "inconclusive" error (window text `errFullInconclusive`), not with a clean result, and
  the test folder is removed

#### Scenario: Genuine drive is not stopped
- **WHEN** every sample comes back intact
- **THEN** writing continues to the planned amount and `stoppedEarly` = false

### Requirement: Read back everything that was written
After the write phase, the full check SHALL read back and verify every byte of test data that was written, also
after an early stop. `tested` SHALL be the number of bytes written and read back, `ok + bad = tested`, and
`firstBad` the position of the first damaged 4 KiB block in the test data. A read error that is not an access error
SHALL count its whole chunk as damaged.

#### Scenario: Read-back after an early stop
- **WHEN** writing stopped early after W bytes
- **THEN** all W bytes are read back, `tested` = W and `bad` counts every damaged block among them

#### Scenario: Unreadable chunk
- **WHEN** reading a chunk of a test file fails with an I/O error that is not an access error
- **THEN** that whole chunk counts as damaged and the check goes on with the next chunk

### Requirement: Real-capacity estimate
`realEstimate` SHALL be the part of the volume that keeps data: `usedBefore + tested` when nothing is damaged,
`usedBefore + firstBad` when the damage is plain data loss, and, when the drive is proven to wrap its addresses, the
measured wrap period (the real size) lowered to the lowest damaged cell, as the scenarios below define.

#### Scenario: Genuine volume
- **WHEN** no block is damaged
- **THEN** `realEstimate` = `usedBefore + tested`

#### Scenario: Lost writes
- **WHEN** the test data is intact up to position P and damaged from P on, and no block holds data of a later position
- **THEN** `realEstimate` = `usedBefore + P`

#### Scenario: Drive keeps nothing
- **WHEN** every block of test data is damaged and none holds data of a later position
- **THEN** `realEstimate` = `usedBefore`

#### Scenario: One bad block at the start is not a wrap
- **WHEN** only the first block of the test data is damaged and it holds no intact block of a later position
- **THEN** `realEstimate` = `usedBefore + firstBad` = `usedBefore`

#### Scenario: Wrap-around
- **WHEN** blocks read back hold intact data of later positions (a block at position `pos` holding position `addr`)
- **THEN** the wrap period is the smallest `addr − pos`, which is the real size R counted from the start of the device,
  and `realEstimate` is R when nothing else is damaged, also when the drive has a write cache, when the test data
  wrapped several times, and when the user's files are bigger than R

#### Scenario: Damage on a wrap-around drive
- **WHEN** blocks that are not blocks of a later position are damaged (dead or unreadable cells, lost writes), at test
  position Q
- **THEN** the damage is mapped to the cell `(usedBefore + Q) mod R`, and `realEstimate` is the lowest such cell at or
  above `usedBefore` (cells below it are the user's area and are ignored) or R if that is lower, so the estimate never
  covers damage and never exceeds R

#### Scenario: Stopped before a wrap is visible
- **WHEN** writing stopped before any block of a later position could be read back (written at most R plus the drive's
  cache) and the damage lies on cells below `usedBefore`
- **THEN** there is no period and `realEstimate` = `usedBefore + firstBad`, which can exceed R by up to `usedBefore`
  (a documented limit; the check offered after the repair shows it)

### Requirement: Access errors abort the run
A sample read or read-back read that fails because the system refused access (no rights, busy, write-protected,
device gone) SHALL end the check with an I/O error instead of counting as damage, so a blocked volume never looks
fake. A test file that is missing when the check opens it SHALL end the check with the "drive gone" error (window
text `errDriveGone`) only when the volume itself is gone too (its free space cannot be read); on a mounted volume a
missing file counts as damage. Other read errors SHALL count as damage.

#### Scenario: Volume becomes inaccessible during sampling
- **WHEN** a sample read fails with an access error
- **THEN** the check ends with an I/O error and the test folder is removed

#### Scenario: Volume becomes inaccessible during read-back
- **WHEN** a read-back read fails with an access error
- **THEN** the check ends with an I/O error and the test folder is removed

#### Scenario: Sample read fails with a media error
- **WHEN** a sample read fails with any other I/O error
- **THEN** the sample counts as damaged and writing stops early

#### Scenario: Drive unplugged during the check
- **WHEN** opening a test file for a sample read or for the read-back fails with "not found" and the volume's free
  space cannot be read either
- **THEN** the check ends with the "drive gone" error, no verdict is shown and the drive's record is unchanged

#### Scenario: A test file is missing on a mounted volume
- **WHEN** opening a test file fails with "not found" but the volume is still mounted (a wrap-around fake can
  overwrite the directory)
- **THEN** the missing data counts as damage, like any other failed read

#### Scenario: Cut short by a write error after almost nothing was written
- **WHEN** a write error ends the test after less than 1 MiB was written
- **THEN** the check is reported as failed (I/O error with the system text), the record is unchanged, and the repair
  plan is never the whole drive because of it

### Requirement: Progress and speed statistics
Progress events SHALL report `write` while filling and `read` while reading back, with the done/total bytes of that
phase and the speed of each completed 16 MiB window. Sample reads SHALL emit no progress event and their time SHALL
NOT count in the write statistics. Read-back after an early stop SHALL be marked as such.

#### Scenario: Write speed excludes samples
- **WHEN** sample reads are slow (for example 100 ms each) and writes are fast
- **THEN** `write.seconds` contains only the time of the write calls and file flushes

#### Scenario: Progress after an early stop
- **WHEN** writing stopped early because a sample came back with wrong data
- **THEN** every read-back progress event carries the early-stop mark, and the window shows the stage text
  `prVerifyEarly` instead of `prVerify`

#### Scenario: Stop caused by read errors only
- **WHEN** writing stopped because sample reads failed (no wrong data was seen)
- **THEN** the read-back progress events do not carry the early-stop mark, so the window does not claim data loss

#### Scenario: Old test folder from an interrupted run
- **WHEN** a test folder from an earlier run is still on the volume
- **THEN** it is removed before the free space is read, so its files do not shrink the planned test

#### Scenario: No foreign progress operations
- **WHEN** the check runs, with or without sample checks
- **THEN** every progress event has op `write` or `read`, and `done` never decreases within a phase

### Requirement: Cancel and cleanup
The full check SHALL use I/O calls of 4 MiB so that cancel acts within seconds on slow drives, and SHALL check for cancel before every write chunk, every sample read and every read-back chunk. The
test folder SHALL be removed when the check ends in any way: success, cancel or error.

#### Scenario: Cancel during sample reads
- **WHEN** cancel is requested while samples are being read
- **THEN** the check ends with "cancelled" before the next read or write and the test folder is removed

#### Scenario: Error while writing
- **WHEN** a test file cannot be created for a reason other than a full volume
- **THEN** the check ends with an I/O error and the test folder is removed

#### Scenario: Normal end
- **WHEN** the check completes
- **THEN** the test folder no longer exists and the user's other files are untouched

### Requirement: Test data streams without idle gaps
The full check SHALL prepare the next buffer's test data and verify the previous buffer's data while the current
read or write call is in progress. The data written and the verification result SHALL be exactly the same as with
one-buffer serial processing.

#### Scenario: Same data on the volume
- **WHEN** the full check writes test data with the pipelined writer
- **THEN** every 4 KiB block at test position p equals the block the pattern defines for position p and the run's
  salt

#### Scenario: Same verification result
- **WHEN** chunks with damaged blocks, unreadable chunks and intact chunks are verified by the pipelined reader
- **THEN** `bad`, `firstBad` and the damaged lead length equal the results of checking the same chunks one by one

### Requirement: Early stop is shown to the user
The outcome JSON SHALL include `stoppedEarly`. When it is true, the window's full-check verdict SHALL add a sentence
saying the check stopped early because the drive was already losing data and the rest of the free space was not
written. The repair size SHALL still come from `realEstimate` with the 1% margin.

#### Scenario: Verdict after an early stop
- **WHEN** a full check ends with `bad` > 0 and `stoppedEarly` = true
- **THEN** the verdict shows the "loses data" title, the damaged / tested / estimate sentence, and the early-stop
  sentence in the selected language

#### Scenario: Repair size after an early stop
- **WHEN** a full check on a wrap-around fake ends with `realEstimate` = E
- **THEN** the remembered real capacity for repair is `min(E, reported size) / 100 × 99`, as before

#### Scenario: Genuine verdict has no early-stop sentence
- **WHEN** the verdict is "genuine"
- **THEN** the early-stop sentence is not shown

#### Scenario: Chart after an early stop
- **WHEN** writing stopped early
- **THEN** the window rescales the write curve of the chart to span the same width as the read-back curve

#### Scenario: All languages translated
- **WHEN** `node scripts/check-i18n.mjs` runs
- **THEN** it passes, with the new early-stop texts present in en, ru, es, de and zh

### Requirement: Command line full check reports early stop
`usbmemorychecker --cli full <disk> [limit MB]` SHALL print the outcome JSON including `stoppedEarly`, and SHALL
print a line to stderr when writing stops early (data loss found, or a neutral line when only sample reads failed).

#### Scenario: CLI on a genuine RAM disk
- **WHEN** the CLI full check runs on a genuine RAM disk
- **THEN** stdout has the JSON outcome with `"stoppedEarly": false` and `"bad": 0`

### Requirement: Check offered after a repair
After a successful repair the window SHALL offer to check the repaired drive with the full check. The offer SHALL be
a message and a button; the check SHALL start only when the user presses the button, and on the repaired disk's new
volume.

#### Scenario: Offer shown after a successful repair
- **WHEN** a repair of a drive finishes successfully
- **THEN** the Repair tab shows the offer (text explaining that the check takes about twice the time of writing the
  real size and shows whether the size estimate was off) and the button "Check the repaired drive"

#### Scenario: No offer after a failed or declined repair
- **WHEN** the repair fails, is declined (no administrator access) or is cancelled
- **THEN** no offer is shown

#### Scenario: Button waits for the new volume
- **WHEN** the repair has just finished and the repaired disk has no mounted writable volume of the repaired size yet
  (or less than 3 s have passed since the repair)
- **THEN** the button is disabled with the text "Waiting for the drive to mount…"; after 45 s without a volume the text
  tells the user to reconnect the drive and use the Real capacity tab

#### Scenario: Button starts the full check
- **WHEN** the volume is mounted and the user presses the button
- **THEN** the normal full check starts on that disk's volume (same progress, chart and verdict), the offer is hidden
  once the check has started, and the saved check mode is unchanged

#### Scenario: Request refused
- **WHEN** the application refuses the request (another task is running, the drive is gone)
- **THEN** the offer stays and the error is shown

#### Scenario: Another task or an update download is running
- **WHEN** a task or an update download is running
- **THEN** the button is disabled

#### Scenario: Update download starts while the offer is shown
- **WHEN** the offer's button is enabled and an update download starts (the window gets an update message with
  `downloading` = true)
- **THEN** the button becomes disabled at once, and enabled again when the download ends without restarting the app

#### Scenario: Drive removed or changed
- **WHEN** the repaired disk disappears from the list or its size changes
- **THEN** the offer and the "repaired" message are dropped

#### Scenario: Old volume is not taken for the new one
- **WHEN** the drive list still shows the volume that existed before the repair
- **THEN** the button waits (the disk must have been seen unmounted, or the volume must look different, or 10 s must
  have passed)

#### Scenario: The repaired drive still loses data
- **WHEN** the check after a repair finds damaged data
- **THEN** the normal verdict and repair flow apply with the new, smaller estimate

### Requirement: Helper-thread failure is an error
A failure (panic) of a helper thread of the full check or the speed test SHALL end the run with an I/O error, never
with a panic, a hang, or a result that looks clean; a failure of a whole task SHALL release the task slot and be
reported to the window as an error.

#### Scenario: Helper thread panics
- **WHEN** the thread that prepares or verifies test data panics
- **THEN** the full check and the speed test return an error, the test folder / file is removed, and the next task can
  start

### Requirement: A drive that keeps nothing cannot be repaired
When a check's result leaves no usable size (the repair size `helper::partition_size(real, declared)` is 0), the
window SHALL show the drive as keeping no data and not offer a repair size, and the application SHALL refuse a repair
of it. A real size above 0 SHALL NOT by itself mean the drive is usable.

#### Scenario: Estimate 0
- **WHEN** a full check ends with bad data and a repair size of 0
- **THEN** the verdict and the Repair tab say that the drive keeps no data and cannot be repaired, and the repair
  button stays disabled

#### Scenario: Estimate is only the used space before the test
- **WHEN** a full check on a volume where only the test folder's first cluster (32 KiB) was in use finds every test
  block damaged, so the estimate is 32 KiB and the repair size is 0
- **THEN** the drive shows as keeping no data, the verdict has no "Repair to …" button, and the window never shows
  "Repair to 0 B"

### Requirement: Full check of a partition smaller than the drive
A full check SHALL count as a check of the whole device only when the volume misses at most max(64 MiB, 2%) of the
declared size, the partitions end within 16 MiB (`TAIL_SLACK`) of its end, and at most that much was in use before the test. Any
other volume, a volume within a known loss, and any volume of a drive our repair made smaller SHALL count as a check of
that partition only. A passing result SHALL verify the partition, never the full declared size.

#### Scenario: Passing check of a repaired fake
- **WHEN** the full check of the 8.3 GB volume on a 125.8 GB drive finds no damaged block
- **THEN** the result is "partition verified" with real = 8.3 GB. The Real capacity box does not show
  "126 GB · Verified: all real", and the Repair tab keeps 8.3 GB as the default size

#### Scenario: Passing check of a repaired fake that kept 95%
- **WHEN** a drive with a recorded loss of 120 GB and a recorded repair has a 118 GB volume (the unused tail is
  below the margin) and the full check passes
- **THEN** the scope is partition, the loss stays, and the state is repaired and verified, not genuine

#### Scenario: Volume was nearly full
- **WHEN** more than max(64 MiB, 2%) of the drive was in use before a full check, and the check passes (the little free
  space written wraps onto the cells of the old files on a fake drive)
- **THEN** it is a partition check that is not "verified", a known loss is kept, and the drive is not shown as genuine

#### Scenario: Passing check of a volume a bit smaller than the drive
- **WHEN** there is no record, the drive declares 126 GB, the partition is 119 GB and its full check passes
- **THEN** the result is "partition verified" for 119 GB, not "Verified: all real 126 GB", and the default repair
  size is the verified size

#### Scenario: Full check ended by a write error
- **WHEN** the check writes part of the free space, the drive then answers writes with an I/O error, and what was
  written reads back intact (also when less than the free space was written without an error)
- **THEN** the result is a partition check with real = the bytes in use before plus the bytes written, never
  genuine at the declared size, and a known loss is neither cleared nor raised; the window shows the write error

#### Scenario: Cut-short check on a limited or repaired drive
- **WHEN** a full check ended by a write error (or wrote clearly less than the free space) on a drive with a
  limited layout or a repaired drive
- **THEN** it never counts as "verified"; "partition verified" shows only what was written (the smaller of the limit
  and the written size), the repair plan follows that size, and the verdict says the check was cut short

#### Scenario: Passing check of a full-size volume
- **WHEN** the volume covers the declared size (at most max(64 MiB, 2%) is missing, the partitions end within 16 MiB
  (`TAIL_SLACK`) of its end) and at most that much was in use before the test, and the check passes
- **THEN** the result is genuine for the whole drive, as before

#### Scenario: Partition check finds loss
- **WHEN** the full check of a partition finds damaged data with estimate E
- **THEN** the recorded loss becomes the smaller of the known loss and `min(E, declared) / 100 × 99`, and the window
  shows the "loses data" verdict
