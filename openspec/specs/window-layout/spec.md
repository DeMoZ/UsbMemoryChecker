# window-layout Specification

## Purpose
Defines how the application window arranges the drive list, the selected drive's summary and the Speed, Real
capacity and Repair tabs, so that at the default window size every function and message is visible without
scrolling. Also defines how the window behaves when it is resized.

## Requirements

### Requirement: Default and minimum window size
The window SHALL open with an inner size of 1180×760 logical pixels and SHALL NOT be resizable below 880×600.

#### Scenario: First start
- **WHEN** the application starts
- **THEN** the window's content area is 1180×760 logical pixels on macOS and on Windows

#### Scenario: Resizing down
- **WHEN** the user drags the window smaller
- **THEN** it stops at 880×600

### Requirement: Nothing scrolls at the default size
At an inner size of 1180×760 or larger, the page SHALL NOT scroll, no pane except the drive list and the speed
results table SHALL scroll, and every visible control SHALL lie fully within the window, in every tab, display state
and language.

#### Scenario: Every display state
- **WHEN** the window shows any capacity state (not checked, genuine, fake, damaged, keeps nothing, repaired, repaired
  and verified, partition verified, limited, unconfirmed), any task state (idle, speed test, quick check, full check,
  repair running), the post-repair offer in any of its states, the full-size tick on or off, any dialog, or a toast,
  in en, ru, es, de or zh
- **THEN** nothing scrolls and every control is visible

#### Scenario: Layout check at the default size
- **WHEN** the layout check (`scripts/ui-check`) renders every state of the list above in all 5 languages at
  1180×760, in WKWebView and in headless Chromium
- **THEN** it reports for every case that `document.scrollingElement.scrollHeight` ≤ `clientHeight`, that no element
  other than the drive list and the results table has `scrollHeight` > `clientHeight`, and that no visible button,
  input, select, tick, verdict, progress card or offer ends below the visible area of the window

#### Scenario: Longest texts
- **WHEN** the selected drive is repaired, the full check of its partition passed with a write error, and the language
  is German or Russian
- **THEN** the limited-layout note, the verdict with its partition, write-error and date sentences, and the
  "Check capacity" button are all visible at once on the Real capacity tab

#### Scenario: Repair tab after a repair
- **WHEN** a repair has just finished, so the done message, the "Check the repaired drive" offer and button, the
  "use the full declared size" tick, the confirmation tick and the "Erase and repair" button are all shown
- **THEN** all of them are visible together without scrolling, in every language

#### Scenario: Speed results
- **WHEN** a speed test has finished and the results table has runs
- **THEN** the four result readouts (write average and max, read average and max), the chart and at least the three
  newest runs are visible without scrolling

### Requirement: Smaller windows never scroll the page
Between the minimum size and the default size, the page itself SHALL NOT scroll and nothing SHALL be cut off out of
reach. When the content of a tab does not fit, only the work card's content SHALL scroll inside the card, with the
tabs, the drive strip, the drive list and the title bar staying in place. Below about 1100 px width the two columns
of a tab SHALL become one column in the same order (setup first, then the outcome).

#### Scenario: Minimum size
- **WHEN** the layout check renders every state in all languages at 880×600
- **THEN** the page never scrolls, and every control is either visible or inside the work card's scrollable content

#### Scenario: Narrow window
- **WHEN** the window is 900 px wide
- **THEN** each tab shows one column, the setup part first, and the outcome part below it in the same scrollable card

### Requirement: Larger windows use the space
Above the default size, the chart and the outcome column SHALL grow with the window, and text SHALL scale fluidly
from the base size at 1180 px width up to at most 15 px base size. Above the default size the window SHALL NOT add
scrolling or leave controls stretched across the whole width.

#### Scenario: Large window
- **WHEN** the window is 1600×1000
- **THEN** nothing scrolls, the chart is taller than at the default size, and buttons keep their natural width
  except the primary action of each tab, which fills only its own column

### Requirement: Single-screen arrangement
The window SHALL show a title bar, a drive rail on the left, and for the selected drive a drive strip above one
work card that holds the tabs and the selected tab's content.

#### Scenario: Title bar and rail
- **WHEN** the window is open
- **THEN** the title bar shows the app name, tagline, language choice, version and update button and Support button,
  and the rail shows its heading with Reload, the drive list (or the empty or searching state) and the note that
  internal disks are never shown

