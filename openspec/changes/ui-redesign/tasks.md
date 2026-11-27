# Tasks

Specs: `specs/window-layout/spec.md`, `specs/window-appearance/spec.md`. How: `design.md` (D1–D10). Reference mockup
(not in the repo): `/private/tmp/claude-501/-Users-roman-Git-UsbMemoryChecker/aea22114-dfc3-4ece-9c2a-f12ffa73f126/scratchpad/ui-redesign/mockup-src.html`
with renders in the same folder. If the scratch folder is gone, design.md D1–D7 is the source of truth.

Rules for the whole change:
- Work on a branch stacked on `fix-after-repair` (or on `main` once it is merged). The fix-after-repair logic in
  `index.html` (`repairPlan`, `renderRepair`, `askRepair`, `doRepair`, `renderRepairOffer`, `repairedVolume*`,
  `verifyRepaired`, `rescan`/`clearRescan`, `selectDrive`, `taskDone`, `onRustMessage`) is moved, not rewritten. Its
  element ids stay.
- Never start a speed test, check or repair on a real drive while testing. Use the harness (it disables `send`) or a RAM
  disk as described in CLAUDE.md. Launch the GUI only with a scratch `HOME`.

## 1. Layout check tool, run against the current window

- [x] 1.1 Create `scripts/ui-check/harness.js` (D9). It builds the fake `init`, `update`, `disks` + `capacity`,
  `task`, `point`, `rescale` and `taskDone` messages for every scenario in D9, swallows real Rust messages, replaces
  `send` with a recorder (`window.__sent`), and resets `results`, `task`, `series`, `capacity`, ticks and dialogs
  between scenarios. Exposes `__umcScenario(name, lang, opts)`. Verify: in the composed current page, every scenario
  name runs without a JS error, and `__sent` stays empty.
- [x] 1.2 Create `scripts/ui-check/measure.js`. It returns `{page:{scrollH,clientH,overflow}, scrollers:[…],
  below:[…]}`. Allowed scrollers are a parameter (default `.drives`, the results table). `below` lists visible
  controls, progress cards, verdicts, notes and offers outside their pane or the window. Verify: on the current page
  at 1180×800 the `capFake` scenario reports the verdict and "Repair to" as below (as in design.md "Measured problem").
- [x] 1.3 Create `scripts/ui-check/render.swift` (offscreen WKWebView: size, appearance, query, JS, PNG snapshot,
  JSON line per job; CSS animations off for snapshots) and `scripts/ui-check/run.mjs`. `run.mjs` composes
  index.html + i18n.js like `page()`, writes jobs for states × languages × themes × sizes, runs WKWebView on macOS
  and headless Chrome or Edge (own `--user-data-dir` in the temp folder, `--dump-dom` report). It prints a summary
  table and exits 1 on a failure. Options: `--sizes 1180x760,880x600`, `--langs`, `--states`, `--engine
  webkit|chromium|both`, `--out <dir>` for PNGs. Verify: `node scripts/ui-check/run.mjs --sizes 1180x800 --engine
  webkit` on the current page reproduces the design.md table within ±5 px and exits 1.
- [x] 1.4 Create `scripts/ui-check/contrast.mjs`. It reads the `:root` and `prefers-color-scheme: light` token
  blocks from `index.html` and checks the pairs from spec "Contrast": text, text-2 and text-3 on s1, s2, s3 ≥ 4.5;
  signal, ok, warn and bad on s1 and s2 ≥ 3 (≥ 4.5 where used as text); signal-ink on signal and bad-ink on bad ≥ 4.5;
  edge on s1 and s2 ≥ 3. Verify: it runs on the current page and prints the ratios. Failing pairs of the old palette
  are expected and are only reported in this group.
- [x] 1.5 Add the layout check to CLAUDE.md ("Проверки и релиз", in Russian, as the file is): the command, what it
  checks, that it never starts a task. Verify: the documented command runs as written.

## 2. Lumen tokens and base styles

