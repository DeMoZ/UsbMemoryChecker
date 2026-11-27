# Spec Delta

## Purpose

Decides what size the repair gives a drive and stops a repair from making a fake drive unsafe again: the default size
comes from what is known about the drive, the full declared size is never a default except for a drive a whole-device
check found genuine and otherwise needs an explicit choice, and both the window and the elevated helper refuse sizes
that make no sense.

## ADDED Requirements

### Requirement: Default repair size follows the drive's state
The Repair tab's default new size SHALL be the repair size of the recorded loss (fake, damaged, repaired), the
current limit rounded down to a whole MiB (partition verified, limited), or the full declared size only for a genuine
drive. A computed size of 0, or one not below the declared size, SHALL give no plan. With no plan (keeps nothing,
unconfirmed, not checked) the repair button SHALL stay disabled and the tab SHALL ask for the quick check.


#### Scenario: Repair tab right after the repair from the bug report
- **WHEN** the 125.8 GB fake has just been repaired to 8.3 GB
- **THEN** NEW SIZE shows 8.3 GB with "instead of the declared 126 GB", and the text does not say "No capacity problem
  known" or "formatted at its full size"

#### Scenario: Repeated repair does not shrink the drive
- **WHEN** a repaired drive is repaired again with the default size
- **THEN** the new partition has the same size as the current one

#### Scenario: Limited drive without a record
- **WHEN** the state is limited with a 8.3 GB partition and no check is known
- **THEN** the default new size is 8.3 GB (rounded down to whole MiB), and a note says that no check is known, the
  size of the current partition is kept, and a capacity check gives the real size

#### Scenario: Unchecked drive with a full-size partition
- **WHEN** the state is not checked and the layout is not limited
- **THEN** there is no default size: the tab says to run the quick check first or to choose the full declared size
  oneself, and the "use the full declared size" option is offered with neutral wording (no fake-drive warning)

#### Scenario: Genuine drive
- **WHEN** a quick check, or a full check of a nearly empty volume (at most max(64 MiB, 2%) in use before), found the
  whole device genuine
- **THEN** the default is the full declared size

#### Scenario: Limited drive with a partition smaller than 1 MiB
- **WHEN** the state is limited and the largest partition is below 1 MiB
- **THEN** there is no plan (the size rounds to 0), the repair is refused with the "no plan" error, and the full size
  is available only by the explicit option

### Requirement: Full declared size only by explicit choice
Unless the default is the full declared size, the Repair tab SHALL offer an option, off by default, to use the full
declared size instead. With a known limit it SHALL warn that data past the known real size is lost; for a drive with no
plan the wording SHALL be neutral. The option SHALL reset to off after a repair, when the drive's state changes and
when another drive is selected. The confirmation dialog SHALL name the chosen size.


#### Scenario: The explicit tick was made in another state
- **WHEN** the full-size plan is sent with the tick, but the state or limit the window showed then is not the
  application's state and limit now
- **THEN** the request is refused with the "state changed" error

#### Scenario: Unconfirmed knowledge
- **WHEN** the knowledge of a drive is unconfirmed by its current layout
- **THEN** every repair plan is refused with the "unconfirmed" error and the Repair tab asks for a quick check

#### Scenario: Stale window asks for the full size
- **WHEN** the window still shows an older state and sends the full-size plan without the explicit tick, but the
  application's plan for the drive is a limit
- **THEN** the request is refused with the "state changed" error and nothing runs

#### Scenario: Default stays limited
- **WHEN** the user opens the Repair tab of a repaired drive, ticks only the confirmation box and presses Repair
- **THEN** the dialog names 8.3 GB and the repair uses 8.3 GB

#### Scenario: User chooses the full size
- **WHEN** the user turns on "use the full declared size" and confirms
- **THEN** the dialog names 126 GB, the repair formats the whole drive, and the drive shows as fake again afterwards

#### Scenario: Not offered when the full size is the default
- **WHEN** the default already is the full declared size, or the state is "keeps nothing" or "unconfirmed"
- **THEN** the option is not shown