#### Scenario: Drive strip contents
- **WHEN** a drive is selected
- **THEN** the drive strip shows its name, its bus, disk id and volume (file system and mount point, or "not
  mounted"), a capacity map, and the Declared, Free and Real capacity readouts

#### Scenario: A drive is selected
- **WHEN** a drive is selected
- **THEN** the drive strip shows its name, chips, the capacity map with the used percentage, and the three readouts,
  and the work card shows the tabs with the Real capacity tab's alert badge as defined by the capacity state

#### Scenario: No drive
- **WHEN** no removable drive is connected
- **THEN** the drive rail shows the "insert a flash drive" empty state, and the work area shows the "choose a drive"
  placeholder, without a drive strip

#### Scenario: Searching
- **WHEN** the first drive scan has not finished yet
- **THEN** the drive rail shows the searching state

### Requirement: Capacity map
The drive strip SHALL show a bar that stands for the declared size, with the used space of the volume marked. When
the real capacity is known to be smaller (fake, damaged, keeps nothing, repaired, limited), the part beyond the real
or partition size SHALL be marked as lost (fake, damaged, keeps nothing, repaired) or unused (limited). A legend SHALL
give the used percentage and, when known, the real part and the fake part in the selected language.

#### Scenario: Fake drive
- **WHEN** the selected drive is fake with a real size of 8.3 GB of 126 GB declared
- **THEN** the map marks about 7% as real and the rest as lost, and the legend reads "2% used · real 8.3 GB · fake
  118 GB" (in the selected language)

#### Scenario: Not checked
- **WHEN** the capacity state is not checked
- **THEN** the map shows only the used part and the legend only the used percentage

### Requirement: Real capacity readout
The Real capacity readout SHALL show exactly the value and hint the capacity state defines (fix-after-repair:
"Header real-capacity box"), with the state's colour and a matching icon. A hint that is cut off for lack of space
SHALL show its full text as a tooltip.

#### Scenario: Limited drive
- **WHEN** the state is limited with an 8.3 GB partition on a 125.8 GB drive
- **THEN** the readout shows 8.3 GB in the warning colour with a warning icon, and the full "only this partition is
  used, not checked, declared 126 GB" hint is available as a tooltip and is not lost

### Requirement: Drive rail shows the capacity state
Each drive in the rail SHALL show a small icon for its capacity state (genuine, fake or keeps nothing, damaged, limited
or unconfirmed, repaired or partition verified, not checked), with the state's name as its tooltip and accessible
name. A drive with a running task SHALL show the busy mark instead.

#### Scenario: Several drives
- **WHEN** the rail lists a genuine card, a fake drive and an unchecked drive
- **THEN** each shows its own state icon, and the fake one's tooltip says "Fake drive!" in the selected language

### Requirement: Tab layout: setup and outcome
Each tab SHALL show its setup (title, explanation, options, the main action button) in the first column and its
outcome in the second column.

#### Scenario: Speed tab
- **WHEN** the Speed tab is open
- **THEN** the setup column has the test size, the Start button and the four result readouts, and the outcome column
  has the live chart and the results table

#### Scenario: Real capacity tab
- **WHEN** the Real capacity tab is open
- **THEN** the setup column has the Quick and Full check choice, the description of the selected check and the Check
  button, and the outcome column has the limited-layout note and the verdict, or the live chart while a full check
  runs

#### Scenario: Repair tab
- **WHEN** the Repair tab is open
- **THEN** the setup column has the explanation, the new size with its "instead of the declared" hint, the file system
  and the name, and the outcome column has the notes, the done message and the post-repair offer, the full-size tick
  with its warning, the confirmation tick and the Erase and repair button

#### Scenario: Description of the non-selected check
- **WHEN** Quick check is selected
- **THEN** the Quick check description is shown below the choice, and the Full check description is available as the
  tooltip of the Full check card

#### Scenario: Verdict actions
- **WHEN** the verdict is fake or damaged with a repair size above 0
- **THEN** the verdict has the "Repair to <size>" button, which opens the Repair tab

### Requirement: Progress in place of the action
While a task runs on the selected drive, the progress card (stage text, progress bar, percent, speed, time left, and
Stop for every task except the repair) SHALL take the place of that tab's main action button. The options of that
tab SHALL be disabled. While a full check runs, the Real capacity tab's outcome column SHALL show the live write and
read chart, the same series the Speed tab chart shows.

#### Scenario: Speed test running
- **WHEN** a speed test runs on the selected drive
- **THEN** the Start button is replaced by the progress card with Stop, the test sizes are disabled, and the chart in
  the outcome column draws each new point

#### Scenario: Full check running
- **WHEN** a full check runs on the selected drive
- **THEN** the Check button is replaced by the progress card with Stop, and the outcome column shows the live chart.
  After the check, the verdict takes its place and the Speed tab chart still shows the series, as before

#### Scenario: Repair running
- **WHEN** a repair runs
- **THEN** the Erase and repair button and the ticks are replaced by the progress card without Stop

#### Scenario: Task on another drive
- **WHEN** a task runs on a drive that is not selected
- **THEN** the selected drive's action buttons are disabled, no progress card is shown for it, and the drive with
  the task shows the busy mark in the rail

### Requirement: No function or message is lost
The redesigned window SHALL keep every control, state, text and safety rule of the window it replaces (the
fix-after-repair version), with the same conditions for showing, enabling and sending, and SHALL send the same
messages to the application. The list of what must be kept is the inventory in design.md.

#### Scenario: Title bar, rail and dialogs kept
- **WHEN** the redesigned window is compared with the inventory
- **THEN** the language choice, the update button, dialog and offer, the Support dialog (methods, offline note,
  spinner), Reload with its busy and slow states and the "could not refresh" error, drive selection, the searching
  and empty states, the quit dialog and every toast and error text are present

#### Scenario: Tab contents kept
- **WHEN** the three tabs are compared with the inventory
- **THEN** the capacity alert badge, the test sizes ("does not fit" disabled with a tooltip), Start, Stop, the results
  table, the check modes, Check, the limited-layout note, every verdict text with the split, "checked in", "checked on
  <date>", restore failures and "Repair to", every Repair lead text, the new size and hint, FAT32 disabled above 32 GB,
  the name filter, the need-check and limited notes, the full-size tick and both warnings, the confirmation tick, the
  dialog naming the chosen size, the done message and the post-repair offer with its waiting and no-mount texts are
  all present

#### Scenario: Same requests
- **WHEN** the user starts a speed test, a quick or full check, a repair (default or full size), the check of a
  repaired drive, a Reload, an update action, a donation link, a language change or quits during a task
- **THEN** the window sends the same message with the same fields as the window it replaces

#### Scenario: Safety conditions unchanged
- **WHEN** the confirmation tick is off, the plan is none (keeps nothing or unconfirmed), a task runs, or the
  full-size tick belongs to an older state
- **THEN** the Erase and repair button is disabled or the tick is cleared exactly as before

#### Scenario: Translation check
- **WHEN** `node scripts/check-i18n.mjs` runs on the redesigned window
- **THEN** it passes, and every key the old window used is still used or was deliberately removed together with its
  translations