- [x] 2.1 Replace the `:root` / light token blocks with the D3 table (colours, radii, spacing, type scale with
  `clamp()`, motion durations). Remove `.aurora`, `--grad*` except `--brand` for the logo, `backdrop-filter`, and
  drop shadows. Add the static ambient light element tinted by `body[data-state]`. Verify: `node
  scripts/ui-check/contrast.mjs` passes for both themes.
- [x] 2.2 Base element styles from the mockup: `.surface` (rim + hairline), `.btn` (primary, danger, ghost, small,
  disabled), `.seg`, `.pill`, `.chip`, `.input`, `.tick`, `.note` (warn and info), `.label`, `.hint`,
  `:focus-visible` ring, scrollbar. Only the features listed in D3 "Engine safety". Verify: a grep finds no
  `backdrop-filter`, `oklch(`, `:has(`, `@container` or `url(http` in `index.html`.

## 3. Window skeleton: title bar, drive rail, drive strip, work card

- [x] 3.1 Grid skeleton (D1, D7): `.app` rows `48px minmax(0,1fr)`; `.main` columns `248px minmax(0,1fr)`; stage rows
  `auto minmax(0,1fr)`; `min-height: 0` on every grid child; page `overflow: hidden`. Keep the macOS title bar left
  padding and the drag handler. Verify: ui-check `idle` at 1180×760 and 880×600 shows no page overflow.
- [x] 3.2 Title bar: brand + tagline (hidden below 980 px), language select, update pill (dot + new colour when an
  update is available), Support pill. Same handlers and `data-i18n*` keys as now. Verify: the ui-check `idle` and
  `toast:update` screenshots show both pill states, and `check-i18n` passes.
- [x] 3.3 Drive rail: heading + Reload (busy state kept from `rescan()`), listbox of drive options (icon, name,
  size · label or `notMounted`, used bar, state icon or busy mark per spec "Drive rail shows the capacity state"),
  searching, empty state, `sideFoot` at the bottom. The list scrolls inside. Verify: ui-check `nodrives`, `searching`
  and a scenario with 12 drives (rail scrolls, page does not). The state icon tooltips name the state in all 5
  languages.
- [x] 3.4 Drive strip (D4): name, chips (the third chip hidden below 1100 px), capacity map + legend from
  `capacity[id]`, and the three readouts. Real capacity uses the existing `setReal` mapping plus the icon per class
  and a `title` with the full hint. Remove the ring; the `used` key moves to the legend. Verify: ui-check
  screenshots of `capFake` (red hatch past 8.3 GB, legend "real · fake"), `repRepaired` (hatch past the partition),
  `capLimited` (amber hatch, full hint as tooltip) and `capUnknown` (used only) match spec "Capacity map".
- [x] 3.5 Work card: tab list (`role=tablist`, badge kept from `renderDriveView`), the panel area (`.panel` two
  columns `5fr/7fr`, one column below 1100 px with `grid-auto-rows: max-content` and `min-height: auto` on the
  columns), and the placeholder when no drive is selected. Verify: ui-check at 900×600 shows one column, no overlap
  (compare with `10-min-size-900x600-ru-dark.png`), and no page scroll.

## 4. Speed tab

- [x] 4.1 Setup column: title, `speedText`, size seg (`radiogroup`, "does not fit" disabled with the `sizeNoFit`
  tooltip as now), full-width Start, progress in its place while a speed test runs on the selected drive (D5,
  spec "Progress in place of the action"), 2×2 readouts. Verify: ui-check `idle`, `speedRun`, `speedDone` at
  1180×760 in 5 languages: no overflow, the Start button is absent and the progress card with Stop is present
  during `speedRun`.
- [x] 4.2 Outcome column: chart (flex, at least 160 px, empty text when there is no series), legend, results table
  with a sticky header, at most about 132 px high, so the newest 3 runs are visible and more scroll inside. Verify:
  `speedDone` with 8 runs keeps the page still; only the table scrolls.
- [x] 4.3 `drawChart(canvas)` takes a canvas and uses the token colours (D6). Speed tab redraws on `point` and
  `rescale` as now. Verify: ui-check `speedRun` in light and dark shows the write curve in `--signal` with axis text
  in `--text-3`. Switching the system appearance redraws on the next point.

## 5. Real capacity tab

