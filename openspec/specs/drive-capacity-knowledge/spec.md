# drive-capacity-knowledge Specification

## Purpose
Keeps one consistent record of what the app knows about each drive's real capacity: checks, repairs and the
partition layout. The record survives a restart and a replug, and the window shows a state worked out from it, so a
repaired fake drive is never shown as unchecked or genuine.

## Requirements

### Requirement: Knowledge record per drive
For every drive, the application SHALL keep: the declared size, the last check (quick or full, scope whole device or
partition, verdict, real size, details, date), the measured data loss (real size and verdict fake or damaged, or
none) and the last repair (partition size and date). The details of a quick check SHALL include the number of points
read back different and the number of points with read or write errors. The record SHALL change only because of a
check or a repair of that drive.

#### Scenario: Quick check finds a fake
- **WHEN** a quick check of a 125.8 GB drive ends with verdict fake and real 8.3 GB
- **THEN** the record holds a loss of 8.3 GB (fake) and the last check (quick, whole device, fake, 8.3 GB, the date)

#### Scenario: A whole-device check that passes clears the loss
- **WHEN** a drive with a recorded loss passes a quick check, or passes a full check of a volume that covers the
  declared size
- **THEN** the loss is cleared and the last check is that genuine result

#### Scenario: Full check finds data loss
- **WHEN** a full check ends with damaged data and a real-capacity estimate E on a drive of declared size S
- **THEN** the loss is `min(E, S) / 100 × 99` (as before) with verdict fake, or the smaller of that and an already
  recorded loss when only a partition was checked

#### Scenario: Repair keeps what is known
- **WHEN** a repair to a 8.3 GB partition succeeds
- **THEN** the loss and the last check stay, and the repair (8.3 GB partition, date) is added

#### Scenario: Repair at the full declared size
- **WHEN** the user explicitly repairs a drive with a recorded loss at its full declared size
- **THEN** the repair entry is removed and the loss stays, so the drive shows as fake again

#### Scenario: Speed test changes nothing
- **WHEN** a speed test runs or a check fails, is cancelled or is declined
- **THEN** the record is unchanged

#### Scenario: A quick check that cannot be trusted changes nothing
- **WHEN** a quick check of a repaired drive ends as "the drive stopped answering", or with a restore that is not
  confirmed, a failed write-back or a layout that is not the same afterwards
- **THEN** the record is unchanged (loss, last check and repair stay), so the state and the repair plan are what they
  were before the check

#### Scenario: Counts of a quick check
- **WHEN** a quick check ends with verdict fake, 1 700 points with I/O errors and 50 points read back different
- **THEN** the last check holds both counts, they survive a restart for a repaired drive, and a record from an older
  version without them reads as 0 for both

### Requirement: Partition-limited layout
A drive's layout SHALL count as limited when its partition table has at least one partition and the space from the
end of the last partition to the declared size is at least the limit margin: the larger of 64 MiB and 10% of the
declared size. The limit of such a drive SHALL be the size of its largest partition.

#### Scenario: The repaired fake from the bug report
- **WHEN** a 125.8 GB drive has one partition of 8.3 GB at offset 1 MiB and 117.5 GB unallocated after it
- **THEN** the layout is limited with a limit of 8.3 GB

#### Scenario: Normal full-size partition
- **WHEN** the partitions end less than the limit margin before the declared size
- **THEN** the layout is not limited

#### Scenario: No partition table
- **WHEN** the drive has no partitions (blank, or a volume directly on the whole device)
- **THEN** the layout is not limited

#### Scenario: Small card
- **WHEN** a 1 GB card has one partition of 300 MB at its start
- **THEN** the layout is limited (700 MB unallocated is more than 100 MB)

### Requirement: Displayed capacity state
The application SHALL derive one state per drive from its record and its current layout. When the layout does not
confirm the knowledge the state is unconfirmed; otherwise, in this order: keeps nothing; repaired; fake or damaged;
genuine; partition verified; limited; not checked. Unconfirmed means the knowledge belongs to another layout than the
drive has now. Repaired means a loss, partitions
ending no more than 1 MiB past it, and a limited layout or a recorded repair.

#### Scenario: Right after a repair
- **WHEN** a fake drive with a loss of 8.3 GB was repaired and now has one partition ending at or below 8.3 GB with
  the rest unallocated
- **THEN** the state is repaired, not "not checked"

#### Scenario: Repaired, then verified
- **WHEN** a repaired drive then passes the full check of its partition
- **THEN** the state is repaired and marked verified, and never genuine at the declared size

#### Scenario: Repaired fake that keeps almost its whole declared size
- **WHEN** a quick check finds real 120 GB on a 125.8 GB drive, the repair gives a partition of 118 GB, and the
  full check of that volume passes
- **THEN** the state is repaired and verified (never genuine), the default repair size stays the partition size, and
  the full declared size is used only by an explicit choice

