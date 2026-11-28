# Spec Delta

## Purpose

The quick check finds the real capacity of a whole device from about a thousand points, run by the elevated helper
on the raw device. This capability says what the probe may write, how it tells a drive that loses data from a drive
that stops answering, how it proves its restore, and what the user is told in each case.

## ADDED Requirements

### Requirement: A block is written only after its old content was saved
The probe SHALL write a block only after it has read that block's current content. A block whose old content cannot
be read SHALL NOT be written. It SHALL count as an I/O error point when it is a test point, and as an unsaved block
in the result in every case.

#### Scenario: Old content of a point cannot be read
- **WHEN** reading the old content at one test address fails with a media error, while writes there would succeed
- **THEN** nothing is written at that address, its content is the same after the probe as before, the point counts
  as an I/O error point, and the result reports one unsaved block

#### Scenario: The start of the drive cannot be read before the first write
- **WHEN** any block of the first 16 MiB cannot be read before the probe has written anything
- **THEN** the probe writes nothing at all and ends as "the drive stopped answering"

### Requirement: Everything written is put back in reverse order
Whatever the result (a verdict, a cancel, an access error, "stopped answering"), the probe SHALL write back every
saved block before it returns, in the reverse order of the writes, so that every cell gets its old content back,
also when several addresses share one cell. A write-back that fails SHALL count as a restore failure.

#### Scenario: Wrap-around fake
- **WHEN** a 2 TB drive that wraps every address modulo 8 GiB is probed
- **THEN** the verdict is fake and every cell holds its original content afterwards

#### Scenario: Cancel in the middle
- **WHEN** the user cancels during the coarse round
- **THEN** the run ends as cancelled and every cell holds its original content

#### Scenario: A write-back fails
- **WHEN** the drive refuses the write-back of one saved block
- **THEN** the result reports one restore failure

### Requirement: Data loss and I/O errors are counted apart
The probe SHALL count failed points by kind: `mismatches` (the point was read back, but the data was different) and
`ioErrors` (a write or a read of the point returned an error, or its old content could not be read). Both counts,
the number of unsaved blocks and the number of restore failures SHALL be part of the result.

#### Scenario: Fake that loses writes
- **WHEN** a drive keeps 31 GB and silently loses every write above it
- **THEN** the verdict is fake, `mismatches` > 0 and `ioErrors` = 0

#### Scenario: Fake whose reads fail past its real size
- **WHEN** a drive keeps 16 GB and every read above it returns a media error, while the start keeps answering
- **THEN** the verdict is fake with the real size near 16 GB, and `ioErrors` > 0

### Requirement: A drive that stops answering gets no verdict
The probe SHALL end as "the drive stopped answering", with no verdict, when the first coarse point fails by an I/O
error, when a block of the first 16 MiB cannot be read before it would be written, when 64 operations in a row fail
with I/O errors and the start no longer reads, or when re-reading the start after a round fails with an I/O error.

#### Scenario: Every read and write fails
- **WHEN** every read and every write of the device returns a media error
- **THEN** the probe ends as "stopped answering", not with "keeps no data", and nothing was written

#### Scenario: The drive stops answering in the middle of the coarse round
- **WHEN** a genuine drive returns a media error for every operation once the probe has written half of the coarse
  round's blocks
- **THEN** the probe ends as "stopped answering", with neither the fake nor the "keeps no data" verdict

#### Scenario: One transient error on the first point
- **WHEN** only the write of the first coarse point fails with a media error and every other point is good
- **THEN** the probe ends as "stopped answering", not with "keeps no data"

### Requirement: "Keeps no data" needs data that came back different
The verdict "keeps no data" (broken) SHALL be given only when the first coarse point was read back and its data was
different, and the start of the drive still answers.

#### Scenario: A drive that keeps nothing
- **WHEN** every write to a drive is lost and reads return zeros
- **THEN** the verdict is "keeps no data", with `mismatches` > 0 and `ioErrors` = 0