- [x] 5.1 Setup column: title, `capText` (in the outcome placeholder instead when there is no check), the two mode
  cards as a radiogroup (title + tag; long description as `title`), the selected mode's description, full-width
  Check or the progress card while a quick or full check runs. Verify: ui-check `capUnknown`, `capRun` in de and ru
  show the Check button or the progress card with Stop fully visible.
- [x] 5.2 Outcome column: the limited note, then the verdict built by the existing `renderVerdict` text logic
  (titles, texts, split bar, faint line, "Repair to" button) restyled per the mockup. While a full check of the
  selected drive runs, `#chartCap` with the live series is shown instead (D6). Verify: ui-check `capFake`,
  `capDamaged`, `capBroken` (no Repair button, no "Repair to 0 B"), `capFull2tb`, `capPartOk`, `capPartWriteError`,
  `capLimited`, `capUnconfirmed`, `capRun` at 1180×760 in 5 languages with no overflow. In `capRun` the canvas shows
  the points.

## 6. Repair tab (logic unchanged, new placement)

- [x] 6.1 Setup column: title, lead (`repLead`), new size with hint, file system seg (FAT32 rule unchanged) and name
  input side by side, combined hint. Outcome column: done + offer card (`repDone`, `repVerify`, `repVerifyBtn`,
  `repVerifyStatus`), note (`repNote`), confirm box with `repWholeBox` (tick + warning), `repConfirm`, `repBtn`. The
  progress card replaces the confirm box while repairing. All ids unchanged. Verify: ui-check `repUnknown`,
  `repGenuine`, `repFake`, `repFakeWhole`, `repRepaired`, `repRepairedOffer`, `repOfferWaiting`, `repOfferNoMount`,
  `repLimited`, `repLimitedWhole`, `repRun`, `repBroken`, `repUnconfirmed` at 1180×760 in 5 languages, with no
  overflow and every listed control visible.
