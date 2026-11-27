# Spec Delta

## Purpose

The speed test measures how fast a drive writes and reads a test file of a chosen size past the system cache, and
checks that every block of it comes back intact.

## ADDED Requirements

### Requirement: Exact test size
The speed test SHALL write and read exactly the requested number of bytes, rounded up only to a multiple of 4 KiB,
independent of the internal transfer size. It SHALL fail with "no space" when the free space is below the size plus
a 32 MiB reserve.

#### Scenario: Size that is not a multiple of the transfer size
- **WHEN** the speed test runs with 40 MiB or 100 MiB while transfers are 4 MiB
- **THEN** the result reports `bytes` equal to 40 MiB or 100 MiB, not a size rounded up to 48 MiB or 112 MiB

#### Scenario: Not enough space
- **WHEN** the free space is smaller than the size plus 32 MiB
- **THEN** the test fails with the "no space" error and writes nothing

### Requirement: Honest timing
Write and read speeds SHALL count only the time spent in the drive's read/write calls, plus the final flush to the
drive for the write average. Time spent waiting for test data to be prepared or verified SHALL NOT count.

#### Scenario: Average includes the final flush
- **WHEN** the write phase ends
- **THEN** the time of the final flush is part of `write.seconds` and of the write average

#### Scenario: Slow data preparation does not lower the speed
- **WHEN** preparing test data is slower than the drive
- **THEN** the reported speeds still reflect only the read/write calls

### Requirement: Speed windows of 16 MiB
Maximum and minimum speeds SHALL be computed over windows of 16 MiB of transferred data. A last window shorter than
8 MiB SHALL count only in the average. One chart point SHALL be sent per completed window.

#### Scenario: Points per window
- **WHEN** 40 MiB are written
- **THEN** two write points are sent (at 16 MiB and 32 MiB, each after four transfers), and the last 8 MiB close a third window for max/min
  only if they are at least half a window

### Requirement: Every block is verified
Every 4 KiB block read back SHALL be compared with what was written; `badBlocks` SHALL be the number of blocks that
differ.

#### Scenario: Intact drive
- **WHEN** the drive keeps all data
- **THEN** `badBlocks` = 0

### Requirement: Test data streams without idle gaps
The speed test SHALL prepare the next buffer's test data and verify the previous buffer while the current read or
write call is in progress, using transfers of 4 MiB (a speed window is four transfers). The written data and `badBlocks` SHALL be the same as with
serial processing.

#### Scenario: Same data as serial processing
- **WHEN** the pipelined writer produces the test file
- **THEN** each block at offset p equals the pattern block for p and the run's salt

### Requirement: Cancel and cleanup
The speed test SHALL check for cancel before every transfer and SHALL remove its test file when it ends in any way.

#### Scenario: Cancel
- **WHEN** cancel is requested during the test
- **THEN** the test ends with "cancelled" and the test file no longer exists

#### Scenario: Normal end
- **WHEN** the test completes
- **THEN** the test file no longer exists
