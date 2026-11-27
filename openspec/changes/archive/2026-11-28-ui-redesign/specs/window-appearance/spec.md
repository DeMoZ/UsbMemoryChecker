# Spec Delta

## Purpose

Defines how the application window looks and how it is used without a mouse: themes, contrast, how states are
shown, focus and keyboard navigation, dialogs and toasts, motion, and the styling limits that keep the page working
offline in both system web views.

## ADDED Requirements

### Requirement: Light and dark follow the system
The window SHALL use the dark theme when the system appearance is dark and the light theme when it is light, and
SHALL switch when the system appearance changes while the app runs. Both themes SHALL come from one set of colour
tokens, and the chart SHALL use the current theme's colours.

#### Scenario: Switching appearance
- **WHEN** the system switches from dark to light while the window is open
- **THEN** the page, the chart (on its next draw) and the dialogs use the light tokens without a restart

### Requirement: Contrast
In both themes, body text and hint text SHALL have a contrast ratio of at least 4.5:1 against the surface they are
on. Large numbers, icons, focus rings and control borders (inputs, ticks, unselected segmented controls) SHALL have at
least 3:1. Text on the signal and danger buttons SHALL have at least 4.5:1.

#### Scenario: Token check
- **WHEN** the contrast script of the layout check computes the ratios of the theme tokens
- **THEN** every pair listed above meets its minimum in light and in dark

### Requirement: State is never shown by colour alone
Every capacity state, verdict, note, toast and the Real capacity tab's alert SHALL carry an icon or a text besides its
colour.

#### Scenario: Fake drive in greyscale
- **WHEN** the window with a fake drive is viewed without colour
- **THEN** the Real capacity readout shows a cross icon and "Fake drive!", the rail shows the cross icon, and the
  verdict shows its icon and title

### Requirement: Visible focus and keyboard navigation
Every interactive control SHALL show a clearly visible focus ring when focused from the keyboard. The tabs SHALL be
a tab list: arrow keys move between tabs, and Home and End go to the first and last tab. The drive list SHALL be a
list in which the Up and Down arrows select the previous or next drive. The test sizes, the check modes and the file
systems SHALL be radio groups operated with the arrow keys. Disabled options SHALL be skipped. All of it SHALL be
reachable with Tab in visual order.

#### Scenario: Keyboard only
- **WHEN** the user presses Tab from the start of the window
- **THEN** focus visits the language choice, the update and Support buttons, Reload, the selected drive, the tabs,
  then the controls of the selected tab in their visual order, each with a visible focus ring

#### Scenario: Arrow keys in the tabs
- **WHEN** the Speed tab has focus and the user presses the Right arrow
- **THEN** the Real capacity tab becomes selected and focused, as a click would do

#### Scenario: Arrow keys in the drive list
- **WHEN** a drive in the list has focus and the user presses the Down arrow
- **THEN** the next drive is selected, as a click would do (the full-size tick is cleared as on any drive change)

### Requirement: Dialogs keep the focus
When a dialog (Support, updates, confirm repair, confirm quit) opens, focus SHALL move into it, Tab SHALL stay inside
it, Escape and the close button SHALL close it, and focus SHALL return to the control that opened it. The close
button SHALL have an accessible name in the selected language. In the confirm-repair dialog the initial focus SHALL
be on Cancel.

#### Scenario: Confirm repair
- **WHEN** the user presses Erase and repair and the dialog opens
- **THEN** Cancel has focus, Enter on it closes the dialog without a repair, and focus returns to the Erase and repair
  button

### Requirement: Toasts stay out of the way
Toasts SHALL appear in a corner of the window, SHALL be announced to screen readers (errors as alerts), and SHALL close
on click or on Escape before their timeout.

#### Scenario: Long error
- **WHEN** a 9-second error toast covers part of the outcome column
- **THEN** a click on it or Escape closes it at once

### Requirement: Motion is feedback only
Motion SHALL be used only for feedback: hover and press (about 120 ms), state changes (about 200 ms), panel changes
(at most 320 ms, opacity and at most 4 px of movement), the progress bar, and the busy indicators. Nothing SHALL move
in a loop while no task runs, except the searching spinner before the first scan. When the system asks for reduced
motion, the window SHALL show no animations or transitions. Busy and progress states SHALL still be clear without
motion.

#### Scenario: Reduced motion
- **WHEN** reduced motion is on and a speed test runs
- **THEN** nothing animates, the progress bar jumps to each new value, and the busy state is still shown (static
  spinner ring, progress text)

#### Scenario: Idle window
- **WHEN** no task runs and the drive list has loaded
- **THEN** no element of the page animates

### Requirement: Offline and engine-safe styling
The page SHALL load no external resource (fonts, images, scripts, styles). It SHALL use the system fonts, and it SHALL
use only CSS and JavaScript features that work the same in the WKWebView of the supported macOS versions and in
WebView2. The look SHALL be the same in both, apart from the system font and the title bar.

#### Scenario: No network
- **WHEN** the app starts with no network connection
- **THEN** the window looks and works the same, and no request is made by the page

#### Scenario: Same look in both engines
- **WHEN** the layout check renders the same states in WKWebView and in headless Chromium
- **THEN** both pass the no-scroll check, and their screenshots differ only in font rendering

### Requirement: App icon is readable at every size
The app icon SHALL be drawn natively for every size it ships in, not shrunk from one large image. The Windows icon
file SHALL hold 16, 20, 24, 32, 40, 48, 64, 128 and 256 px images that fill their canvas, and the macOS icon SHALL
hold 16 to 1024 px images on the macOS icon grid. At 48 px and below every edge SHALL sit on a whole pixel, with no
arc and no shadow.

#### Scenario: Contact sheet
- **WHEN** the icon is rebuilt and the contact sheet is drawn from the packed `.ico` and `.icns` files
- **THEN** it shows every listed size at 1x on white, light grey, dark grey and black, and at 16, 24 and 32 px the
  tile edge, the plug, the cyan body and the check mark are each distinct pixels on every background

### Requirement: Title bar mark matches the app icon
The mark next to the app name in the window's title bar SHALL be the 24 px app icon drawing in the icon's own
colours, in both themes.

#### Scenario: Title bar mark
- **WHEN** the window is shown in the light or the dark theme
- **THEN** the mark next to the app name is the 24 px icon drawing in the icon's own colours

### Requirement: Windows shows the app icon on the window
On Windows the window SHALL use the icon embedded in the executable for its title bar (small icon) and its taskbar
button and Alt+Tab (big icon), at the size matching the display scaling. If the icon resource is missing, the
window SHALL still open.

#### Scenario: Taskbar on a scaled display
- **WHEN** the app starts on Windows at 150% scaling
- **THEN** the title bar and the taskbar button show the app icon loaded from the executable at 24 px and 48 px,
  not the generic window icon