#### Scenario: Drive reformatted or swapped while connected
- **WHEN** the layout of a connected drive changes by something other than our own repair (reformatted by another
  tool, or another drive got the same `diskN` between two scans)
- **THEN** the stored record of the old layout is left as it is. The state is what a record made by our repair
  holds for the new layout; without one the in-memory knowledge is shown as unconfirmed (no plan) until a new check

#### Scenario: Partition table could not be read
- **WHEN** the partition table of a drive could not be read (a partition's `diskutil info` failed, or `Get-Partition`
  failed), with or without an in-memory record
- **THEN** the state is unconfirmed (no plan, the repair refused) and a stored repaired fake is not shown as "not
  checked". A blank disk (no partitions, nothing failed to read; on Windows "not found" from `Get-Partition` is not a
  failure) is not unreadable: it is not checked, with no plan and the explicit option. A check made while the table is
  unreadable does not change the state; the knowledge shows again as soon as the table reads fine

#### Scenario: Limited layout without a record
- **WHEN** a drive with a limited layout has no record
- **THEN** the state is limited, not "not checked"

#### Scenario: A loss whose repair size is 0
- **WHEN** the recorded loss is below 3 MiB (including 0), so the repair size is 0
- **THEN** the state is "keeps nothing", whatever the layout

#### Scenario: Quick check says the drive keeps nothing
- **WHEN** a quick check ends with verdict broken and real 0
- **THEN** the state is "keeps nothing" and the repair size is not the full declared size

#### Scenario: Genuine drive with a small partition
- **WHEN** the last check is a passing quick check and the layout is limited
- **THEN** the state is genuine

### Requirement: Header real-capacity box
The Real capacity box in the drive header SHALL show the state: not checked, genuine (the declared size), fake or
damaged (the loss real size), keeps nothing (0), repaired or repaired and verified (the partition size), partition
verified, limited (the partition size with a "not checked" hint), or unconfirmed (a hint to check again).

#### Scenario: Header right after the repair from the bug report
- **WHEN** the 125.8 GB fake has just been repaired to 8.3 GB
- **THEN** the box shows 8.3 GB with "Repaired", not "Not checked / Run the capacity check"

#### Scenario: Header for a detected limited drive
- **WHEN** the state is limited with an 8.3 GB partition on a 125.8 GB drive
- **THEN** the box shows 8.3 GB in the warning colour with a hint that only this partition is used and it is not
  checked

#### Scenario: Capacity tab badge
- **WHEN** the state is fake, damaged, keeps nothing, limited or unconfirmed
- **THEN** the Real capacity tab shows its alert badge; for not checked, genuine, repaired and partition verified it
  does not

### Requirement: Real capacity tab shows the remembered result
The Real capacity tab SHALL show the verdict of the last check from the record, including after a restart, with the
date of the check when it was not made in this run. When the layout is limited, the tab SHALL say what the largest
partition is and how much space after the last partition is unused, that the full check tests only the volume and
the quick check tests the whole device (the text is right also for several partitions).

#### Scenario: After a restart
- **WHEN** the app restarts and a remembered drive is connected
- **THEN** the tab shows the last verdict with "checked on <date>"

#### Scenario: Partition verdict
- **WHEN** the last check is a passing full check of a partition of 8.3 GB on a 125.8 GB drive
- **THEN** the verdict title says the partition is real and the text says that only the 8.3 GB partition was tested
  out of 126 GB declared

#### Scenario: Limited layout note
- **WHEN** the layout is limited
- **THEN** the note with the partition size and the unused size is shown above the verdict

### Requirement: Knowledge survives restarts and replugs
The application SHALL save the record of a repaired drive in `capacity.json` after every change. A newly connected
drive SHALL be matched only to a record whose key was made by our own repair, when the declared size and every
partition (offset, size, volume UUID or disk signature or GUID) are equal. A layout without an id SHALL never match.
What a check found on a drive that was not repaired by us SHALL stay in memory while the drive stays connected.

#### Scenario: Restart
- **WHEN** a drive was repaired, the app is restarted, and the drive is still connected
- **THEN** the drive shows as repaired with the remembered loss and check

#### Scenario: Replug under another disk number
- **WHEN** the drive is unplugged and plugged in again and becomes `disk7` instead of `disk6`
- **THEN** its record is found by size and layout, and the state is the same as before

#### Scenario: A checked drive that was not repaired
- **WHEN** a drive was checked (quick or full) but not repaired by us, then unplugged and plugged in again, or the
  app is restarted
- **THEN** its result is not shown any more, and it has to be checked again

#### Scenario: Two instances of the app
- **WHEN** another running instance wrote `capacity.json` after this instance read it
- **THEN** saving merges by key: the record whose content was written last (`changed`, not `seen`) wins, and a key
  removed by either instance after the record's last change (a recorded removal) does not come back

