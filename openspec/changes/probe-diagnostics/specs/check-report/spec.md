# Spec Delta

## Purpose

Every capacity check and repair leaves a small diagnostic report on the computer, so that a failed or strange
outcome can be understood afterwards, without exposing the user's files, the drive's data or names that identify the
user.

## ADDED Requirements

### Requirement: Every check and repair leaves a report
When a quick check, a full check or a repair ends, the application SHALL write one report file for that run, whatever
the outcome: a verdict, an error, "stopped answering", a cancel, or a declined password / UAC prompt. A failure to
write the report SHALL NOT change the outcome; it SHALL be written to stderr.

#### Scenario: Quick check blocked by privacy protection
- **WHEN** a quick check ends with the Full Disk Access cause
- **THEN** a report exists for it with the cause "privacy", the operation (open or read), the OS error code and the
  stage

#### Scenario: Declined prompt
- **WHEN** the user closes the password window
- **THEN** a report exists that says the run was declined, and nothing about the drive beyond what was chosen

#### Scenario: The reports folder cannot be written
- **WHEN** the reports folder cannot be created
- **THEN** the check result is shown as usual, and an error line goes to stderr

### Requirement: Reports live in the user's logs folder
Reports SHALL be written to `~/Library/Logs/UsbMemoryChecker/checks/` on macOS and to
`%LOCALAPPDATA%\UsbMemoryChecker\logs\checks\` on Windows (the home folder comes from `HOME` / `USERPROFILE`). They
SHALL be named `<UTC date>-<UTC time>-<kind>-<diskN>.json`, be written by the window process (never by the elevated
helper) through a temporary file and a rename, and on macOS be readable by the owner only.

#### Scenario: Manual run with a scratch home
- **WHEN** the app runs with `HOME=<scratch>` on macOS
- **THEN** the report is written under `<scratch>/Library/Logs/UsbMemoryChecker/checks/`, and nothing is written
  under the real home

#### Scenario: Ownership after an elevated check
- **WHEN** a quick check ran in the elevated helper as root on macOS
- **THEN** the report file and the reports folder belong to the user, with modes `0600` and `0700`

### Requirement: What a report contains
A report SHALL hold: the format version, the app version, OS and architecture, start and end time (UTC), the kind,
the drive (id, bus, kind, model name, declared size, block size, partition offsets, sizes and file systems, whether
the table was unreadable), each stage with its time and result, and the outcome. Quick checks SHALL also hold the
counts, the first failures and the read-back and layout checks.

#### Scenario: Quick check that stopped answering
- **WHEN** a quick check ends as "stopped answering"
- **THEN** its report holds the stages (find, unmount, open, baseline, rounds, write-back, cache flush, read-back
  check, close, remount, layout check), the counts (`mismatches`, `ioErrors`, unsaved, restore failures), the first
  failures with offset, operation and OS error code, the layout before and after, and the outcome key

#### Scenario: Full check and repair
- **WHEN** a full check or a repair ends
- **THEN** its report holds the drive, the stages that ran, the result fields that the window shows (for a repair:
  the planned partition size, the file system and the system tool's exit status), and the outcome key

### Requirement: A report holds no private data
A report SHALL NOT contain file contents, data read from the drive or digests of it, volume labels, mount paths,
volume UUIDs, disk GUIDs or signatures, serial numbers, the user name or the home path. Error texts that name a
label, a mount path or the home folder SHALL have them replaced by `<volume>` or `<home>`. The replacement SHALL NOT reach into other words (a Windows mount `E:` leaves `FILE:` as it
is) or into a placeholder already put in.

#### Scenario: Error text with the volume path
- **WHEN** a full check of the volume `PHOTOS` at `/Volumes/PHOTOS` ends with an error whose text contains
  `/Volumes/PHOTOS/UsbMemoryChecker-test/00001.bin`
- **THEN** the report contains `<volume>/UsbMemoryChecker-test/00001.bin`, and neither `PHOTOS` nor the volume's UUID
  appears anywhere in the report

#### Scenario: Read-back check
- **WHEN** the read-back check of a quick check finds mismatched blocks
- **THEN** the report lists their offsets and counts only, and never their content or a digest of it

### Requirement: The elevated helper never writes through a link
The window SHALL create the job folder readable and writable by the owner only. The elevated helper SHALL refuse a
job folder that is a symbolic link, and SHALL keep using the folder it opened even if its path is replaced later. It
SHALL create each file it writes there (its event lines, the progress and
result files) as a new file that is not a link, and SHALL never open an existing file there for writing.

#### Scenario: A planted link
- **WHEN** `log.jsonl` in the job folder is a symbolic link to another file before the helper starts
- **THEN** the helper does not write to the target of the link, and the run ends with an error

#### Scenario: Job folder swapped for a link while the helper runs
- **WHEN** the job folder's path is replaced by a link to another folder after the helper has opened it
- **THEN** nothing in the other folder is created, removed or changed

#### Scenario: Progress file replaced by a link
- **WHEN** the temporary progress file name is a symbolic link while the helper runs
- **THEN** the helper does not write through it, and the next progress update uses a new file

### Requirement: The elevated helper stays inside the job folder
On every system the helper SHALL create, change and remove nothing outside the job folder it opened. It SHALL read the
job file only when that is a plain file with a single name (on macOS also owned by the folder's owner), and SHALL copy
nothing of a refused file into its result.

#### Scenario: A hard link as the job file
- **WHEN** `job.json` is a hard link to another file
- **THEN** the helper does not read it, the result says that the job could not be read, and holds none of that file's
  content

#### Scenario: Windows folder turned into a junction before the log exists
- **WHEN** on Windows the empty job folder is turned into a junction after the helper has opened it and before it has
  created `log.jsonl`
- **THEN** nothing is created in the target of the junction

### Requirement: Reports are kept for a limited time
Each time a report is written, the application SHALL delete the report files that are older than 90 days, and then
the oldest ones until at most 50 remain. It SHALL delete only files whose names match the report pattern.

#### Scenario: Many reports
- **WHEN** the folder holds 60 report files from the last week and one unrelated file
- **THEN** after the next report 50 report files remain, the newest ones, and the unrelated file is untouched

#### Scenario: Old reports
- **WHEN** the folder holds reports older than 90 days
- **THEN** they are deleted when the next report is written

### Requirement: Reports stay small
A report SHALL list at most 32 failures in detail per list (failed points, unreadable or mismatched blocks), count
the rest, and stay below 256 KiB.

#### Scenario: Every point failed
- **WHEN** all 1 875 points of a quick check failed
- **THEN** the report lists 32 of them and says that 1 843 more are not listed

### Requirement: The user can find the report
The Real capacity tab SHALL offer "Show report" for the selected drive when a report of a check of that drive was
written in this run of the program. The link SHALL reveal that file in Finder or Explorer. The application SHALL pick
the file itself, never a path that the window sends. `--cli probe`, `--cli full` and `--cli fix` SHALL print the
report path to stderr. A repair report SHALL NOT be what that link shows.

#### Scenario: Show the report of the failed check
- **WHEN** the last quick check of `disk6` failed, and the user presses "Show report"
- **THEN** Finder opens with that report selected

#### Scenario: Repair after a check
- **WHEN** a drive was checked and then repaired, and the user presses "Show report" on the Real capacity tab
- **THEN** the report of the check is revealed

#### Scenario: Command line
- **WHEN** `usbmemorychecker --cli probe disk6` ends
- **THEN** the last line on stderr names the report file