### Requirement: The restore is checked by reading back
Before the first write the probe SHALL keep a digest of every 4 KiB block of the first 16 MiB, of the last 1 MiB and
of every block the coarse round will write. After the write-back and a cache flush it SHALL read those blocks again
and compare. The restore is confirmed only when every one of them reads back and matches.

#### Scenario: Genuine drive
- **WHEN** a genuine drive is probed
- **THEN** the restore is confirmed

#### Scenario: Aliased addresses do not raise a false alarm
- **WHEN** a drive that wraps addresses modulo 8 GiB is probed, so several addresses share one cell
- **THEN** the restore is confirmed

#### Scenario: The write-back of the partition table did not stick
- **WHEN** the drive reports success for writes but keeps the probe's pattern in block 0 after the write-back
- **THEN** the restore is not confirmed, one mismatched block is reported at offset 0, and it is flagged as the
  partition table area

#### Scenario: Re-read fails
- **WHEN** a block of the first 16 MiB cannot be read during the check
- **THEN** the restore is not confirmed and the block is reported as unreadable

#### Scenario: Blocks of the refine rounds
- **WHEN** refine rounds wrote blocks that were not in the coarse round
- **THEN** those blocks are put back in reverse order but not re-read, and the result reports how many were not
  re-read

### Requirement: The drive is asked to commit its cache
After the write-back and before the read-back check, the probe SHALL ask the drive to flush its write cache
(`DKIOCSYNCHRONIZE` on macOS, `FlushFileBuffers` on Windows). A drive or system that does not support it SHALL be
recorded in the result and SHALL NOT fail the check. The texts SHALL NOT claim that a matching read-back proves the
data is stored on the flash.

#### Scenario: Flush not supported
- **WHEN** the cache flush returns "not supported"
- **THEN** the check goes on, and the result and the report say that the flush was not supported

### Requirement: The partition layout is compared after the check
After closing the device and remounting what it unmounted, the elevated helper SHALL look the drive up again for up
to 10 s and compare its partition layout with the one the job named (the one found at the start when the job names
none). It SHALL report: same, changed, partition table gone, drive gone, or could not be read.

#### Scenario: Layout unchanged
- **WHEN** the drive lists with the same partitions after the probe
- **THEN** the layout check says "same"

#### Scenario: The partition table vanished
- **WHEN** the drive lists with no partitions after the probe, as in the incident where macOS then showed `disk6` as
  Uninitialized
- **THEN** the layout check says "partition table gone"

#### Scenario: The scan does not answer
- **WHEN** looking the drive up fails or times out for 10 s
- **THEN** the layout check says "could not be read"

### Requirement: An untrusted result asks for a replug first
The window SHALL tell the user to unplug the drive, plug it in again and only then check it again, and not to format,
initialize or repair it first (also not when the system offers to), when the restore is not confirmed, a write-back
failed, the layout check is not "same", or the run ended as "stopped answering".

#### Scenario: The incident, replayed
- **WHEN** every operation of the coarse round fails with an I/O error, and afterwards the drive lists with no
  partitions
- **THEN** no verdict is shown and the record is unchanged. The Real capacity tab shows the replug notice with the
  counts, and the text never says "keeps no data" or "cannot be repaired"

#### Scenario: Verdict with an unconfirmed restore
- **WHEN** the probe found a fake but one block at the start read back different after the write-back
- **THEN** the verdict is not recorded, and the replug notice says that the check result cannot be trusted until the
  drive is plugged in again and checked again

#### Scenario: Confirmed restore
- **WHEN** the restore is confirmed, no write-back failed and the layout is the same
- **THEN** the verdict is recorded and shown as before, with a note that the restored data read back unchanged

### Requirement: No task runs on a drive that waits for a replug
While the replug notice is pending for a drive, the application SHALL refuse to start a speed test, a check or a
repair on it with `errReplugFirst`, and the window SHALL disable those actions. The notice SHALL end when the drive
has left a successful drive scan, or the program is started again.