#### Scenario: Reformatted elsewhere
- **WHEN** the drive was reformatted on another computer, so its layout or volume UUID changed
- **THEN** no record matches, and the state comes from the layout alone

#### Scenario: Broken or missing file
- **WHEN** `capacity.json` is missing, unreadable or not valid JSON
- **THEN** the app starts with no records, works normally, and writes a valid file at the next change

#### Scenario: Old records
- **WHEN** there are more than 100 records
- **THEN** the records not seen for the longest time are dropped

### Requirement: A record moves to a new layout only after our own repair
A record SHALL move to a new layout only by the repair the application itself ran on that drive, and only when the
new layout can name the drive and differs from the old one. Any other layout change of a connected drive, or a layout
that cannot be read, SHALL leave the stored record untouched and show the in-memory knowledge as unconfirmed. During a
running task the layout of the task's own drive belongs to that task; every other drive is kept in step as usual.

#### Scenario: Drive unplugged during another drive's task loses its knowledge
- **WHEN** a long check runs on one drive and another drive with in-memory knowledge is unplugged
- **THEN** a scan during the task drops that knowledge, so a drive that takes its `diskN` does not inherit it; the
  task's own drive is left alone

#### Scenario: Key updated after a repair
- **WHEN** our repair creates a new partition with a new volume UUID
- **THEN** the record moves to the new layout in the same step (no duplicate), the list sent at the end of the task
  already shows the repaired drive, and after a restart the drive is still recognised

#### Scenario: The new layout cannot be used as a key
- **WHEN** our repair finished but the listing failed, the new layout has no id, or it still equals the layout from
  before the repair
- **THEN** nothing is moved or stored under any key and the stored record (if any) stays as it is; the knowledge
  stays in memory only, shown as unconfirmed (no plan), until a new check replaces it. The layout-based "limited"
  detection (the repaired partition is much smaller than the drive) keeps the proposed size at the partition size
  after a restart. Known limit: a repaired fake that kept almost its whole declared size (an unused tail below the
  limit margin) has no such detection, so with no stored record (the listing failed, or no volume UUID) it shows as
  "not checked" with no plan after a restart (the full size only by the explicit option), until a check is made

#### Scenario: Full-size repair of a drive that knows nothing
- **WHEN** a drive with no known result is repaired at the full declared size
- **THEN** nothing is stored

#### Scenario: Entry without a key
- **WHEN** a check was made while the layout could not identify the drive, and a drive with a usable layout then
  appears under that `diskN`
- **THEN** the check result is shown as unconfirmed for it. (Known limitation: two drives without any layout and of
  one size swapped between two scans cannot be told apart.)

#### Scenario: A scan overlapped a task
- **WHEN** a task started or ended while a drive scan was running
- **THEN** that scan's result is not applied, and the next scan is

#### Scenario: Swap between two scans, the other drive has no usable layout
- **WHEN** drive A (repaired, known as `disk6`) is replaced by drive B of the same size that has no partition table or
  whose `diskutil info` failed, before the next scan, or before the first scan after a task
- **THEN** the knowledge is kept but shown as unconfirmed (no limit, no full-size default, no plan from it), the
  repair is refused with the "unconfirmed" error, the window asks for a quick check, and a new check replaces it

#### Scenario: Known fake whose layout changes by something other than our repair
- **WHEN** a connected drive with a recorded loss gets another usable layout (reformatted by another tool)
- **THEN** the knowledge is shown as unconfirmed (no plan) until a new check; a stored record of the old layout is
  left as it is, and a record of our repair that matches the new layout is used instead

#### Scenario: Swap between two scans
- **WHEN** drive A (known as `disk6`) is unplugged and drive B of the same size, with another layout, gets `disk6`
  before the next scan
- **THEN** B does not inherit A's knowledge, A's record is intact and A is recognised when it is plugged in again

#### Scenario: Record about another size
- **WHEN** a stored record's knowledge names another declared size than its key
- **THEN** it is never matched, and scans do not rewrite `capacity.json` because of it

#### Scenario: Older record when a task ends
- **WHEN** a check finishes and the drive has no in-memory entry but the store has a record for its layout
- **THEN** the result is added to that record, not to an empty one

#### Scenario: Unreadable record
- **WHEN** one record of `capacity.json` cannot be read
- **THEN** the other records load, and the unreadable one is written back unchanged at the next save

#### Scenario: Incomplete partition table
- **WHEN** `diskutil info` fails or times out for one partition of a drive
- **THEN** the drive is listed without a partition table (no limited layout, no record matched or saved; its
  knowledge is unconfirmed), and the other drives are listed as usual

#### Scenario: Drive gone
- **WHEN** a drive disappears from the list or its declared size changes
- **THEN** the app stops showing its record for that disk id, and the saved record stays for a later replug
