# Design

## Context

The whole window is `src/ui/index.html` (HTML, CSS and JS, `include_str!`) with texts from `src/ui/i18n.js`. This
design starts from the working-tree version that fix-after-repair produces: capacity states from Rust
(`capacity[id]`: state, verified, value, partition, unused, lossReal, check, remembered, repair plan), the Real
capacity header box, the full-size tick with two warnings, the post-repair offer with its waiting logic, Reload
busy/slow, and the new error texts. All of that logic stays. The redesign replaces markup, CSS and the parts of the
render functions that place things.

The window is built in `src/main.rs`: `with_inner_size(1180, 800)` and `with_min_inner_size(880, 620)`. On macOS
the title bar is transparent with a full-size content view, so the page draws under the traffic lights (the page
pads the title bar by 86 px on the left).

### Measured problem (before)

Measured with the state harness described in D9. The real page (index.html + i18n.js) was rendered in an offscreen
WKWebView at 1180×800. Every state was injected through `window.onRustMessage`, as Rust would send it. The table shows
by how many pixels the content pane (`.content`, 724 px visible) overflows:

| State | en | ru | de | es | zh |
|---|---|---|---|---|---|
| No drives | 0 | 0 | 0 | 0 | 0 |
| Speed, idle | 108 | 128 | 128 | 128 | 108 |
| Speed, running (chart) | 210 | 230 | 230 | 230 | 210 |
| Speed, 3 results | 302 | 322 | 322 | 322 | 302 |
| Capacity, not checked | 2 | 63 | 43 | 23 | 0 |
| Capacity, quick check: fake | 240 | 301 | 301 | 280 | 179 |
| Capacity, full check: loses data (2 TB fake) | 260 | 341 | 321 | 280 | 199 |
| Capacity, full check running | 104 | 165 | 145 | 125 | 44 |
| Capacity, repaired, partition check with write error | 244 | 325 | 305 | 284 | 164 |
| Repair, not checked | 33 | 51 | 33 | 33 | 33 |
| Repair, fake, full-size tick + confirm on | 65 | 83 | 65 | 65 | 65 |
| Repair, just repaired + offer | 216 | 273 | 237 | 216 | 216 |
| Repair, limited, full-size tick on | 128 | 164 | 147 | 147 | 128 |
| Repair, running | 123 | 140 | 123 | 123 | 103 |
| Repair, keeps nothing | 0 | 0 | 0 | 0 | 0 |

What is below the fold (partly or fully), by tab:

- **Speed**: the four result tiles (fully hidden in ru/de/es even when idle), the chart's lower half while a test
  runs, and the whole results table.
- **Real capacity**: the verdict card and its "Repair to …" button in every language. In ru/de also the "Check
  capacity" button itself (the full-check description is 335 characters in German), the progress card with Stop
  during a full check, and the limited-layout note.
- **Repair**: the "Erase and repair" button in every state. With the full-size tick shown, also the confirmation
  tick. After a repair: the done message, the post-repair text and the "Check the repaired drive" button (all
  languages), and the progress card while repairing.
- **Header and rail**: always visible. The 150 px ring and the 26 px title take about 210 px of the 724.
- **Dialogs and toasts**: they are fixed overlays with their own scroll (`max-height: calc(100vh - 60px)`), so they
  are never cut off. The page behind them is.

Screenshots of every case (dark, 1180×800, 5 languages):
`/private/tmp/claude-501/-Users-roman-Git-UsbMemoryChecker/aea22114-dfc3-4ece-9c2a-f12ffa73f126/scratchpad/ui-before/`
(`<state>-<lang>-dark.png`, raw numbers in `measure.jsonl`). One real-app capture for comparison:
`.../scratchpad/ui-before/app-real-speedRunning-en.png`.

### Inventory (everything the new layout must place)

- **Title bar**: brand and tagline (`appTagline`); language select (`langSystem` + names, `languageTip`); version /
  update pill (`versionBtn`, dot when an update is available, `updatesTip`); Support pill (`donate`, `donateTip`).
  macOS: drag on empty space.
- **Drive rail**: heading `drives`; Reload (`rescanTip`, busy state, `rescanSlow` toast after 20 s, `errRescan`
  toast); list items (icon, name, size · label or `notMounted`, used bar, busy dot); `searching`; empty state
  (`noDrivesTitle`, `noDrivesText`); `sideFoot`.
- **Drive header**: name; chips (bus, `diskN` / id, fs · mount or `notMounted`); Declared (`declared`,
  `declaredHint`); Free (`available`, `availableOf`, `notMounted`); Real capacity (`real` plus the 9 state
  variants: `realUnknown`/`Hint`, `realOk`, `realFake`, `realDamaged`, `verdictBroken`, `realRepaired`,
  `realRepairedOk`, `realPartOk`, `realLimited`, `realUnconfirmed`/`Hint`); ring with `used`.