#### Scenario: Repair asked before the replug
- **WHEN** the replug notice is pending for `disk6` and the window sends a repair or a check for it
- **THEN** the request ends with `errReplugFirst` and no administrator prompt appears

#### Scenario: After the replug
- **WHEN** `disk6` disappears from a successful scan and comes back (under any `diskN`)
- **THEN** the notice is gone and every action is available again

#### Scenario: A failed scan does not end the notice
- **WHEN** a scan fails (for example `diskutil` does not answer) while the notice is pending
- **THEN** the notice stays

### Requirement: Access errors are told apart
An access error on the raw device (open, read or write) SHALL end the run with no verdict and the record unchanged,
with a cause: macOS `EPERM` is the privacy protection (Full Disk Access); `EBUSY` and Windows 32 / 33 are a busy
drive; `EROFS`, `EACCES` in the elevated helper and Windows 19 are a write-protected drive; `ENXIO`, `ENODEV` and Windows 21 / 433 /
1167 are a drive that is gone; anything else (Windows 5) is a refused access.

#### Scenario: macOS refuses to open the device
- **WHEN** opening `/dev/rdiskN` fails with `EPERM` on macOS
- **THEN** the run ends with the Full Disk Access message

#### Scenario: macOS refuses the first read
- **WHEN** the device opens, but the first read fails with `EPERM` on macOS
- **THEN** the run ends with the Full Disk Access message, and nothing was written

#### Scenario: Busy volume
- **WHEN** the device open fails with `EBUSY`
- **THEN** the message says that the drive is in use, and never mentions Full Disk Access

#### Scenario: Write-protect switch
- **WHEN** a write in the elevated helper fails with `EROFS` or `EACCES` on macOS, or with error 19 on Windows
- **THEN** the message says that the drive is write-protected, and never mentions Full Disk Access

#### Scenario: Unplugged during the check
- **WHEN** a read fails with `ENXIO` or `ENODEV`, or with error 1167 on Windows
- **THEN** the message is the "drive gone" error

#### Scenario: Windows never shows Full Disk Access
- **WHEN** any access error happens on Windows, error 5 included
- **THEN** the message is not the Full Disk Access message

### Requirement: The Full Disk Access message says what to do
On macOS the Full Disk Access message SHALL say that macOS blocks direct access to drives for apps without Full Disk
Access, where to turn it on (System Settings → Privacy & Security → Full Disk Access), that the app must be quit and
opened again, and that after an app update the entry may have to be removed and added again.

#### Scenario: The message after a blocked check
- **WHEN** a quick check ends with the Full Disk Access cause
- **THEN** the Real capacity tab shows the message with the steps and a button that opens the Full Disk Access pane,
  in every one of the 5 languages, until the next check of that drive starts

### Requirement: The settings button opens only the privacy pane
The button SHALL ask the application to open the Full Disk Access pane. The application SHALL open only that fixed
pane, never a link that the window sends, and SHALL fall back to opening System Settings when the pane cannot be
opened. The button SHALL exist only on macOS.

#### Scenario: Pressing the button
- **WHEN** the user presses "Open Full Disk Access settings"
- **THEN** System Settings opens at Privacy & Security → Full Disk Access

### Requirement: The result shows what failed
The quick-check verdict SHALL show, when they are not 0: points read back different, points with read or write
errors, blocks not written because they could not be read, and blocks that could not be written back. A confirmed
restore SHALL be shown as a short note.

#### Scenario: Fake with errors past the real size
- **WHEN** a fake verdict has 1 700 points with I/O errors and 50 that came back different
- **THEN** the verdict panel shows both numbers next to "Checked in …"

#### Scenario: Record from an older version
- **WHEN** a remembered check has none of the new counts
- **THEN** the verdict shows as before, without the counts
