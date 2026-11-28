# Design

## Context

See proposal.md for why. Facts this design rests on (read on 2026-11-28 at main 3ee8aff):

- **Release mechanics** (`.github/workflows/build.yml`). Every PR and every push to main runs the macOS job (i18n
  check, `cargo fmt --check`, clippy, `scripts/build-macos.sh`, which also runs `cargo test`) and the Windows job
  (`cargo test`, clippy, release build, zip with `UsbMemoryChecker.exe` plus the legacy `UsbMemoryCheker.exe`). A
  pushed `v*` tag also checks that the tag equals `v` + the `Cargo.toml` version, and the `release` job runs
  `gh release create <tag> --notes-file docs/releases/<tag>.md` with three files:
  `UsbMemoryChecker-macOS-<tag>.dmg`, `UsbMemoryChecker-<version>-macos-arm64.zip`,
  `UsbMemoryChecker-<version>-windows-x64.zip`. Plain branch pushes do not run CI (only PRs and main).
- **Earlier releases.** v0.1.3: annotated tag `v0.1.3` ("Version 0.1.3") on the merge commit of PR #5 (2731810),
  release not draft, not prerelease, the three files above. Commit 4a1d125 did the archive move, the version bump and
  the notes in one commit.
- **How the update window shows the notes.** `update::parse_release` takes the GitHub release `body` and
  `update::release_notes` cuts it at the first `"\n## Установка"` and then at the first `"\n## Install"`, strips
  leading `#` and spaces from every line and trims the result. The page puts that string into `#updNotes` with
  `textContent`; `.notes` is `white-space: pre-wrap`, `font-size: var(--fs-s)`, `max-height: 200px`, scrolls inside,
  in a modal at most 480 px wide. So the text is shown raw: `**bold**` shows its asterisks, backticks stay, and every
  line break in the file is a line break on screen. Both language sections are always shown, whatever the UI
  language. v0.1.0–v0.1.3 all have the same `release_notes` and `.notes` rules (checked with `git show
  <tag>:src/update.rs` and `:src/ui/index.html`), and it is the **installed old version** that renders the notes of
  the new one.
- **Signature.** `scripts/build-macos.sh` signs the app ad hoc (`codesign --force --deep --sign -`). The archived
  probe-diagnostics design records `codesign -d -r-` on the installed v0.1.3: `designated => cdhash H"…"`,
  `Signature=adhoc`, no team. Every build has a different cdhash, so a different designated requirement.
- **What v0.1.4 ships** is exactly `git diff v0.1.3..main`: PR #6 (`src/probe.rs`, `dio.rs`, `rawdev.rs`,
  `helper.rs`, new `report.rs`, `main.rs`, `capacity.rs`, `cli.rs`, `net.rs`, `disks.rs`, `src/ui/*`, READMEs,
  CLAUDE.md, specs). `src/fullcheck.rs`, `src/bench.rs` and `src/update.rs` did not change.
- The only places that carry the version are `Cargo.toml` (`version = "0.1.3"`) and the app's own entry in
  `Cargo.lock` (`name = "usbmemorychecker"`, `version = "0.1.3"`). `git grep 0.1.3` finds nothing else outside
  archives and old notes (the `0.1.2` / `0.2.0` in `scripts/ui-check/harness.js` are fixed test values).

## Goals / Non-Goals

**Goals:**
- A v0.1.4 release whose notes are true to the code, readable in the old app's update window, and published with
  all three files.
- A branch history where the OpsX change is archived before the merge, and nothing after the merge is a ticked or
  unticked task of an archived change.

**Non-Goals:**
- Any code, UI, script, workflow or README change, including fixing how the update window shows Markdown.
- Notarization or a stable signing identity (would remove the Full Disk Access caveat; not in scope).
- Issues #7–#14.

## Decisions

### D1. No spec delta: `skip_specs: true`

