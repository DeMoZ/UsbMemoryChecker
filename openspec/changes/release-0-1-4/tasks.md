# Tasks

Work in `/Users/roman/orca/workspaces/UsbMemoryChecker/release-0.1.4` (branch `DeMoZ/release-0.1.4`). Read
proposal.md and design.md first; design.md Appendix A is the draft of the notes, D2 its format rules, D5 the claims
to check.

**Scope.** Change only `Cargo.toml`, `Cargo.lock`, the new `docs/releases/v0.1.4.md` and this `tasks.md` (ticks
and notes). Nothing under `src/`, `scripts/`, `.github/`, `README*.md`, `CLAUDE.md` or `openspec/specs/`. No push, no
PR, no tag, no archive: those are the workflow follow-up at the end.

**Safety rules for every step.** Real drives are attached to this Mac (a USB stick and the NVMe volume
`NVME_4TB`). Never run a speed test, a check, a repair, `unmountDisk` or any raw or volume write, and never press a
task button in a window you start. Every app run uses `HOME=<scratchpad>/home` (a folder in the session scratchpad),
so nothing is written under the real `~/Library/Application Support/UsbMemoryChecker/` or
`~/Library/Logs/UsbMemoryChecker/`. Do not kill an instance you did not start. Leave no background processes
(`pgrep -fl usbmemorychecker` shows none of yours at the end; `pgrep -x yes` shows nothing). No artificial load.
Stage files by name (`git add <paths>`), never `git add -A` / `.`; check `git diff --cached --stat` before every
commit. No AI attribution anywhere; commit messages in English.

## 1. Start

