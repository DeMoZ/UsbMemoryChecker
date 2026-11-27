# Spec Delta

## Purpose

Finds the removable drives and tells the window what is on them: the volumes, the partition table, and the capacity
state of each drive. The list stays current by itself, and the Reload button always gets an answer.

## ADDED Requirements

### Requirement: Drive list includes the partition table
Every listed drive SHALL include its partitions, each with its offset and size in bytes, also partitions that are
not mounted. On macOS each partition SHALL carry its volume UUID when it has one. On Windows the drive SHALL carry its
MBR disk signature or GPT disk GUID.

#### Scenario: Repaired drive on macOS
- **WHEN** `diskutil` describes `disk6` (125 829 120 000 bytes, FDisk scheme) with one partition `disk6s1` of
  8 303 673 344 bytes at offset 1 048 576 and volume UUID U
- **THEN** the drive has one partition {offset 1 048 576, size 8 303 673 344, id U}

#### Scenario: Unmounted partition
- **WHEN** a drive has a partition that is not mounted
- **THEN** the partition is still listed with its offset and size

#### Scenario: Windows disk
- **WHEN** PowerShell reports a USB disk with MBR signature S and one partition (offset 1 048 576, size 8 303 673 344)
  without a drive letter
- **THEN** the drive has table id S and that partition, and no volume (as before for partitions without a letter)

### Requirement: Reload always gets an answer
Every Reload request SHALL be answered with a drive list from a scan that started after the request, also when
nothing changed. Until the answer comes, the Reload button SHALL show that it is working and SHALL not start
another request. After 20 s without an answer the window SHALL stop waiting and say the system is slow.

#### Scenario: Nothing changed
- **WHEN** the user presses Reload and the drives are the same as before
- **THEN** the window gets the list within one scan, the button returns to normal, and the shown state is unchanged

#### Scenario: Slow scan
- **WHEN** a scan is still running 20 s after Reload was pressed
- **THEN** the button returns to normal and the window shows a message that the system takes long to answer and the
  list will update by itself

#### Scenario: Several presses
- **WHEN** Reload is pressed again while the first request has no answer yet
- **THEN** no second request is sent

### Requirement: System tools have a time limit
Every call of `diskutil` (macOS) or PowerShell (Windows) made by the drive scan SHALL be stopped after a time limit
(10 s for `diskutil`, 30 s for PowerShell). A stopped list or whole-disk call SHALL fail the scan and the previous
list SHALL stay; a waiting Reload SHALL be answered with that list marked as not refreshed. A stopped `diskutil info`
of one partition SHALL NOT fail the scan.


#### Scenario: diskutil does not answer
- **WHEN** the `diskutil list` call or the `diskutil info` call of a whole disk does not finish within 10 s
- **THEN** that call is stopped, the scan ends with an error, the window keeps the previous list, and the next scan
  runs as usual (a stopped call of one partition does not fail the scan, see "One partition does not answer")

#### Scenario: One partition does not answer
- **WHEN** `diskutil info` of one partition of a drive times out
- **THEN** that drive is listed without partitions and flagged as having an unreadable table (its knowledge
  unconfirmed, no plan, the repair refused), and the other drives are listed as usual

#### Scenario: Reload during a stuck scan
- **WHEN** Reload is pressed and the scan then fails on a time limit
- **THEN** the window shows the "could not refresh the drive list" message and keeps showing the previous list

### Requirement: Drive list carries the capacity state
Every drive list sent to the window SHALL include, for each drive, its capacity state, real-capacity value and hint
inputs, last check, and repair plan as the application works them out. The application SHALL send a new list right
after every task ends, so the window never shows a state worked out from an older record.

#### Scenario: Knowledge changed without a change of the list
- **WHEN** a scan finds the same drives but the knowledge attached to them changed (a record was loaded after a
  task ended)
- **THEN** a new list is sent, so the window never keeps offering a plan from an older state

#### Scenario: After a check
- **WHEN** a quick or full check ends
- **THEN** the task result and a drive list with the new state reach the window, and the header shows the new state
  without waiting for the next scan

#### Scenario: After a repair
- **WHEN** a repair ends successfully
- **THEN** the list sent at its end already holds the drive's new layout and shows it as repaired with the default
  repair size of its partition