The schema's `specs` artifact describes behaviour; a release changes none. OpenSpec 1.14.2 supports this directly:
`skip_specs: true` in the change's `.openspec.yaml` makes `status` report `specs` as `skipped` and lets
`openspec validate` accept zero deltas. `openspec archive` then moves the folder without touching `openspec/specs/`.
Alternative rejected: a delta that restates an existing requirement or invents a "release notes" requirement. It
would put non-behaviour into the main specs and the schema's own guidance forbids it.

### D2. Notes format chosen for the update window

The update window is plain text (see Context). Rules for the two "What's new" sections of `v0.1.4.md`:

- **No inline Markdown** (`**`, backticks, links in `[]()` form) in `## What's new` and `## Что нового`. Earlier
  notes used `**…**`, and the old update window shows the asterisks. A bare URL is fine (text on both screens).
- **One physical line per bullet** (no hard wrap at 120 columns). In the 480 px modal a 115-column source line wraps
  once by itself and then breaks again where the file broke, which gives ragged lines with a 2-space indent. One line
  per bullet wraps naturally in the box and renders the same on GitHub.
- Section order and headings exactly `## What's new`, `## Что нового`, `## Install`, `## Установка`; nothing
  between or inside the first two sections may start with `## Install` (the cut is a plain substring split). The two
  install sections stay byte-identical to v0.1.3's: they never reach the window, and they are still right.
- LF line endings and a final newline (the cut looks for `"\n## …"`).

Alternative rejected: keep the earlier look (bold lead-ins, hard wrap). Better on GitHub, worse in the window, which
is where most users read them. The owner may prefer the old look; it is a text-only change either way.

### D3. What the notes may say about Full Disk Access

Certain: the app is signed ad hoc, not with an Apple Developer ID; its designated requirement is its cdhash; v0.1.4
has a different binary, so a different cdhash. Verified in the incident (probe-diagnostics design): macOS denied raw
access to the helper, and granting Full Disk Access plus restarting the app stopped it. Not tested: that an existing
grant stops working after an update. Hence the notes say **may**: "if you turned on Full Disk Access before, macOS
may not apply it to the new version, because the app is not signed with an Apple Developer ID; if the quick check
then says macOS blocked access, remove it from the list, add it again and restart the app". They must not say that
it always happens, that Full Disk Access is always needed, or that it is needed for the full check, speed test or
repair (only the quick check opens the raw device). The app's own notice (`fdaUpdate`) and README already give the
same advice. The in-app text is unconditional ("remove … and add it again"); the notes stay with "may". No change
to the app text in this release.

### D4. What the notes say about Windows

Windows is always released in this project. The app has never been run by hand on real Windows (#8): CI builds it,
runs the unit tests and clippy. PR #6 added Windows code that only CI has run (job folder via `NtCreateFile`,
`FlushFileBuffers` on the physical drive, the access-cause mapping), and the elevated path (UAC, `diskpart`) has
never run at all. The notes say plainly, in one bullet, that the Windows version is built and tested automatically
but has not been tried by hand on a real Windows PC yet, and where to report problems. They do not promise that
anything works on Windows beyond that.

### D5. Content: only what PR #6 ships, checked against the code

Each bullet of the draft (Appendix A) maps to code the executor re-reads before writing the file:

| Bullet | Where to check |
|---|---|
| Full Disk Access notice and button (macOS, quick check only) | `dio::cause_of` (EPERM → `Privacy` only on macOS with raw access), `main.rs` notice + `open_privacy`, `i18n.js` `fda*` |
| Busy / write-protected / unplugged get own messages | `main.rs` cause → `errAccessBusy` / `errAccessReadOnly` / `errDriveGone`; Windows codes in `dio` tests |
| "Stopped answering" instead of "keeps no data"; counts in verdicts | `probe.rs` `Abort::Unresponsive`, `decide`; `errUnresponsive`, `countIo`, `countMismatch` |
| No write without a saved copy | `probe.rs` (`unsaved` points; unreadable start → nothing written) |
| Read-back of start, end and first-pass points; layout compared | `probe.rs` `RestoreCheck` (first 16 MiB, last 1 MiB, coarse-round blocks; refine blocks `not_reread`), `helper.rs` `LayoutAfter` |
| Untrusted result not saved; replug first; app starts nothing on the drive until it left the list | `main.rs` `judge_probe`, `replug`, `errReplugFirst` |
| Reports for quick check, full check, repair; locations; "Show report"; what they never contain; nothing sent | `report.rs` (`redact`, limits, retention), `net::logs_dir`, `reveal_report` (check reports only), README "Reports" |

Claims the notes must not make (CLAUDE.md "Важно" and the specs): that a matching read-back proves the data is on
the flash; that "stopped answering" means the drive is fake or broken; that the quick check never loses data on any
drive (it restores and checks, it does not guarantee); that reports are sent anywhere; that the full check is
affected (it did not change). If the executor finds a draft claim that the code does not back, it rewrites the
claim to what the code does, in both languages, and notes it under task 3.1.

### D6. Version bump through Cargo

Edit only `Cargo.toml`; let `cargo check` rewrite the app's own `Cargo.lock` entry. Verify with `git diff Cargo.lock`
(one line) and later with `cargo test --locked`, which fails if the lock file needed more changes. No `cargo update`.

### D7. Order of commits, review, PR and archive

1. **Plan commit** (only `openspec/changes/release-0-1-4/`), so the active change exists in history and the
   pre-commit guard sees `tasks.md` for the next commit.
2. **Release commit** — `Cargo.toml`, `Cargo.lock`, `docs/releases/v0.1.4.md` and the ticked `tasks.md` (executor
   tasks 1–5). Review fixes, if any, are separate commits on top.
3. **Review** (OpsX step 3) on the release commit range until clean.
4. **Push the branch and open the PR.** CI runs on the PR for the release commit. This is the only CI run on the
   real Windows runner before archive, so a Windows failure is still fixed under the active change.
5. **Archive commit** — `openspec archive release-0-1-4 --yes` moves the folder to
   `openspec/changes/archive/<date>-release-0-1-4/` and changes nothing under `openspec/specs/` (D1). Only
   `openspec/` files change, so the guard does not need an active change any more. Push; CI runs again on the PR.
