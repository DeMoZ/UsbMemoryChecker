# Spec Delta

## ADDED Requirements

### Requirement: No repair while the drive waits for a replug
When the last quick check of a drive asked for a replug (its restore was not confirmed, a write-back failed, the
layout changed or vanished, or the drive stopped answering), the application SHALL refuse a repair of that drive with
`errReplugFirst` before the helper starts, whatever plan the request names, until the drive has left a successful
drive scan. The Repair tab SHALL say why the repair is not available.

#### Scenario: Repair right after the incident
- **WHEN** a quick check ended as "the drive stopped answering" and the partition table of the drive is gone, and a
  repair of the whole drive is requested
- **THEN** the request ends with `errReplugFirst`, no administrator prompt appears, and the drive is not touched

#### Scenario: Repair tab while waiting for the replug
- **WHEN** the selected drive waits for a replug
- **THEN** the Repair tab shows the replug text instead of a plan, and the repair button is disabled

#### Scenario: After the replug
- **WHEN** the drive has been unplugged and plugged in again
- **THEN** the Repair tab shows the plan from the drive's state as before
