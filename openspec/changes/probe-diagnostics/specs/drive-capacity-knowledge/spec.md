# Spec Delta

## MODIFIED Requirements

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
