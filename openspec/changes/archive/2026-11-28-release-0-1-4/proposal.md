# Proposal

## Why

The owner asked on 2026-11-28 to release v0.1.4. Since v0.1.3, main has gained PR #6 (quick check diagnostics:
Full Disk Access notice, no overwrite without a saved copy, restore and layout check, "stopped answering" instead of
"keeps no data", per-check reports). Users get it only through a release: the in-app updater offers whatever
`releases/latest` on GitHub is. The CI run for main at 3ee8aff passed.

## What Changes

- Version `0.1.3` → `0.1.4` in `Cargo.toml` and in the app's own entry in `Cargo.lock` (nothing else in the lock
  file). The CI tag job refuses a tag that does not match `Cargo.toml`.
- New `docs/releases/v0.1.4.md`, English and Russian, in the structure of the earlier files: `## What's new`,
  `## Что нового`, `## Install`, `## Установка`. CI uses it as the GitHub release body. The update window of the
  installed (older) version shows it as plain text, without the two install sections.
- The notes describe for end users what PR #6 really does (checked against the code), say that on a Mac it may be
  necessary to remove the app from Full Disk Access and add it again after updating, and say that the Windows build
  has not been tried by hand on real Windows.
- Release flow: PR from `DeMoZ/release-0.1.4` to `main` (English, no AI attribution). The owner merges it. Then the
  tag `v0.1.4` on that main commit, CI publishes the release, and the release is checked for the `.dmg`, the macOS
  update zip and the Windows zip.
- No code change. Nothing under `src/`, `scripts/`, `.github/`, `README*.md` or `CLAUDE.md` changes.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

None. The release changes no requirement: the behaviour it ships was specified and archived with
`2026-11-28-probe-diagnostics` (`quick-capacity-check`, `check-report`, `drive-capacity-knowledge`, `drive-repair`).
The change therefore sets `skip_specs: true` in `.openspec.yaml`, the CLI's marker for a change with no spec-level
behaviour change, instead of a made-up spec delta.

## Impact

- Files: `Cargo.toml`, `Cargo.lock`, `docs/releases/v0.1.4.md` (new). Planning files under
  `openspec/changes/release-0-1-4/`, archived before the merge.
- Users: the in-app updater (macOS and Windows) offers v0.1.4 once the release is published. On macOS an existing
  Full Disk Access grant may no longer apply to the updated app (see design.md D3).
- Out of scope: the known gaps filed as GitHub issues #7–#14 (including #7, the diskpart script in `%TEMP%` on
  Windows, which predates this release). Manual testing on real Windows (#8).