- **Tabs**: `tabSpeed`, `tabCapacity` (alert badge for fake, damaged, broken, limited, unconfirmed), `tabRepair`.
- **Speed**: `speedTitle`, `speedText`, `sizeLabel`, sizes (disabled when they do not fit, `sizeNoFit` tooltip),
  `btnStart`, progress (stage `pr*`, %, MB/s, `eta`, `btnStop`), chart (`legendWrite`, `legendRead`, `chartEmpty`,
  axis `mbps`), 4 stats, results table (`history`, `col*`, `minMax`, `badBlocks`).
- **Real capacity**: `capTitle`, `capText`, two mode cards (`modeQuick`/`Text`/`Tag`, `modeFull`/`Text`/`Tag`),
  `btnCheck`, progress, `capLimitedNote`, verdict (title and text keys for quick and full, split `realPart` /
  `fakePart`, `checkedIn`, `capRemembered`, `restoreFailures`, `fullCutShort`, `fullStoppedEarly`, `fullWriteError`,
  `errBroken`, `btnRepairTo`).
- **Repair**: `repTitle`, lead (`repText`, `repTextLimit`, `repTextLimited`, `repTextFull`, `errBroken`,
  `errUnconfirmed`); new size (`repSize`, `repSizeFrom`, `repSizeFull`); file system seg (FAT32 disabled above
  32 GB, `repFsHint`); name input (filter A–Z 0–9 _ -, `repLabelHint`); note (`repNeedCheck`, `repLimitedNote`);
  full-size tick (`repWhole`) and warning (`repWholeWarn`, `repWholeWarnLimited`); confirm tick (`repConfirm`);
  `btnRepair`; progress (no Stop); done (`repDone`); offer (`repVerifyText`, `repVerifyBtn`, `repVerifyWaiting`,
  `repVerifyNoMount`).
- **Dialogs**: Support (`donateHeading`, message, methods, `donateOffline`, spinner); updates (`updTitle` /
  `updAvailableTitle`, `updCurrent`, notes, `updWarn`, status line, `autoUpdate`, `updDev`, actions `btnSkip`,
  `btnLater`, `btnInstall`, `btnPage`, `btnAll`, `btnUpdCheck`); confirm repair (`repAskTitle`, `repAskText`,
  `btnCancel`, `btnRepair`); confirm quit (`quitTitle`, `quitText`, `btnQuit`).
- **Toasts**: all `err*`, `done*`, verdict names, `repDone`, `updateFailedToast`, `rescanSlow`, `errRescan`.

## Goals / Non-Goals

**Goals:**
- Everything above fits at 1180×760 in all states and languages (spec `window-layout`). The layout has margin:
  the mockup still fits down to about 1100×700 (at 1100×680 only the running full check in Russian overflows, by 4 px).
- One coherent, original visual language with light and dark themes, AA contrast, keyboard use and reduced motion
  (spec `window-appearance`).
- A repeatable check that proves the fit, so later text changes cannot quietly break it.

**Non-Goals:**
- No change to what Rust computes or sends, to the capacity states, to the repair plan or its safety checks, or to
  any message format.
- No remembered window size or position, no zoom setting, no theme switch inside the app (the system decides).
- No new texts besides `btnClose`. Long texts are not rewritten to fit; the layout makes room for them.
- No Windows release work (the code keeps working on Windows; see CLAUDE.md).

## Decisions

### D1. Layout: rail + drive strip + one work card with two columns

```
┌ title bar 48 ─────────────────────────────────────────────────────────────────────┐
│ [logo] UsbMemoryChecker  tagline                  [lang ▾] [↑ v0.1.2] [♥ Support]  │
├ rail 248 ────────┬ drive strip ─────────────────────────────────────────────────────┤
│ DRIVES       ⟳   │ Name                                 ┌Declared┐┌Free──┐┌Real────┐ │
│ ▣ SanDisk   ✓    │ [USB] [disk6] [exFAT · /Volumes/USB] │126 GB  ││124 GB││✕ 8.3 GB│ │
│ ▣ SD card   ⛉    │ ▮▮▯▯▯▯▯▯▯▯▯▯▯▯ capacity map          │reports ││of 126││Fake!   │ │
│ ▣ 2 TB      ✕    │ 2% used · real 8.3 GB · fake 118 GB  └────────┘└──────┘└────────┘ │
│                  ├ work card ───────────────────────────────────────────────────────┤
│                  │ [Speed] [Real capacity •] [Repair]                               │
│                  │ ┌ setup (5fr) ─────────────┐ ┌ outcome (7fr) ────────────────┐ │
│                  │ │ title / one-paragraph    │ │ live chart / verdict /        │ │
│                  │ │ options                  │ │ results / notes, ticks,       │ │
│                  │ │ [ main action | progress]│ │ Erase and repair, offer       │ │
│                  │ │ readouts                 │ │                               │ │
│ internal disks…  │ └──────────────────────────┘ └───────────────────────────────┘ │
└──────────────────┴──────────────────────────────────────────────────────────────────┘
```