- [x] 1.1 Check the checkout. `git status -sb` is on `DeMoZ/release-0.1.4` with only
  `openspec/changes/release-0-1-4/` untracked or changed; `git stash list` has nothing of this task;
  `git fetch origin` and `git log --oneline -1 origin/main` is `3ee8aff` (if main moved, stop and report what the new
  commits are: they would have to be in the notes, design.md Risks); `git merge-base HEAD origin/main` is
  `3ee8aff`. Verify: the four facts written under this task.
  Done: branch `DeMoZ/release-0.1.4`, only `openspec/changes/release-0-1-4/` untracked; `git stash list` empty;
  after `git fetch origin`, `origin/main` is `3ee8aff` (Merge pull request #6); merge-base is `3ee8aff`.
- [x] 1.2 Before any run, record the state of the real app folders without changing them:
  `ls -la "$HOME/Library/Application Support/UsbMemoryChecker" "$HOME/Library/Logs/UsbMemoryChecker" 2>&1` and
  `ls -la "$HOME/Library/Logs/UsbMemoryChecker/checks" 2>&1 | wc -l`, saved to `<scratchpad>/real-before.txt`.
  Verify: the file exists (it is compared in 4.7).
  Done: `/private/tmp/claude-501/-Users-roman-orca-workspaces-UsbMemoryChecker-release-0-1-4/92dadd88-aa59-4e3d-95c6-883e6ad00613/scratchpad/real-before.txt`. The settings folder held `capacity.json` and `settings.json`; the real
  `~/Library/Logs/UsbMemoryChecker` did not exist.
- [x] 1.3 Commit the plan: `git add openspec/changes/release-0-1-4/.openspec.yaml
  openspec/changes/release-0-1-4/proposal.md openspec/changes/release-0-1-4/design.md
  openspec/changes/release-0-1-4/tasks.md`; `git diff --cached --stat` lists exactly these four files;
  `git commit -m "Plan the v0.1.4 release"`. Verify: `git show --stat HEAD` shows only those files and
  `openspec validate release-0-1-4` passes.
  Done: commit `78c5fed`, exactly the four files; `openspec validate release-0-1-4` valid.

## 2. Version 0.1.4

- [x] 2.1 In `Cargo.toml` set `version = "0.1.4"` (line 3, nothing else). Run `cargo check` so Cargo rewrites the
  app's own `Cargo.lock` entry (design D6; no `cargo update`). Verify: `git diff --stat` shows `Cargo.toml` and
  `Cargo.lock` with one changed line each; `git diff Cargo.lock` changes only the `version` under
  `name = "usbmemorychecker"`; `cargo metadata --no-deps --format-version 1 | python3 -c "import json,sys;
  print(json.load(sys.stdin)['packages'][0]['version'])"` prints `0.1.4`; `git grep -n '"0\.1\.3"' -- Cargo.toml
  Cargo.lock` finds nothing.
  Done: one changed line in each of `Cargo.toml` and `Cargo.lock` (the app's own entry); `cargo metadata` prints
  `0.1.4`; the `git grep` finds nothing.

## 3. Release notes

- [x] 3.1 Check every claim of the draft (design.md Appendix A) against the code at the places in design D5, in both
  languages, and against the "must not claim" list in D5, D3 (Full Disk Access: "may", reason, fix) and D4
  (Windows). Also check the UI words the notes quote: `showReport` and `tabCapacity` in `src/ui/i18n.js`
  (en "Show report" / "Real capacity", ru «Показать отчёт» / «Реальный объём») and the macOS settings path as the
  app writes it in `fdaSteps` (en and ru). Verify: under this task, one line per bullet: "holds" or the change made
  and why. A changed claim is changed in both languages.
  Done. UI words hold: `showReport` "Show report" / «Показать отчёт», `tabCapacity` "Real capacity" / «Реальный
  объём»; the settings path matches `fdaSteps` in en and ru. D3, D4 and the D5 "must not claim" list: nothing added
  or broken. Per bullet:
  - Full Disk Access / busy, write-protected, unplugged: holds.
  - "Stopped answering": changed (en, ru). "How many reads or writes failed" overstated the counter: `io_errors` and
    `mismatches` count test points only (failures at the start of the drive, in the baseline or at wrap-around targets
    are not added). Now: how many test points had a read or write error and how many came back with different data.
    The rest holds (v0.1.3 called any bad first point Broken).
  - Safer for your data: holds.
  - Result not saved, replug: changed (en, ru). The block lives in memory only (`State.notices`), so it ends when the
    app is quit. Added "while it stays open".
  - Reports: changed (en, ru). "Show report" exists only for checks made since the app started (`State.reports` is in
    memory). Added "After a check, until you quit the app". The rest holds (kinds, paths, contents, nothing sent).
  - Mac, after updating: holds ("may", reason, fix).
  - Windows: holds (CI runs `cargo test` and clippy on Windows).
- [x] 3.2 Write `docs/releases/v0.1.4.md` from the checked draft: `## What's new`, `## Что нового`, then `## Install`
  and `## Установка` copied from `docs/releases/v0.1.3.md`. Format rules of design D2: no `**`, no backticks and no
  `[text](url)` in the two "What's new" sections, one physical line per bullet, a blank line between sections, LF
  endings, final newline. Verify all of:
  `diff <(sed -n '/^## Install$/,$p' docs/releases/v0.1.3.md) <(sed -n '/^## Install$/,$p' docs/releases/v0.1.4.md)`
  prints nothing; `grep -n '^## ' docs/releases/v0.1.4.md` prints the four headings in that order and no other;
  `sed -n '1,/^## Install$/p' docs/releases/v0.1.4.md | perl -ne 'print "$.: $_" if /\*\*|\x60|\]\(/'` prints
  nothing (no bold, backtick or Markdown link); `sed -n '1,/^## Install$/p' docs/releases/v0.1.4.md | grep -nE '^  '`
  prints nothing (no wrapped continuation lines); `file docs/releases/v0.1.4.md` does not mention CRLF;
  `tail -c1 docs/releases/v0.1.4.md | xxd` shows `0a`; the English section has no Cyrillic
  (`sed -n '/^## What/,/^## Что/p' docs/releases/v0.1.4.md | perl -CSD -ne 'print if /\p{Cyrillic}/'` prints only
  the `## Что нового` line; macOS grep has no `-P`).
  Done: all the listed commands give the expected output (empty install diff, four headings, no Markdown, no wrapped
  lines, no CRLF, final `0a`, no Cyrillic in the English section).
- [x] 3.3 Compute the text the update window will show. In the scratchpad (not in the repo) write a small Node
  script that does what `src/update.rs` `release_notes` does: cut the file at the first `"\n## Установка"`, then at the
  first `"\n## Install"`, strip leading `#` characters and then leading whitespace from every line, trim the whole.
  Save its output as `<scratchpad>/notes-window.txt`. Verify: the text starts with `What's new`, contains
  `Что нового`, contains neither `Install` nor `Установка` nor `.dmg`, has no `#` at a line start, and every bullet is
  one line (`grep -c '^- '` equals the number of bullets in the file's first two sections).
  Done: `/private/tmp/claude-501/-Users-roman-orca-workspaces-UsbMemoryChecker-release-0-1-4/92dadd88-aa59-4e3d-95c6-883e6ad00613/scratchpad/notes-window.txt` starts with `What's new`, has `Что нового`, no `Install` / `Установка` / `.dmg` and
  no leading `#`; 14 bullet lines, 14 bullets in the file.
- [x] 3.4 Look at it in the real window (WKWebView). `cargo build --release`. Make `<scratchpad>/home`. Build the dev
  script with Node so the notes are JSON-escaped:
  `window.onRustMessage({type:'update',checking:false,downloading:false,available:{version:'0.1.4',notes:<text of
  3.3>,page:''},writable:false,status:null}); openUpdates();`. Start
  `HOME=<scratchpad>/home UMC_DEV_SCRIPT="$(cat <scratchpad>/dev-script.js)" target/release/usbmemorychecker` in the
  background and note its PID. The binary is not inside an `.app`, so there is no automatic update check (design
  D9). Find the window id (for example a scratch Swift script with `CGWindowListCopyWindowInfo` filtered by that PID)
  and capture `screencapture -o -l <id> <scratchpad>/update-window.png` after about 4 s; scroll the notes box once
  more with a second dev script if the end is not visible, or capture the box after
  `document.getElementById('updNotes').scrollTop = 1e6` in the same script. Then stop only that PID. Do not touch any
  drive in the window. Verify: the PNG shows the notes with naturally wrapped bullets, no literal `**`, backticks or
  `#`, both languages, and no install text; `pgrep -fl usbmemorychecker` shows no process you started; the path of
  the PNG is written under this task.
  Done: `/private/tmp/claude-501/-Users-roman-orca-workspaces-UsbMemoryChecker-release-0-1-4/92dadd88-aa59-4e3d-95c6-883e6ad00613/scratchpad/update-window.png` (top of the notes box) and `/private/tmp/claude-501/-Users-roman-orca-workspaces-UsbMemoryChecker-release-0-1-4/92dadd88-aa59-4e3d-95c6-883e6ad00613/scratchpad/update-window-end.png`
  (scrolled to the end: Russian section, ends at the issues URL). Bullets wrap naturally; no `**`, backticks, `#` or
  install text. The first attempt, on a cold start, captured the window without the dialog (probably the dev script
  ran before the page was ready); the same script worked on the next start. No task button pressed; only my PIDs
  stopped; `pgrep` shows none.

## 4. Checks

- [x] 4.1 `cargo fmt --check` passes. Verify: exit code 0.
  Done: exit code 0.
- [x] 4.2 `cargo clippy --all-targets -- -D warnings` passes. Verify: exit code 0, no warnings.
  Done: exit code 0, no warnings.
- [x] 4.3 `cargo test --locked` passes (fails if `Cargo.lock` needed more than the 2.1 edit). Verify: exit code 0;
  the test count is written under this task.
  Done: exit code 0, 289 passed, 0 failed.
- [x] 4.4 `node scripts/check-i18n.mjs` passes. Verify: exit code 0.
  Done: exit code 0 (`i18n ok: 5 languages × 212 keys, 199 keys used`).
- [x] 4.5 Windows from the Mac: `rustup target list --installed` contains `x86_64-pc-windows-msvc` (if not, run
  `rustup target add x86_64-pc-windows-msvc`), then `cargo clippy --all-targets --target x86_64-pc-windows-msvc --
  -D warnings` passes. Verify: exit code 0.
  Done: the target was already installed; exit code 0, no warnings.
- [ ] 4.6 Local mirror of the CI macOS packaging: `scripts/build-macos.sh` (it runs `cargo test`, builds
  `aarch64-apple-darwin`, signs ad hoc, writes the gitignored `dist/`). Do not open the `.dmg` and do not start the
  built `.app`. Verify: `ls dist` shows `UsbMemoryChecker.app`, `UsbMemoryChecker-macOS.dmg` and
  `UsbMemoryChecker-0.1.4-macos-arm64.zip`; `plutil -extract CFBundleShortVersionString raw
  dist/UsbMemoryChecker.app/Contents/Info.plist` prints `0.1.4`; `codesign -dv dist/UsbMemoryChecker.app 2>&1`
  shows `Signature=adhoc` (this is the D3 fact the notes rely on); `git status --porcelain` shows no `dist/` entry.
  NOT DONE: `scripts/build-macos.sh` exits 1 at its last step, `hdiutil create`, so there is no `.dmg`.
  Repeating that step without `-quiet` gives `create failed - no mountable file systems` (error 113), and so does a
  control image of a folder with one text file in the scratchpad: it fails on this machine (macOS 27.0.1) whatever is
  packed. The script was not changed. What did pass: its `cargo test` (289), the `aarch64-apple-darwin` build,
  `dist/UsbMemoryChecker.app` and `UsbMemoryChecker-0.1.4-macos-arm64.zip` exist, version `0.1.4`,
  `Signature=adhoc`, no `dist/` in `git status`. The `.dmg` is left to CI on the PR. Each failed `hdiutil create` left
  its own temporary image mounted at `/Volumes/UsbMemoryChecker` (or `UMCctl`); those images (identified by their
  backing file, protocol Disk Image) were detached and the script's leftover staging folder removed.
- [x] 4.7 The ui-check scripts are not needed: `git diff --stat 3ee8aff -- src scripts .github README.md
  README.ru.md CLAUDE.md openspec/specs` prints nothing, so no window, script, workflow, README or spec changed
  (design D9; the real window was checked in 3.4). Compare the real app folders with 1.2 (same `ls -la` into
  `<scratchpad>/real-after.txt`, `diff` with `real-before.txt`). Verify: both commands print nothing; if the real
  folders changed, stop and report which run did it.
  Done: the `git diff --stat` and the `diff` of `real-before.txt` / `real-after.txt` print nothing.
- [x] 4.8 `openspec validate release-0-1-4` and `openspec validate release-0-1-4 --strict` pass, and
  `openspec status --change release-0-1-4` shows `specs` as skipped. Verify: the three outputs.
  Done: both validations pass (`skip_specs` info line); `status` shows `[~] specs (skipped: change declares
  skip_specs)`.

## 5. Release commit

- [x] 5.1 Tick tasks 1.1–5.1 in this file with the notes asked for above, then `git add Cargo.toml Cargo.lock
  docs/releases/v0.1.4.md openspec/changes/release-0-1-4/tasks.md`; `git diff --cached --stat` lists exactly these
  four files; `git commit -m "Version 0.1.4: release notes for the quick check diagnostics"`. Verify:
  `git show --stat HEAD` shows only those files, `git log -1 --format=%B` has no AI attribution, `git status -sb`
  is clean apart from gitignored files, and `pgrep -fl usbmemorychecker` shows none of yours.
  Done: every task through 5.1 is ticked except 4.6 (see its note); the release commit holds the four files.

## Workflow follow-up

Not tracked tasks of the change; done by the orchestrating session and the owner, in this order (design D7, D8).

- **Review** (OpsX step 3) of the commit range from 1.3 to 5.1 against this change; fixes go back to an executor as
  separate commits until the review is clean (at most 3 rounds).
- **Push and PR.** `git status -sb` (never on main), `git push -u origin DeMoZ/release-0.1.4`, then
  `gh pr create --base main --head DeMoZ/release-0.1.4 --title "Version 0.1.4"` with an English body and no AI
  attribution: what the release contains (PR #6, quick check diagnostics), the notes file, the Full Disk Access note
  on macOS, Windows not tried by hand (#8), known gaps not in this release (#7–#14), and the checks run. Wait for the
  PR's CI run (`gh pr checks --watch`) to pass on macOS and Windows. A failure here is fixed under the active
  change (new task, executor, review).
- **Archive on the branch.** `openspec archive release-0-1-4 --yes`. Check that the folder moved to
  `openspec/changes/archive/<date>-release-0-1-4/`, that `git status --porcelain openspec/specs` is empty, and that
  `openspec list --json` shows no active change. `git add openspec/changes`, `git diff --cached --stat` shows only
  the rename of this change's files, `git commit -m "Archive the release-0-1-4 change"`, `git push`. Wait for the
  PR's CI run on this commit to pass.
- **Merge — the owner's step.** Show it as a command block (merge commit, like #1–#6), never run it:
  `gh pr merge <number> --merge --delete-branch`.
- **Tag.** `git fetch origin --tags`; take the merge commit (`gh pr view <number> --json mergeCommit -q
  .mergeCommit.oid`); check that it is `origin/main` (or an ancestor of it), that
  `git show <sha>:Cargo.toml | grep '^version'` is `version = "0.1.4"`, and that `git log --oneline v0.1.3..<sha>`
  holds only PR #6 and this PR. Then `git tag -a v0.1.4 -m "Version 0.1.4" <sha>` and `git push origin v0.1.4` (a
  tag push, not a branch push; if a hook refuses it, show it to the owner as a command block).
- **Wait for CI.** `gh run list --branch v0.1.4 --event push --limit 1 --json databaseId,status,conclusion`, then
  `gh run watch <id> --exit-status`: the macOS, Windows and release jobs pass, including "Check the version".
- **Check the release.** `gh release view v0.1.4 --json tagName,isDraft,isPrerelease,assets,body`: not draft, not
  prerelease; assets exactly `UsbMemoryChecker-macOS-v0.1.4.dmg`, `UsbMemoryChecker-0.1.4-macos-arm64.zip`,
  `UsbMemoryChecker-0.1.4-windows-x64.zip`, each with size > 0 and a `sha256:` digest; the body equals
  `docs/releases/v0.1.4.md` apart from trailing whitespace. `gh api repos/DeMoZ/UsbMemoryChecker/releases/latest
  --jq .tag_name` prints `v0.1.4` (what the in-app updater reads).
- **Look inside the files without running them.** In a new empty scratch folder: `gh release download v0.1.4
  --pattern '*.zip'`; `shasum -a 256` of each equals its asset digest; `unzip -l` of the Windows zip lists
  `UsbMemoryChecker.exe` and `UsbMemoryCheker.exe`; `ditto -x -k` the macOS zip into a subfolder and check
  `plutil -extract CFBundleShortVersionString raw UsbMemoryChecker.app/Contents/Info.plist` is `0.1.4` and
  `codesign -dv UsbMemoryChecker.app` shows `Signature=adhoc`. Do not open the `.dmg`, do not start either app.
- **Report** to the owner: the release link, the three files, and that the in-app update window of an installed
  v0.1.3 will show these notes (the owner may look at it in his own app; agents do not run the real app).
- **Clean up** after the merge: remove this Orca worktree (`orca worktree rm`), and delete the local branch.
