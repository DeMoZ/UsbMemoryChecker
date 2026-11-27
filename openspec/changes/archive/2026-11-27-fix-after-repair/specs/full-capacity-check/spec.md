# Spec Delta

## ADDED Requirements

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

## MODIFIED Requirements

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