- The old hero (26 px title, 3 metric tiles of 90 px, 150 px ring) took about 210 px. The new strip takes about 100 px,
  because it uses its width instead of its height: identity on the left, readouts on the right. The ring's
  information ("2% used") moves into the capacity map, which also shows real against fake at a glance.
- Two columns per tab. This makes the space a tab needs about half what the old single column needed. The setup
  column always ends with the action, so its place does not move between states. The outcome column holds what
  changes: the chart, the verdict, the confirmation. Safety order on Repair: notes and warnings sit directly above
  the ticks and the Erase button, inside one red-tinted "confirm" box.
- Alternatives considered:
  - (a) Keep one column and collapse texts behind "details". The German verdict and the post-repair explanation are
    safety texts, and hiding them is worse than placing them.
  - (b) An accordion per tab. It hides the action again.
  - (c) Tabs in the title bar. It saves 40 px, but on macOS the traffic lights and the drag zone compete for that space.
- Measured on the mockup (D10): no overflow and no hidden control in 17 states × 5 languages × 2 themes at 1180×760.
  Margin: the tallest column (Real capacity setup column, German full-check description) uses about two thirds of the work card height.

### D2. Window size: default 1180×760, minimum 880×600

- 800 px of content plus the menu bar does not fit a 1280×800 laptop screen. 760 leaves room for the menu bar
  (24–37 px). On Windows 1366×768 laptops nothing below about 728 fits anyway. There the user resizes, and the
  smaller-window behaviour (D7) covers it.
- The width stays 1180, because two columns need about 1100.
- The minimum height goes down from 620 to 600, because the page now degrades gracefully. The minimum width stays 880.
- Alternatives:
  - Keep 1180×800: the layout fits there too, but it does not fit the small laptop screen.
  - 1100×700: it fits, but without margin for longer texts in the future.
- This is the only Rust change: two numbers in `src/main.rs`.

### D3. Visual language "Lumen" (the 2045 forecast)

Forecast, and how it shapes the design:

1. **Calm, ambient software.** By the 2040s most screens are glanceable surfaces among many (wall, glasses, car). Tools
   earn attention only when something matters. → Neutral, low-chroma surfaces. Colour is reserved for meaning
   (genuine, fake, warning, action). One question gets one big answer per drive (the Real capacity readout).
2. **Depth through light, not shadow.** Spatial and AR interfaces light their layers. They do not cast drop shadows
   onto a desk that is not there. → Surfaces are separated by a 1 px rim light on top (`inset 0 1px 0 rim`), a
   hairline edge and a slightly lighter tone per elevation step. No blurred drop shadows.
3. **Adaptive material and ambient state.** Materials react to context. → One static "ambient light" at the top of
   the window takes on a faint tint of the selected drive's state (neutral, green, amber, red), so the state can be
   read from across the room. It is static and never pulses.
4. **Glanceable density.** Dashboards replace pages. Tabular numerals, data-ink over decoration, and one
   visualisation per fact (the capacity map, the split bar, the live chart).
5. **Fluid, variable type.** Variable system fonts (SF Pro, Segoe UI Variable) with intermediate weights (450, 560,
   620, 680) and a size that scales with the window.
6. **Accessible by default.** High contrast is the norm in 2045 regulation and in OLED-dark use. AA contrast in both
   themes, state with icon and text, visible focus, motion only as feedback.
7. **Honest motion.** Things move because something happened (press, state change, progress), never to decorate.
   The drifting aurora blobs go.

Tokens (CSS custom properties on `:root`; dark is the default, light comes from `@media (prefers-color-scheme:
light)`. Measured ratios are against `--s1`):

| Token | Dark | Light | Use |
|---|---|---|---|
| `--bg` | #090B10 | #EEF1F5 | window background |
| `--s1` | #10131A | #FFFFFF | cards (rail, strip, work) |
| `--s2` | #161A23 | #F6F7FA | inset tiles, inputs, seg tracks |
| `--s3` | #1D2230 | #ECEFF4 | selected item, raised control |
| `--line` | #2A3140 | #D9DEE7 | hairline edges |
| `--edge` | #737D92 | #7F889A | control borders (≥3:1) |
| `--rim` | rgba(255,255,255,.055) | rgba(255,255,255,.9) | top rim light |
| `--text` | #EDF0F6 (16.3) | #0E1117 (18.9) | primary text |
| `--text-2` | #AEB5C4 (9.0) | #434C5E (8.6) | secondary text |
| `--text-3` | #8C94A6 (6.1) | #5A6376 (6.0) | hints, labels |
| `--signal` | #5EE2E8 (12.0) | #00737A (5.6) | actions, focus, write series |
| `--signal-ink` | #04161A (11.9 on signal) | #FFFFFF (5.6 on signal) | text on signal |
| `--violet` | #AFA2FF (8.4) | #5640D0 (6.9) | read series |
| `--ok` | #4ADE9A (10.8) | #0A7A45 (~5.3) | genuine, repaired |
| `--warn` | #F5B54A (10.3) | #9A5800 (5.6) | limited, damaged, unconfirmed |
| `--bad` | #FF7380 (7.1) | #C0263A (5.9) | fake, keeps nothing, danger |
| `--bad-ink` | #1C0508 (7.5 on bad) | #FFFFFF (5.9 on bad) | text on danger button |
| `--brand` | removed in round 2 (D11) | | the logo is the icon SVG |