6. The owner merges the PR (merge commit, as with #1–#6).
7. **Tag and verify** on main after the merge (see D8).

Why not one "archive + version" commit as in 4a1d125: the archive moves `tasks.md` out of the active changes, and
the guard refuses an agent commit outside `openspec/` without an active change. Splitting also keeps CI on the
release commit before the plan is archived. Steps 4–7 are not tasks of the change: tasks.md lists them under
"Workflow follow-up" as plain bullets, so nothing after the archive is an open checkbox.

### D8. Tag and release verification

- Tag: annotated, like v0.1.1–v0.1.3: `git tag -a v0.1.4 -m "Version 0.1.4" <merge commit>`, after checking the
  merge commit is on `origin/main` and has `version = "0.1.4"`. Pushing the tag (`git push origin v0.1.4`) is not a
  push to a protected branch and not a force push; if a hook refuses it, show it to the owner as a command block.
- Wait for the tag's workflow run (`gh run list --branch v0.1.4 --event push`, `gh run watch --exit-status`).
- Check the release: not draft, not prerelease, it is what `GET /releases/latest` returns (that is what the in-app
  updater reads), exactly the three files with size > 0 and a `sha256:` digest, body equal to the notes file.
- Look inside without running anything: in a new scratch folder, download the two zips; the Windows zip lists
  `UsbMemoryChecker.exe` and `UsbMemoryCheker.exe`; the macOS zip unpacks (with `ditto -x -k`) to
  `UsbMemoryChecker.app` with `CFBundleShortVersionString` 0.1.4 and `codesign -dv` showing `Signature=adhoc`;
  `shasum -a 256` of each download equals the asset digest. Do not open the `.dmg` or start either app.

### D9. Checks and the update-window preview

- CLAUDE.md "Проверки и релиз": `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`
  (run as `--locked`), `node scripts/check-i18n.mjs`, `cargo clippy --all-targets --target x86_64-pc-windows-msvc --
  -D warnings`, plus `scripts/build-macos.sh` as the local mirror of the CI macOS packaging (checks the 0.1.4 file
  names and `Info.plist`).
- The ui-check scripts are not run: nothing under `src/ui/` or `scripts/` changes, and the harness's update state
  uses fixed notes, so it cannot show these notes. Instead the real window renders the real text: compute the window
  string with a scratch script that applies the two `release_notes` rules to the file, launch the release build
  with a scratch `HOME` and a `UMC_DEV_SCRIPT` that calls `window.onRustMessage({type:'update', …, available:
  {version:'0.1.4', notes}})` and `openUpdates()`, and take a `screencapture -o -l <window id>` image. A
  `target/release` binary is not inside an `.app`, so `install_target()` is `None`: no automatic update check, no
  network, no download.

## Risks / Trade-offs

- [Windows code from #6 never ran elevated on real Windows (#8)] → stated in the notes (D4); CI on the PR runs the
  Windows tests that exist. The owner releases Windows by project policy; a broken Windows quick check would need a
  v0.1.5.
- [The scratch copy of `release_notes` drifts from the Rust one] → it is two rules, `src/update.rs` did not change
  since v0.1.0, and the post-release check compares the real release body with the file.
- [Full Disk Access wording over- or under-claims] → "may", with the reason and the fix; based on D3 evidence only.
- [CI fails after the archive commit] → the archive commit touches only `openspec/`, so a failure there is
  unrelated flakiness; rerun. A real fix needs the change back: `git revert` the archive commit (agent may do it on
  the branch), fix, archive again.
- [Tag CI fails before `gh release create`] → no release exists; fix on a new branch through a PR, then the owner
  decides how to move the tag (deleting or moving a pushed tag is the owner's command, not the agent's).
- [`gh release create` fails part-way (release created, some files missing)] → do not delete anything; report to
  the owner with `gh release view v0.1.4 --json assets` and the failing log. Re-running the job fails while the
  release exists; the owner chooses between `gh release upload` of the missing files and recreating.
- [main moved after 3ee8aff before the merge] → before tagging, check that the commits between v0.1.3 and the merge
  commit are only PR #6 and this PR (`git log v0.1.3..<merge>`); anything else must be in the notes or stop.

## Migration Plan

Nothing to migrate: v0.1.4 reads v0.1.3's `settings.json` and `capacity.json` as they are (serde defaults added in
#6). Rollback after publishing: a release is not withdrawn (updaters may already have it); a problem gets v0.1.5.

## Open Questions

None that change the plan. The owner may override D2 (format) or drop the Windows bullet (D4) during plan review;
both are text-only.

## Appendix A. Draft of `docs/releases/v0.1.4.md`

The executor starts from this text, checks every claim (D5) and keeps the rules of D2. The install sections are
copied from `docs/releases/v0.1.3.md` unchanged. Bullets are one line each in the file (wrapped here only by the
list item).

```markdown
## What's new

- Clearer messages when the quick check cannot reach the drive. If macOS blocks direct access to it, the window says so, explains how to turn on Full Disk Access and has a button that opens that setting. A busy, write-protected or unplugged drive gets its own message.
- A drive that stops answering during the quick check is no longer reported as "keeps no data". The check then ends without a verdict and shows how many reads or writes failed and how many blocks came back different. The verdicts show these counts too, when there are any.
- Safer for your data. The quick check no longer writes over a block whose old content it could not read. After putting everything back, it reads the start and the end of the drive and the points of its first pass again and compares them with what was there before, and it compares the partitions with those from before the check.
- If the drive stopped answering or that comparison does not match, the result is not saved and the window asks you to unplug the drive and plug it in again before anything else. Do not format, initialize or repair it first, even if the system offers to. Until then the app starts nothing on that drive.
- A report for every check. Each quick check, full check and repair leaves a small report file, also when it failed or was cancelled: on a Mac in ~/Library/Logs/UsbMemoryChecker/checks, on Windows in %LOCALAPPDATA%\UsbMemoryChecker\logs\checks. "Show report" on the Real capacity tab opens the folder at the drive's last check report. Reports hold none of your files or data from the drive, no volume names and no serial numbers, and nothing is sent anywhere.
- Mac, after updating: if you turned on Full Disk Access for UsbMemoryChecker before, macOS may not apply it to the new version, because the app is not signed with an Apple Developer ID. If the quick check then says that macOS blocked access, remove UsbMemoryChecker from System Settings → Privacy & Security → Full Disk Access, add it again and restart the app.
- Windows: this version is built and its automatic tests run on Windows, but it has not been tried by hand on a real Windows PC yet. If something does not work, please report it at https://github.com/DeMoZ/UsbMemoryChecker/issues.

## Что нового

- Понятнее сообщения, когда быстрая проверка не может добраться до носителя. Если macOS блокирует прямой доступ к нему, окно так и говорит, объясняет, как включить полный доступ к диску, и даёт кнопку, которая открывает эту настройку. Для занятого, защищённого от записи и вынутого носителя — свои сообщения.
- Носитель, который перестал отвечать во время быстрой проверки, больше не считается «не хранящим данные». Проверка тогда заканчивается без вердикта и показывает, сколько чтений или записей не удалось и сколько блоков вернулись с другими данными. Вердикты тоже показывают эти счётчики, если они не нулевые.
- Бережнее к вашим данным. Быстрая проверка больше не записывает поверх блока, старое содержимое которого не удалось прочитать. Вернув всё на место, она заново читает начало и конец носителя и точки своего первого прохода и сравнивает их с тем, что там было, а также сравнивает разделы с тем, какими они были до проверки.
- Если носитель перестал отвечать или сравнение не совпало, результат не сохраняется, а окно просит прежде всего вынуть носитель и вставить снова. До этого не форматируйте, не инициализируйте и не исправляйте его, даже если это предлагает система. До тех пор программа ничего не запускает на этом носителе.
- Отчёт о каждой проверке. После каждой быстрой и полной проверки и каждого исправления остаётся небольшой файл отчёта, даже если они не удались или были отменены: на Mac в ~/Library/Logs/UsbMemoryChecker/checks, в Windows в %LOCALAPPDATA%\UsbMemoryChecker\logs\checks. «Показать отчёт» на вкладке «Реальный объём» открывает папку с последним отчётом о проверке носителя. В отчётах нет ваших файлов и данных с носителя, имён томов и серийных номеров, и они никуда не отправляются.
- Mac, после обновления: если вы раньше включили для UsbMemoryChecker полный доступ к диску, macOS может не применить его к новой версии, потому что программа не подписана сертификатом Apple Developer ID. Если быстрая проверка после этого сообщает, что macOS заблокировала доступ, удалите UsbMemoryChecker из списка «Системные настройки → Конфиденциальность и безопасность → Полный доступ к диску», добавьте снова и перезапустите программу.
- Windows: эта версия собирается и проходит автоматические тесты в Windows, но вручную на настоящем компьютере с Windows её ещё не проверяли. Если что-то не работает, напишите об этом: https://github.com/DeMoZ/UsbMemoryChecker/issues.

## Install

(copied from docs/releases/v0.1.3.md unchanged)

## Установка

(copied from docs/releases/v0.1.3.md unchanged)
```