- [x] 6.2 IPC equivalence check `scripts/ui-check/ipc.mjs`. It runs the same scripted clicks on the old page
  (`git show <base>:src/ui/index.html`) and on the new one through the harness recorder: select drive, change size,
  Start, Stop, quick and full check, Repair default, Repair with the full-size tick, cancel in the dialog, Check the
  repaired drive, Reload twice, language change, update and donate actions. It compares `__sent`. Verify: the lists
  are identical (spec "Same requests"), and a tick made in an older state is cleared in both pages (spec "Safety
  conditions unchanged").

## 7. Dialogs, toasts and keyboard

- [x] 7.1 Restyle the four dialogs (Support, updates, confirm repair, confirm quit) with the tokens. Add
  `role=dialog`, `aria-modal`, `aria-labelledby`, focus into the dialog (Cancel first in the repair and quit dialogs),
  a Tab trap, restore focus on close, Escape. Add the `btnClose` key in en, ru, es, de, zh (D8) as the `aria-label` of
  the ✕ buttons. Verify: `node scripts/check-i18n.mjs` passes; ui-check `modal=donate|update|repair|quit` shows each
  dialog within the window at 1180×760 and 880×600; a harness test presses Escape and checks that focus returned to
  the opener.
- [x] 7.2 Toasts: `role=alert` for errors, `status` otherwise; close on click and on Escape (when no dialog is open).
  Verify: a harness test shows an error toast, sends Escape and finds no toast left.
- [x] 7.3 Keyboard: tab list (Left, Right, Home, End), drive listbox with roving `tabindex` (Up, Down, Home, End;
  selection goes through `selectDrive`, so the full-size tick resets), and radiogroups (sizes, modes, file system)
  with arrow keys that skip disabled options. Visible focus everywhere. Verify: harness key-event tests for each
  group. A manual Tab walk in the app follows the order in spec "Keyboard only".

## 8. Motion

- [x] 8.1 Motion per D3: 120, 200 and 320 ms, the progress bar 300 ms linear, spinner only while busy, no idle loops
  (the plug float and the drive busy pulse only while a task runs). `prefers-reduced-motion: reduce` turns off every
  animation and transition and shows a static spinner ring. Verify: with reduced motion emulated in headless Chromium,
  `document.getAnimations().length === 0` in `idle` and in `speedRun`. Without it, `idle` has no running animation.

## 9. Window size (only Rust change)

- [x] 9.1 `src/main.rs`: `with_inner_size(LogicalSize::new(1180.0, 760.0))` and `with_min_inner_size(LogicalSize::new(880.0,
  600.0))`. Verify: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check` and `cargo clippy
  --all-targets --target x86_64-pc-windows-msvc -- -D warnings` pass. Launched with a scratch HOME, the window is
  1180×760 (`screencapture -l` image of 2360×1520 on Retina) and cannot be dragged below 880×600.

## 10. Docs and screenshots

- [x] 10.1 Take `docs/screenshot-capacity.png` (a fake "2 TB" drive that holds 31.9 GB, as the README alt text says)
  and `docs/screenshot-speed.png` (speed test with chart and results) again from the real app at 1180×760.
  Use `UMC_DEV_SCRIPT` with the harness scenarios `capFull2tb` and `speedDone`, a scratch HOME and
  `UMC_SHOW_VIRTUAL=1`; capture with `screencapture -o -l <window id>`. No real test runs. Verify: both files are
  updated, and README.md and README.ru.md still describe them correctly (alt texts unchanged or changed in both).
- [x] 10.2 Check README.md and README.ru.md for descriptions of the old layout (ring, "below", tab positions) and
  update both the same way if one changes. Update the "Устройство" part of CLAUDE.md where it describes the window
  (`scripts/ui-check`, `#chartCap`). Verify: `git diff` shows matching changes in both READMEs, or none.

## 11. Integration checks

- [x] 11.1 Full layout check: `node scripts/ui-check/run.mjs --sizes 1180x760,1600x1000,880x600 --engine both`.
  Verify: exit 0. At 1180×760 and 1600×1000 there is no overflow and nothing below the fold in any scenario × language
  × theme. At 880×600 there is no page scroll, and every control is inside the scrollable panel.
- [x] 11.2 Project checks: `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, `node
  scripts/check-i18n.mjs`, Windows clippy. Verify: all pass.
- [x] 11.3 In-app look: launch the release build with a scratch HOME, `UMC_SHOW_VIRTUAL=1` and `UMC_DEV_SCRIPT`
  for `idle`, `speedRun`, `capFake` and `repRepairedOffer`, in light and dark. Capture the window, compare it with the
  mockup PNGs, and quit. Then a RAM-disk run (CLAUDE.md recipe) of one quick check and one 100 MB speed test with the
  new window, to see live progress, chart and verdict end to end. Verify: screenshots saved next to the PR notes; no
  visual or functional difference from the specs.

## 12. Review round 1 fixes

- [x] 12.1 Blocker: the progress card cache (`dataset.kind`, `dataset.stage`) is cleared when no task runs, so a second task of the same kind rebuilds its card and Stop. Verify: ipc.mjs multi-step scenarios (speed, quick and full twice, drive switch and back, repair twice, repair to offer to verify, plan/layout/state change clears the full-size tick).
- [x] 12.2 Results table fits its column (units dimmed, wraps, bad-blocks mark under the size, no sideways scroll). Verify: scenario `speedBad`, run.mjs at all sizes.
- [x] 12.3 measure.js detects sideways clipping and ellipsis without a tooltip; Real capacity readout text shrinks. Verify: deliberately cropped Start button and readout are reported.
- [x] 12.4 Harness reset clears the card caches; run.mjs loads the page once per language. New scenarios: `repNoPlan`, `speedBad`, `speedRunRead`, `capRunRefine`, `capRunRestore`, `modalUpdateChecking|Downloading|Install`, `toastHelper`, `toastNoSpaceFull`.
- [x] 12.5 Tooltips on names, chips, stage text; the map legend wraps; focus rings are not clipped (`.panel`, `.drives` padding); `:focus` fallback and solid backdrop fallback for macOS 11; stage text written only on change; `check-i18n.mjs` scans every `data-i18n-*`; render.swift has a navigation-failure handler and a 30 s timeout, runners have timeouts.
- [x] 12.6 Nits: panel fade on tab change, unused spacing tokens removed, select chevron per theme, design.md aligned (`--edge`, 84 px, focus and colour-mix wording), CLAUDE.md wording.

## Workflow follow-up

- Review (Opus, high effort) against both specs and the fix-after-repair specs. Fixes go back to the executor until
  the review is clean.
- `/opsx:archive` after the review, then commit, push and PR (the merge into `main` is Roman's step).

## 13. Round 2: app icon for small sizes, Windows window icon, README

Specs: `specs/window-appearance/spec.md` ("App icon is readable at every size", "Windows shows the app icon on the
window"). How: design.md D11–D13. The version, release notes and the release itself are not part of this group.

- [x] 13.1 Rewrite `scripts/make-icon.swift` per D11: one drawing function in tile-relative units, small (≤48,
  16-unit grid, pixel-snapped, no arc/shadow/gradient below 24) and large (≥64, with the one-colour arc) geometry,
  `win-*` (full canvas, radius 22%) and `mac-*` (Big Sur grid, shadow only ≥128) variants, the Lumen colours of the
  D11 table, and the ICO writer (DIB ≤48, PNG ≥64, un-premultiplied). Verify: `swift scripts/make-icon.swift <tmp>`
  writes the 7 `mac-*` and 9 `win-*` PNGs and `icon.ico`; `sips -g pixelWidth -g pixelHeight` shows each file at its
  own size.
- [x] 13.2 Add the `--sheet` mode (D11) and rewrite `scripts/make-icon.sh` to use the native PNGs (iconset mapping
  from D11, no `sips -z`, no Python), with an optional `--sheet <out.png>`. Run it to regenerate `assets/icon.png`,
  `assets/AppIcon.icns`, `assets/icon.ico`. Verify: `file assets/icon.ico` lists 9 icons (16, 20, 24, 32, 40, 48 as
  BMP/DIB, 64, 128, 256 as PNG); `iconutil -c iconset assets/AppIcon.icns -o <tmp>` gives the 10 iconset files;
  the sheet is written to
  `/private/tmp/claude-501/-Users-roman-Git-UsbMemoryChecker/aea22114-dfc3-4ece-9c2a-f12ffa73f126/scratchpad/icon/contact-sheet.png`
  and, looked at, meets spec scenario "Contact sheet" (adjust the small geometry by whole pixels until it does).
- [x] 13.3 Title bar mark in `src/ui/index.html`: replace the `.logo` gradient + `i-bolt` with the inline 24 px
  icon SVG (D11, same coordinates and colours as the script's 24 px drawing), drop the `--brand` token, keep `i-bolt`
  for the Speed tab. Verify: `grep -n -- '--brand' src/ui/index.html` finds nothing;
  `node scripts/ui-check/run.mjs --sizes 1180x760,880x600 --engine both --states idle --out <tmp>` exits 0 and the
  light and dark PNGs show the new mark; `node scripts/ui-check/contrast.mjs` still passes.
- [x] 13.4 Windows window icon in `src/main.rs` (D12): the `#[cfg(windows)]` block after `builder.build()` that sets
  the small (16 dip) and big (32 dip) icon from resource 1. `build.rs` stays (`set_icon` is id 1). Verify: `cargo
  clippy --all-targets --target x86_64-pc-windows-msvc -- -D warnings`, `cargo clippy --all-targets -- -D warnings`,
  `cargo fmt --check`, `cargo test` pass. Real-Windows look stays a manual check (CLAUDE.md).
- [x] 13.5 README.md and README.ru.md per D13: icon at the top, the one-screen paragraph, "Capacity at a glance",
  the Repair / remembered-drive / proposed-size text as short bullets, "Safe by design" in short sentences, "How to
  use" step 4 shortened. Verify: each of the 10 facts in D13 and the "Safe by design" facts can be found in both
  files; both files have the same sections and bullets in the same order; the image paths resolve
  (`ls assets/icon.png docs/screenshot-*.png`).
- [x] 13.6 CLAUDE.md: the "Значок" line describes the per-size drawing (`make-icon.swift` draws every size, Windows
  variant without inset, DIB ≤48, `--sheet` for the contact sheet) and that the Windows window takes its icon from
  resource 1 in `main.rs`. Verify: the documented commands run as written.