- **Type**: system stack as now. `--fs-base: clamp(13px, 0.55vw + 6.5px, 15px)`, which is 13 px at 1180 wide and 15 px
  at 1550 and above. Derived sizes: xs = base − 2.5, s = base − 1.5, m = base + 2, l = base + 7, xl = base + 13.
  Weights 450 for body, 560–620 for UI and labels, 680–720 for numbers and titles. `font-variant-numeric:
  tabular-nums` on every number. Labels are uppercase with letter-spacing .08–.1em, at xs size.
- **Spacing**: a 4 px scale (4, 8, 12, 16, 20). Card padding 14–16. Column gap 20. Gap between cards 12.
- **Shapes**: radii 20 (cards), 14 (tiles, verdict, notes), 10 (buttons, inputs, seg), 7 (seg items, small). Nested
  radius = outer − padding, so the corners stay concentric. Pills are 999.
- **Materials**: cards are `--s1` with a rim and a hairline. Tiles are `--s2` with a hairline. A selected item is
  `--s3` with a strong rim. State tints use `color-mix(in srgb, <state> 5–16%, --s1)` with a 35–55% state edge.
  There is no `backdrop-filter`: the content no longer floats over moving blobs, and dropping it saves GPU work in
  WebView2.
- **Buttons**: the primary is solid `--signal` with `--signal-ink`. Danger is solid `--bad` with `--bad-ink`. Secondary
  is `--s3`. Disabled is 40% opacity, never by colour only. Each tab has one full-width action in its setup column.
- **Iconography**: the existing 24 px line set, stroke 1.75 (2.2 for check and cross), round caps and joins, with
  `currentColor`. A new `i-dash` is used for "not checked". Every state icon has a text next to it or as its accessible
  name.
- **Motion**: `--t-fast` 120 ms (hover, press), `--t-med` 200 ms (state colours), `--t-slow` 320 ms (panel fade, the
  ambient tint). Easing is `cubic-bezier(.2,.7,.2,1)`. The progress bar moves 300 ms linear. The spinner runs only
  while busy. With `prefers-reduced-motion: reduce`, every animation and transition is off and the spinner becomes a
  static ring with two arcs.
- **Focus**: `:focus { outline: 2px solid var(--signal); outline-offset: 2px }` with `:focus:not(:focus-visible) { outline: none }` (an engine without `:focus-visible` keeps the plain ring). Inputs show an inset 2 px
  signal ring.
- Engine safety: custom properties, `clamp()`, `color-mix(in srgb …)` (already used today; macOS 11 WKWebView lacks it before Safari 16.2, so surfaces that need it have a solid fallback declared first, e.g. the dialog backdrop), grid, `:focus-visible` (Safari 15.4, with the `:focus` fallback above),
  `prefers-color-scheme`, `prefers-reduced-motion`. No `@container`, no `:has()`, no `oklch()`, no view transitions,
  no `backdrop-filter`. This keeps the page working on older macOS WKWebView builds as well as on WebView2.

### D4. Drive strip and capacity map

- Left side: name (`--fs-l`), chips (bus, disk id, fs · mount; under 1100 px only the first two), map bar (8 px),
  legend.
- Map: the bar is the declared size. The `used` fill is `(total − free) / declared`. An overlay starts at the real
  size: a red hatch when the state is fake, damaged, broken or repaired (repaired: the space past the partition is
  what the fake drive would lose), an amber hatch when the state is limited. Legend: `{pct}% used` and, when known,
  `realPart` / `fakePart` (existing keys). Input: `capacity[id]` (value, partition, declared). No new Rust data.
- Right side: three readouts, 120–210 px wide each, with value (`--fs-l`) and hint (xs, ellipsis, `title` with the
  full hint). The Real capacity readout keeps today's `setReal` mapping and adds an icon (check, cross, alert, dash)
  per class.

### D5. Tab contents

- **Speed**. Setup: title, `speedText`, size seg, Start (full width) or progress, 2×2 readouts. Outcome: chart (flex,
  at least 160 px), then the results table (sticky header, at most about 132 px, so 3 rows; more rows scroll inside).
