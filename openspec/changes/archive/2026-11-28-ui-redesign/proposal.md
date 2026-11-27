# Proposal

## Why

At the default window size (1180×800) most of the app's controls are below the fold: the work area of every tab
scrolls inside the window. We measured the current window (the fix-after-repair version of the UI) in 17 states × 5
languages. 74 of the 85 combinations scroll, by up to 341 px, and in 72 of them a control or a result is partly or
fully below the fold. Hidden are, for example, the speed results (all languages), the verdict and the "Repair to …"
button after a capacity check, the "Erase and repair" button, the confirmation tick, and the "Check the repaired
drive" offer after a repair. Roman asked for a redesign where everything fits without scrolling, in a visual style
that looks ahead to 2045.

## What Changes

- **A new single-screen layout.** A compact drive rail on the left. On the right, a drive strip (name, chips, a
  capacity map bar, and Declared / Free / Real capacity readouts) and one work card with the tabs. Each tab splits into
  a "setup" column (explanation, options, the main action) and an "outcome" column (live chart, verdict, results,
  confirmation). The 150 px usage ring and the big hero header go. Their information moves into the capacity map.
- **Progress replaces the start button.** While a task runs, the progress card (stage, bar, percent, speed, time
  left, Stop) takes the place of the start button. The live chart of a full check also shows on the Real capacity
  tab, not only on the Speed tab.
- **No page scroll at the default size.** In every tab, every capacity state, every task state, all dialogs and toasts,
  and all 5 languages, nothing scrolls and every control is visible. Smaller windows (down to the minimum) never
  scroll the page. Only the work card's content, the drive list and the results table scroll inside, and nothing is
  ever cut off out of reach. Larger windows grow the chart and the text. They do not add empty bands.
- **Window size**: the default inner size becomes 1180×760 (from 1180×800), so the window fits under the menu bar on
  a 1280×800 laptop screen. The minimum becomes 880×600 (from 880×620). The layout itself fits from about 1100×700.
- **New visual language "Lumen"** (see design.md): calm neutral surfaces, depth from light (rim highlights and one
  ambient light tinted by the selected drive's state) instead of shadows and drifting blobs, one signal colour for
  actions, status colours only for meaning, always with an icon. Fluid type, tabular numbers. Light and dark follow
  the system. Everything is inline and offline, as before.
- **Accessibility**: visible focus on every control, keyboard navigation (tabs and the drive list with the arrow keys,
  focus moved into and back out of dialogs, Escape), WCAG AA contrast in both themes, `prefers-reduced-motion`
  honoured, state never shown by colour alone.
- **Drive rail shows each drive's capacity state** as a small icon with a tooltip, from the capacity view the window
  already gets.
- **Toasts can be dismissed** (click or Escape), so a long error never sits on top of a control for its whole timeout.
- **Every function, state, text and safety rule stays.** This covers what fix-after-repair adds (capacity states,
  header box, full-size tick and warnings, post-repair offer, Reload busy and slow states, new errors) and all older
  ones. No Rust behaviour changes, apart from the window size. One new window text (`btnClose`, the accessible name of
  the dialog close buttons) goes into all 5 languages.
- **A layout check tool** (`scripts/ui-check/`) renders the real page with every state in every language at a given
  size. It reports any page or pane overflow and any control outside the visible area, and saves screenshots. It runs
  in WKWebView on macOS and in headless Chromium, the engine of WebView2.
- **App icon redrawn for small sizes (round 2).** The current icon is one 1024 px drawing (dark aurora square,
  speed arc, white stick) downsampled to every size. At 16–32 px in the Windows taskbar and title bar it is a dark
  square with a blur inside. The new icon uses the Lumen language (one cyan signal colour, light rim, no aurora) and a
  bold stick-with-check silhouette, and `scripts/make-icon.swift` draws every size natively: simplified,
  pixel-snapped geometry at 48 px and below, a full-canvas Windows variant for `assets/icon.ico` (16–256) and the
  macOS grid variant for `assets/AppIcon.icns`. The title bar brand mark in the window becomes the same drawing.
  The app has no tray or menu-bar icon and gets none: "looks fine in the tray" means the taskbar, title bar and Dock.
- **Windows window icon.** tao registers its window class without an icon and sends `WM_SETICON` only when an icon
  is given, so the title bar (and possibly the taskbar button) shows the generic Windows icon instead of the one
  embedded in the .exe. `main.rs` loads the embedded icon (resource 1) for the window and the taskbar.
- **README (both languages)**: the icon at the top, one short paragraph about the new window, and the long
  paragraphs about remembered drives and the proposed repair size rewritten into short bullets with every fact kept.
- README screenshots (`docs/screenshot-capacity.png`, `docs/screenshot-speed.png`) are taken again in the new look.

## Capabilities

### New Capabilities
- `window-layout`: how the window arranges the drive list, the drive summary and the three tabs. It covers fitting
  without scrolling at the default size, behaviour when the window is smaller or larger, the default and minimum
  window size, where progress, charts, verdicts and repair confirmations appear, and the rule that no function or
  message is lost.
- `window-appearance`: how the window looks and how it can be used without a mouse. It covers light and dark themes
  following the system, contrast, state shown by icon and text and not by colour alone, focus visibility, keyboard
  navigation, dialogs and toasts, reduced motion, and offline, engine-safe styling.

### Modified Capabilities
- None. `full-capacity-check` ("Check offered after a repair", "Early stop is shown to the user") and the
  fix-after-repair capabilities (`drive-capacity-knowledge`, `drive-repair`, `drive-list`) keep their requirements
  word for word. This change only moves where those messages and controls appear.

## Impact

- Code: `src/ui/index.html` (markup, CSS and render functions are rewritten; the message handling and the safety
  logic of fix-after-repair stay as they are), `src/ui/i18n.js` (one new key in 5 languages), `src/main.rs` (window
  inner size and minimum size only).
- New dev tooling: `scripts/ui-check/` (state harness, measurement, WKWebView renderer, Chromium runner). No new crates
  and no runtime dependencies. The app ships no external fonts or assets.
- Docs: `docs/screenshot-capacity.png`, `docs/screenshot-speed.png`. `CLAUDE.md` gets the layout check command.
  README.md and README.ru.md keep their texts unless a described control changes place (both are kept in step).
- Round 2: `scripts/make-icon.swift` and `scripts/make-icon.sh` (per-size drawing, ICO writer), `assets/icon.png`,
  `assets/AppIcon.icns`, `assets/icon.ico`, the `.logo` mark in `src/ui/index.html`, a Windows-only block in
  `src/main.rs` (window and taskbar icon from the .exe resource), README.md + README.ru.md, the icon line in
  CLAUDE.md. No new crates. The version and release notes are not part of this change.
- Order: built on a branch stacked on `fix-after-repair`, after that change is final. The window texts and states of
  fix-after-repair are the input of this change.
- Windows: the WebView2 page gets the same CSS (no WKWebView-only features). The Windows compile check and CI are
  unchanged.