### Requirement: The application checks the repair plan before running it
A repair request from the window SHALL name its plan (limited or full), the size it showed and whether the full size
was chosen explicitly. The full-size plan SHALL be accepted only as the application's default plan or when chosen
explicitly. The
application SHALL work out that plan's size from its own record and the current layout, and SHALL refuse the request
when the sizes differ or the plan is not possible. In that case nothing runs on the drive and the window says the
drive's state changed.

#### Scenario: State changed under the window
- **WHEN** the window sends a limited plan of 8.3 GB but the application now computes a different size for the drive
- **THEN** the repair does not start, no administrator prompt appears, and the window shows the "state changed, look
  again" error

#### Scenario: Limited plan without a limit
- **WHEN** a limited plan is requested for a drive whose state has no limited size (genuine or not checked)
- **THEN** the request is refused the same way

#### Scenario: Command line repair
- **WHEN** `usbmemorychecker --cli fix <disk> <real bytes|0>` is used
- **THEN** a missing or non-numeric size is refused; an explicit 0 (or a size not below the drive's) is a request for the
  whole drive and sets `whole_confirmed`; any other size is the real size, without that flag. `--cli probe` needs no size

### Requirement: A drive that keeps nothing is not repaired
When a drive's recorded loss gives a repair size of 0, the application SHALL refuse any repair request for it with
`errBroken` before starting the helper, whichever plan the request names.

#### Scenario: Full check found nothing usable
- **WHEN** a full check ended with a loss whose repair size is 0 and a repair is requested
- **THEN** the request ends with `errBroken` and the helper is not started

#### Scenario: Full size requested for a drive that keeps nothing
- **WHEN** the full-size plan is requested for such a drive
- **THEN** the request ends with `errBroken` as well

### Requirement: The helper checks the drive's layout
The repair and quick-check jobs SHALL carry the partition layout the window saw, and the elevated helper SHALL
refuse the job before it touches the drive when the drive it finds under that id has another layout.

#### Scenario: Drive swapped while the password window is open
- **WHEN** another drive of the same size takes the `diskN` before the helper starts
- **THEN** the helper fails with an error and runs neither `diskutil` nor `diskpart`

### Requirement: The helper refuses a full-size format that nobody confirmed
A repair job SHALL carry a `whole_confirmed` flag that only the application sets: when the plan is the full size by
default for a genuine drive, or by the user's explicit option (and `--cli fix` with an explicit size of 0 or not below the
drive's). The elevated
helper SHALL refuse an `eraseDisk` or full-size `diskpart` format without it, and SHALL refuse any repair when the
partition table of the drive could not be read.

#### Scenario: Job without the confirmation
- **WHEN** a job asks for the whole drive (no exact partition, real size 0) without `whole_confirmed`
- **THEN** the helper fails with an error and runs neither `diskutil` nor `diskpart`

### Requirement: The helper refuses an impossible partition size
The elevated helper SHALL refuse a repair job before it unmounts, cleans or partitions anything when the partition
size it would create is 0, not a whole number of MiB (except the full declared size), or larger than the declared
size. FAT32 SHALL still be refused above 32 GB.

#### Scenario: Partition size 0
- **WHEN** a repair job asks for a real size below 3 MiB (partition size 0)
- **THEN** the helper fails with an error and runs neither `diskutil` nor `diskpart`

#### Scenario: Exact partition size
- **WHEN** a job names an exact partition size of 8 303 673 344 bytes on a 125 829 120 000-byte drive
- **THEN** macOS runs `diskutil partitionDisk <disk> 2 MBR <fs> <label> 8303673344 FREE gap R` and Windows creates a
  primary partition of 7919 MiB

#### Scenario: Whole drive
- **WHEN** a job asks for the whole drive (real 0 and no exact size)
- **THEN** macOS runs `diskutil eraseDisk` and Windows creates one partition over the whole disk, as before