- **Real capacity**. Setup: title, `capText` (shown in the outcome placeholder instead when nothing is checked yet, so it
  is not shown twice), two radio cards (title + tag; the card's `title` is its long description), the selected mode's
  description, Check or progress. Outcome, in priority order: the live chart while a full check of this drive runs;
  else the limited note (`capLimitedNote`) and the verdict; else a quiet placeholder.
- **Repair**. Setup: title, lead, new size (`--fs-xl`) with hint, file system seg and name side by side, the combined
  hint (`repFsHint` + `repLabelHint`). Outcome: done + offer card (when `r.repaired && r.offer`), the note (need-check,
  limited), then the confirm box: full-size tick, its warning, confirm tick, Erase and repair. While repairing, the
  confirm box is replaced by the progress card.
- The `renderRepair`, `repairPlan`, `askRepair`, `doRepair`, `renderRepairOffer`, `verifyRepaired`, `taskDone` and
  `onRustMessage` logic of fix-after-repair is kept line for line. Only the element ids it writes to may move inside
  the new markup. Keep the ids (`repWhole`, `repConfirm`, `repBtn`, `repVerifyBtn`, …), so the diff of the logic stays
  empty.

### D6. One chart component, two places

`drawChart()` takes the canvas to draw. The Speed tab has `#chart`, the Real capacity tab has `#chartCap`. On every
`point` or `rescale` message the visible canvas is drawn: Speed when `tab === 'speed'`, the capacity one when
`tab === 'capacity'` and a full check of the selected drive runs. `series` stays one object, so the Speed tab still
shows the last full check's curves as today (the spec "Chart after an early stop" is unchanged). Colours come from
the tokens (`--signal`, `--violet`, `--line`, `--text-3`).

### D7. Resizing

- At 1100 px and wider: two columns. Below 1100 px: one column in the order setup, outcome. Below 980 px: the tagline
  is hidden and the rail is 216 px.
- The work card's `.panel` is the only pane that may scroll (besides the drive list and the results table). The title
  bar, the rail and the strip never scroll. The page has `overflow: hidden` and fits `100vh` by grid
  (`grid-template-rows: 48px minmax(0,1fr)`, `min-height: 0` on every grid child).
- The single-column case needs `grid-auto-rows: max-content` on the panel and `min-height: auto` on the columns.
  Otherwise the columns shrink and overlap. The mockup showed this at 900×600 before the fix.
- Above the default size: the chart and the outcome column grow (`5fr / 7fr`), and type grows to 15 px.

### D8. Accessibility implementation

- ARIA: `role=tablist/tab/tabpanel` with `aria-selected` and `aria-controls`; the drive list is a `listbox` of
  `option` buttons with roving `tabindex`; the seg controls and mode cards are `radiogroup` / `radio` with
  `aria-checked`; progress has `role=status` (stage text plus `aria-valuenow` on the bar); toasts have `role=alert`
  (errors) or `status`. Dialogs have `role=dialog aria-modal=true aria-labelledby`.
- Keyboard handlers: one `keydown` on the tab list (Left, Right, Home, End), one on the drive list (Up, Down, Home,
  End), one per radiogroup. Escape closes the top dialog or else the newest toast. Dialogs: remember
  `document.activeElement`, focus the first safe control (Cancel in the repair and quit dialogs), trap Tab, restore
  focus on close.
- `btnClose` (new key) is the `aria-label` of every ✕ close button. en "Close", ru "Закрыть", es "Cerrar",
  de "Schließen", zh "关闭".

### D9. Layout check tool (`scripts/ui-check/`)

These are the same pieces that produced the measurements above and the mockup checks, moved into the repo:

- `harness.js`: builds fake `init`, `update`, `disks` (with `capacity` views), `task`, `point` and `taskDone` messages
  for a named scenario and feeds them to the page's `window.onRustMessage`. Real messages and `send` are disabled while
  it runs. It resets the window globals between scenarios. Scenarios: `nodrives`, `searching`, `idle`, `speedRun`,
  `speedDone`, `capUnknown`, `capGenuine`, `capFake`, `capDamaged`, `capBroken`, `capFull2tb`, `capRun`,
  `capPartOk`, `capPartWriteError` (repaired + cut short), `capLimited`, `capUnconfirmed`, `repUnknown`,
  `repGenuine`, `repFake`, `repFakeWhole`, `repRepaired`, `repRepairedOffer`, `repOfferWaiting`,
  `repOfferNoMount`, `repLimited`, `repLimitedWhole`, `repRun`, `repBroken`, `repUnconfirmed`, plus the overlays
  `modal=donate|update|repair|quit` and `toast`.
- `measure.js`: returns page overflow, overflowing panes (minus the allowed ones), and visible controls outside the
  visible area of their pane or of the window.
- `render.swift`: an offscreen WKWebView (AppKit, no window shown, no dock icon) that loads the composed page at a
  given size and appearance, runs the scenario, prints the measurement JSON and saves a PNG. Run it with `swift
  scripts/ui-check/render.swift <jobs.json>` or compile it into the scratch folder.
- `run.mjs`: composes `index.html` + `i18n.js` into a temp file (as `main.rs::page()` does), writes the jobs (states ×
  languages × themes × sizes), runs WKWebView (macOS) and headless Chrome or Edge with its own `--user-data-dir` in a
  temp folder (Chromium = the WebView2 engine). For Chromium, the same harness loops all scenarios in one page and
  writes the report into the DOM, read back with `--dump-dom`. It exits non-zero on any failure. Default sizes:
  1180×760 (must be clean) and 880×600 (page must not scroll; controls only inside the scrollable panel).
- `contrast.mjs`: parses the tokens from `index.html` (both themes) and checks the ratios required by the spec.
- The harness touches window internals (`results`, `task`, `series`, `selectDrive`, `showTab`), so it lives next to
  the UI and is updated with it. It never sends anything to Rust and never starts a task.
- In-app spot check (manual, as for the README screenshots): `UMC_DEV_SCRIPT="$(cat harness.js)…"` with a scratch
  `HOME`, `UMC_SHOW_VIRTUAL=1`, then `screencapture -l <window id>`.

### D10. Mockup

A standalone prototype with the real texts of all 5 languages (it inlines `i18n.js`). It uses the same tokens,
layout and states, outside the repo:
`/private/tmp/claude-501/-Users-roman-Git-UsbMemoryChecker/aea22114-dfc3-4ece-9c2a-f12ffa73f126/scratchpad/ui-redesign/mockup.html`
(open with `?s=<state>&lang=<xx>`; source `mockup-src.html`). Renders at 1180×760 (WKWebView, 2×):

- `.../scratchpad/ui-redesign/01-idle-en-dark.png`, `01-idle-en-light.png`: idle, drive selected (not checked)
- `.../scratchpad/ui-redesign/02-speed-running-en-dark.png`, `02-speed-running-en-light.png`: speed test running, live chart, progress in place of Start
- `.../scratchpad/ui-redesign/03-capacity-fake-en-dark.png`, `03-capacity-fake-en-light.png`: quick check found a fake, split bar, Repair to
- `.../scratchpad/ui-redesign/04-repair-repaired-offer-en-dark.png`, `04-repair-repaired-offer-en-light.png`: repaired drive with the post-repair offer
- `.../scratchpad/ui-redesign/05-capacity-partition-verified-de-dark.png`: longest texts (German)
- `.../scratchpad/ui-redesign/06-repair-full-size-warning-ru-light.png`: limited drive, full-size tick on, warning
- `.../scratchpad/ui-redesign/07-repair-confirm-dialog-en-dark.png`: confirmation dialog
- `.../scratchpad/ui-redesign/08-no-drives-en-light.png`: no drives
- `.../scratchpad/ui-redesign/09-speed-results-zh-dark.png`: speed results with history (Chinese)
- `.../scratchpad/ui-redesign/10-min-size-900x600-ru-dark.png`: below the default size, one column, inner scroll
- `.../scratchpad/ui-redesign/chromium-capPart-de-light-1180x760.png`, `chromium-repOffer-de-light-1180x760.png`:
  the same in headless Chromium
- All 170 checked renders (17 states × 5 languages × 2 themes) and the 900×600 ones:
  `.../scratchpad/ui-redesign/check/` (`jm1.jsonl` and `jm2.jsonl` hold the measurements: 0 problems at 1180×760; at
  900×600 no page scroll, only the work card scrolls).

### D11. App icon: Lumen, drawn per size (round 2)

Problem: `make-icon.swift` draws one 1024 px master (100 px macOS inset, drop shadow, dark aurora square, rainbow
speed arc, white stick with a green check) and `make-icon.sh` shrinks it with `sips` for the .icns and for the .ico
(16–256). At 16–32 px the 100 px inset, the shadow and the arc eat the canvas: the Windows taskbar and title bar show
a small dark square with a blur in it, and it disappears on a dark taskbar.

Motif (original artwork, kept from the current icon so the app is still recognisable): a USB stick, plug up, with a
check mark on its body, on a rounded dark tile. Colours are fixed (not theme tokens):

| Part | Colour |
|---|---|
| tile | vertical gradient #1A1F2B → #0C0F15 (≥24 px), flat #12161F (16, 20 px) |
| tile edge | 1 px inner stroke rgba(255,255,255,.16), all round (keeps the tile visible on a dark taskbar) |
| rim (≥24 px) | 1 px top highlight rgba(255,255,255,.10), Lumen "light, not shadow" |
| plug | #EDF0F6 (`--text` dark); at ≥24 px two contact holes in the tile colour |
| body | #5EE2E8 (`--signal` dark): the one colour, it carries the silhouette on any background |
| check | knocked out of the body in #0C0F15 (≈12:1 on the body) |
| arc (≥64 px only) | track rgba(255,255,255,.10), segment #5EE2E8 at 70%, behind the stick, one colour, no gradient |

Readability on both taskbars: on a light one (#F3F3F3, white) the dark tile is the shape; on a dark one (#202020,
black) the cyan body plus the light edge are the shape. No aurora blobs, no rainbow, no green check.

Geometry:
- **Small (≤48 px: 16, 20, 24, 32, 40, 48)** on a 16-unit grid (y down), scaled by n/16, then every rectangle
  edge rounded to a whole pixel and every stroke to an integer width (`max(2, round(n/8))` for the check), with
  odd-width strokes on pixel centres. Reference at 16 px: tile 0..16 (radius 3.5), plug x 6–10 y 2–6, body x 4–12
  y 6–14 (radius 1.5), check (5.5,10) → (7.5,12) → (10.5,8.5). No arc, no shadow, no gradient below 24. The
  executor may move a part by 1 px after looking at the contact sheet; the rule is "every edge on a pixel".
- **Large (≥64 px)**: the same parts in 1024 units with the arc added and the stick at about 60% of the tile
  height, scaled smoothly (no snapping needed).
- **Windows variant `win-<n>`** (ICO): the tile fills the canvas (inset `round(n/64)`, i.e. 0 px at 16–48), radius
  22% of n, no shadow.
- **macOS variant `mac-<n>`** (icns): the Big Sur grid as now (tile 824/1024, inset `round(n·100/1024)`, radius
  185/1024); the drop shadow (offset −12, blur 28, 35%, scaled) only at ≥128; 16 and 32 use the small geometry
  inside the inset tile. The content is drawn in tile-relative units, so both variants share one drawing function.

Script interface:
- `swift scripts/make-icon.swift <outdir>` draws natively (no downsampling) `mac-16/32/64/128/256/512/1024.png`,
  `win-16/20/24/32/40/48/64/128/256.png` and writes `<outdir>/icon.ico` itself: 9 entries, ≤48 px as 32-bit BGRA
  DIB (BITMAPINFOHEADER, height 2n, bottom-up, un-premultiplied alpha, 1-bit AND mask set where alpha is 0), ≥64 px
  as PNG. DIB for the small entries is the form every Windows version and `rc.exe` take; PNG entries are only
  documented as safe at 256, so the ≥64 PNG entries keep the file small without risk.
- `swift scripts/make-icon.swift --sheet <icon.ico> <AppIcon.icns> <out.png>` reads the **packed** files back with
  ImageIO (`CGImageSourceCreateWithURL`, every image index) and draws each image at 1× (one icon pixel = one sheet
  pixel) on four backgrounds: white, #F3F3F3 (Windows 11 light taskbar), #202020 (Windows 11 dark taskbar), black,
  with a size label; plus a row with win-16, win-24 and win-32 zoomed 4× nearest-neighbour to judge the pixel
  snapping. Output for this change:
  `/private/tmp/claude-501/-Users-roman-Git-UsbMemoryChecker/aea22114-dfc3-4ece-9c2a-f12ffa73f126/scratchpad/icon/contact-sheet.png`.
- `scripts/make-icon.sh` runs the drawing into a temp folder, builds the iconset from `mac-*`
  (`icon_16x16` = mac-16, `icon_16x16@2x` = mac-32, `icon_32x32` = mac-32, `icon_32x32@2x` = mac-64, `icon_128x128` =
  mac-128, `@2x` = mac-256, `icon_256x256` = mac-256, `@2x` = mac-512, `icon_512x512` = mac-512, `@2x` = mac-1024),
  copies mac-1024 to `assets/icon.png` and `icon.ico` to `assets/icon.ico`. No `sips` resizing and no Python left.
  An optional `--sheet <out.png>` runs the sheet mode at the end.

Title bar mark: `.logo` in `index.html` becomes an inline SVG (`viewBox="0 0 24 24"`, 24 CSS px) with exactly the
coordinates and colours of the 24 px small drawing (tile, edge, rim, plug, holes, body, check), fixed colours in both
themes, like the icon itself on a light taskbar. The `--brand` gradient token goes (it was used only by the logo);
`i-bolt` stays for the Speed tab.

### D12. Window icon on Windows; Dock icon of an unbundled binary (round 2)

- tao 0.37.3 registers the window class with `hIcon` and `hIconSm` empty (`platform_impl/windows/window.rs`,
  `event_loop.rs`) and sends `WM_SETICON` only from `set_window_icon` (small) and `set_taskbar_icon` (big). The .exe
  icon that `build.rs` embeds with winresource (`set_icon` = resource id 1) is then shown by Explorer for the file,
  but the title bar shows the generic window icon, and the taskbar button and Alt+Tab may too. Fix, after
  `builder.build()` in `main.rs`:

  ```rust
  #[cfg(windows)]
  {
      use tao::platform::windows::{IconExtWindows, WindowExtWindows};
      let px = |dip: f64| {
          let p = (dip * window.scale_factor()).round() as u32;
          Some(tao::dpi::PhysicalSize::new(p, p))
      };
      window.set_window_icon(tao::window::Icon::from_resource(1, px(16.0)).ok());
      window.set_taskbar_icon(tao::window::Icon::from_resource(1, px(32.0)).ok());
  }
  ```

  `LoadImageW` picks the nearest ICO entry, which is why the ICO has 20, 24, 40 and 48 for 125–300% scaling. A
  missing resource (a cross-build from the Mac, where `build.rs` skips winresource) gives `None`, which is the old
  behaviour. No new crate. A DPI change while running keeps the first size (Windows scales it); acceptable.
- macOS: the released `.app` has `CFBundleIconFile` = AppIcon, so the Dock shows the icon. The bare
  `target/release/usbmemorychecker` binary shows the generic exec icon in the Dock. Setting
  `NSApp.applicationIconImage` at run time would need objc code and a second embedded image for a developer-only
  case, so it is **not done**. To see the icon in the Dock, use `scripts/build-macos.sh` and open
  `dist/UsbMemoryChecker.app`.
- There is no tray or menu-bar icon in the app and none is added.

### D13. README (round 2)

Both files change the same way (README.md English, README.ru.md Russian):
- The icon at the top: `<img src="assets/icon.png" width="96" alt="">` (README.ru the same) before the title line.
- One short paragraph under the first screenshot or in "How to use": everything is on one screen: the drives on the
  left, the selected drive's capacity map (used, free, fake part; declared, free and real size) on top, below it the
  Speed, Real capacity and Repair tabs, each with the settings on the left and the result on the right; light and
  dark follow the system. The "Shows free and total space" bullet becomes "Capacity at a glance" with the map.
- The Repair bullet is split into short bullets. Every fact below stays (check off each one in the review):
  1. one partition of the real size, exFAT or FAT32, fake part unused; the drive then shows e.g. 31.9 GB and stops
     losing files;
  2. after a repair the window offers the full check of the repaired drive;
  3. for a repaired drive the last check result and the repair are saved in `capacity.json` next to the settings,
     so it still shows "Repaired" after a restart or replug;
  4. a drive that was only checked, not repaired, is not remembered after a replug or restart;
  5. a repaired drive is recognised by its declared size and partition layout;
  6. a drive whose partition ends far before its declared size is shown as limited even without a saved result;
  7. proposed repair size: the known real size (from a check), else the current partition's size; the full declared
     size only with the "Use the full declared size" tick;
  8. the full declared size is proposed by default only for a drive that a whole-device check found genuine;
  9. a drive not checked yet gets no proposed size (run the quick check or tick the full size);
  10. the repair erases the drive.
  The "Safe by design" bullet keeps its facts (removable only; the elevated part re-checks id, size and layout; the
  `dd`/ISO "limited" case; a small macOS GPT stick may show "partition verified"; a quick check settles both;
  "Unconfirmed" when the knowledge no longer matches the partitions or the layout cannot be read, with a quick check
  asked for before a repair size) but as 3–4 short sentences. "How to use" step 4 keeps the action and the erase
  warning and refers to the Repair bullets for the size rules instead of repeating them.
- No version, release notes or install changes here.

## Risks / Trade-offs

- [Texts grow later and break the fit] → `scripts/ui-check` runs every state × language at 1180×760 and fails. The
  layout has room to spare: the worst case uses about two thirds of the work card height at 760.
- [Harness drift: it depends on window internals] → It lives in `scripts/ui-check/` next to the UI. Task 1 builds it
  against the current page and task 7 against the new one. A scenario that cannot be built fails the run.
- [Behaviour regressions while moving markup] → The logic functions keep their element ids and code (D5). Group 6
  compares the sent IPC messages of the old and new page for the same scripted clicks.
- [WebView2 differences] → Only features listed in D3. Headless Chromium runs the same check. CI keeps compiling for
  Windows. The page was never run on real Windows (CLAUDE.md), so a manual look on Windows stays open.
- [Lower default height on big screens] → 760 is still fine on a 1512×982 MacBook Pro. The window can be enlarged, and
  everything grows with it.
- [Losing information from the removed ring] → The used percentage moves into the map legend; the `used` key stays
  in use.
- [Icon never seen on real Windows] → The ICO is read back with ImageIO for the contact sheet, `file` lists the 9
  entries, Windows clippy and CI build compile the icon code. The look in a real taskbar stays a manual check, as for
  the rest of the Windows build.
- [Hidden description of the non-selected check mode] → It is still available as the card's tooltip and accessible
  description. Choosing the mode shows it in full before the user starts.

## Migration Plan

- Branch `ui-redesign` from `fix-after-repair` once that change is final. If fix-after-repair is merged first, start
  from `main`.
- Order: tooling against the old page (proves the harness and records the "before"), then the new tokens and
  layout, then the logic moved under it, accessibility, the window size, the docs and the screenshots.
- Rollback: one commit range in `index.html`, `i18n.js` (one key) and two numbers in `main.rs`. No data or IPC change,
  so an older build works with the same settings and `capacity.json`.

## Open Questions

- With the full-size tick on, the Repair lead falls back to `repTextFull` ("No capacity problem known: the drive
  will be formatted at its full size"), which reads oddly for a known fake. This is existing fix-after-repair
  behaviour that the redesign keeps as is. If it should change, it belongs in fix-after-repair or a later change.
