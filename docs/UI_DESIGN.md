# Shooting screen design

One design for the iOS and the Android app. Both follow this document; where a
platform cannot (a camera API limit), docs/APPS.md lists the difference.
Background: [CAMERA_READOUTS](CAMERA_READOUTS.md).

## Principles

- **Corners, not the centre.** The centre of the screen is the picture's.
  Controls live in the four corners (on phones: tools upper left, live
  upper right, side controls lower left and right) and information in the
  top bar along the top edge. Only short messages show at the top centre,
  and they go away by themselves.
- **Everything a shot needs is under a thumb.** Two identical side
  controls, one per edge, each a rocker with its selector and mode beside it
  on the inner side. Either can run zoom, exposure, white balance or focus;
  zoom on the left and exposure on the right until the operator picks
  otherwise. Neither hand has to change its grip.
- **The value is also a way in.** Tapping a value in the values row selects
  its group too, as on cinema camera apps; it is a shortcut, not the only
  way.
- **Chrome over the black bars.** A 16:9 picture leaves bars at the sides of a
  wide phone in landscape and at the top and bottom of an iPad; the edge
  zones and the rows sit there, so the picture stays clear.
- **Nothing covers a control.** Messages have their own slot; every element
  has a fixed place on one grid.
- **Manual stands out.** A value set by hand (manual) is yellow, an
  automatic one white. Warnings are orange or red.
- **The phone and the hub are equals.** Either can change any value at any
  time; the last change wins, and both show it at once. A value the hub just
  changed flashes cyan for a second, so the operator sees the shader act.

## Zones

Landscape on a phone, the main case:

```
┌────────────────────────────────────────────────────────────────────────┐
│ ● STANDBY  ON AIR · Kamera 2  1080p30 │ ISO 26 │ 1/100 │ AF    ● ● ▭ ⚙ │ top bar
│ ▦                      [ message slot ]                  ▮ ( GO LIVE ) │ tools, live
│ ⌗                                                                      │
│ ⊡                                                                      │
│                                                                        │
│ ┌─┐ ┌────┬────┐                                        ┌────┬────┐ ┌─┐ │ side controls
│ │T│ │ZOOM│EXP │                                        │ZOOM│EXP▪│ │+│ │
│ │ │ ├────┼────┤                                        ├────┼────┤ │ │ │
│ │●│ │ WB │FOC │                                        │ WB │FOC │ │●│ │
│ │ │ ├────┼────┤                                        ├────┼────┤ │ │ │
│ │W│ │ 4× │ 1×▪│  2.3×                         +0.3 EV  │ A  │ M▪ │ │−│ │
│ └─┘ └────┴────┘                                        └────┴────┘ └─┘ │
└────────────────────────────────────────────────────────────────────────┘
```

**Landscape: one top bar.** Rows 1 and 2 merge into a single bar so the
picture gets the room: stream state left, the name chip (in its tally
colour) and the values centre, the dots and the gear right. When it does
not fit, the values drop focus, then white balance, then the name chip
shortens to the name alone (the tally border still shows tally).

Portrait: the same zones. Rows 1 and 2 stack at the top (the tally chip may
take its own line) and the side controls sit in the bottom corners, mirror
images of each other. Tools and the live button: see 6.

1. **Status row.** Left: stream state (STANDBY, CONNECTING, LIVE with the
   bitrate, yellow while adaptive bitrate holds it down). Centre: the tally
   chip, the camera name in red with "ON AIR", green with "PREVIEW", dark when
   off. Right: hub link dot, clock dot, battery; thermal state when it
   matters, an orientation-pending icon, the Settings gear. Next to the dots
   a time-of-day timecode, `14:51:46:12` (hours, minutes, seconds, frames at
   the stream rate), from the phone's disciplined clock in local time, in
   tabular digits: white while the clock is healthy, orange when degraded,
   dim with `--` frames when it is not synchronised. The same timecode on
   every camera, so two phones in shot can be compared at a glance.
2. **Values row.** `format │ ISO │ shutter │ EV │ WB │ focus`, each a button
   (see "Values").
3. **Message slot.** Centred under the values row: the yellow banner (8 s)
   for operator messages and the app's connection messages ("Connecting to
   …", "Not a valid connect code", control errors), and toasts (stream
   errors and hints, 4 s). Nothing else goes there and nothing else shows
   messages.
4. **Left edge** and 5. **right edge.** A side control each, mirror images,
   where the thumbs rest, the same height on both sides (see "Side
   control"). **Phones:** in the bottom-left and bottom-right corners,
   against the bottom edge. **iPad:** the centre at 65 % of the screen height
   from the top, moved up only as far as it must to clear the bottom row.
   The upper part of the picture stays clear. Zoom on the left and exposure
   on the right by default; each side remembers its choice.
6. **Tools and the live button.** The tools are the aids menu, QR scan (only
   while no hub is connected and not live) and the screen lock.
   - **iPad:** a bottom row: tools left; microphone meter, then the live
     button in the bottom-right corner (landscape) or bottom centre
     (portrait).
   - **Phones (both orientations):** the bottom corners belong to the side
     controls, so everything else goes to the upper corners, just under the
     top rows: the tools in a row in the upper-left corner, the live button in the upper-right corner at a fixed
     place with the microphone meter beside it on the inner side. The middle
     of the screen stays clear. In portrait the side controls are too wide
     side by side with their value beside them, so the value and K / Tint sit
     above each block. (A tools column does not fit above the side control
     and its tab on a landscape phone, hence the row.)
   - **Screen lock on a phone:** the slide to unlock sits at the bottom edge
     between the side controls in landscape, just above them in portrait.

   The live button is dark at standby, orange connecting, red live; a tap
   starts, a hold stops (with a filling ring and a "Hold to stop" hint),
   with a haptic when it goes live or stops.

The tally border (14 pt, red on program, green on preview) frames the screen;
all chrome stays inside it and the safe areas.

## Values

| Item | Shows | Tap selects |
|---|---|---|
| Format | Stream size and frame rate: `1080p30`, `4K30`, `720p60` | Opens Settings at Picture |
| ISO | `ISO 26`; with a warning word from the image statistics: `CLIP` in red, `DARK` in orange | Exposure |
| Shutter | `1/100` | Exposure |
| EV | `+0.3 EV`, only in auto exposure and when not 0 | Exposure |
| WB | `5200 K`, tint after it when not 0: `5200 K +4` (+ magenta, − green) | White balance |
| Focus | `AF` in auto, `F 0.42` in manual | Focus |

The values of the groups on the side controls are underlined. A tap on a
value puts its group on the right side control, unless one of the sides
already shows it. On a narrow screen the row drops focus first, then white
balance; it never wraps.

## Side control

Right side shown; the left one is its mirror image.

```
 ┌──────┬──────┐  ┌─┐
 │ ZOOM │ EXP▪ │  │+│   selector: a 2 × 2 grid, the chosen group filled
 ├──────┼──────┤  │ │
 │  WB  │ FOC  │  │●│   rocker: push up for more, down for less;
 └──────┴──────┘  │ │   speed rises with the square of the deflection;
 ┌──────┬──────┐  │ │   release: stops and springs back; tap an end: one
 │  A   │  M▪  │  │−│   step; double tap the knob: the neutral value
 └──────┴──────┘  └─┘
      +0.3 EV           the value (zoom: the factor, 2.3×)
     [ K | Tint ]       white balance only; the row is kept for every group
```

- **Selector.** A 2 × 2 grid, `ZOOM` `EXP` over `WB` `FOC`, beside the upper
  half of the rocker on the inner side; the chosen group is filled in its
  colour with white text. Tap a cell to put that group on this side. Both
  sides may not run the same group: choosing the group the other side runs
  swaps the two. The rocker's job is never a guess.
- **Hide and show: one tab.** Each side control has a tab (46 × 46, the
  group's colour, a chevron) at a fixed place just above its rocker at the
  screen edge. While the control shows, the chevron points out to the edge
  (‹ on the left, › on the right) and a tap folds the control away, leaving
  only the tab, so the picture is clear. While hidden, the same tab at the
  same place points inward and a tap brings the control back with its last
  group. A tap on the chosen selector cell does nothing. Either side or both
  may be hidden; the choice is remembered like the group. A tap on a value
  in the values row brings back a hidden side that runs its group; a group
  no side runs goes to the right side and shows it. A swap keeps each
  side's hidden flag. Only shown sides underline their values.
- **Mode switch.** Two cells side by side, `A` and `M`, under the selector,
  the same size and look. It shows the state, not an action: the filled cell
  is the current mode (white auto, yellow manual). Tap the other cell to
  switch; the values are kept. There are only two modes: `A` (auto) and `M`
  (manual). Going to `M` from auto (the switch, a long press on the picture,
  white balance once) holds the camera's current values as numbers, which
  the rocker then changes. Moving the rocker in auto switches white balance
  and focus to manual; exposure stays in auto and changes the compensation. For zoom
  the lens buttons take this row (only on a camera with more than one lens),
  widest on the left to longest on the right (`0.5×` `1×` `4×`), as in the
  iPhone camera, on both sides.
  For white balance a third cell, `◎`, sits beside `A` and `M`: a tap sets
  the white balance once from the picture (`wb_one_shot`) and holds it (`M`).
- **A colour per group,** so the usual setup is recognised at a glance (blue
  left, red right by default): zoom blue, exposure red, white balance
  violet, focus green. It marks only the control: the rocker's outline and
  knob ring, the chosen selector cell and the underline of the group's
  values in the values row. Values never take it as text colour, so yellow
  (held), red and green (tally), cyan (hub change) and orange (warning) keep
  their meaning.
- **See-through, no blur.** The picture must show through the controls
  clearly: no background blur or frosted material anywhere. Every chrome
  surface (cells, value, K / Tint, the rocker track, round buttons, the top
  bar) is a light tint, black at about 15 %, with a thin white border at
  40 % to give it its shape. The chosen cell is filled in its colour at
  70 %, the knob light grey at 70 %. Text and labels are white with a soft
  dark shadow so they read over any picture.
- **Shape: square-ish, not pills.** Selector and mode cells have an 8 pt
  corner radius, the rocker track a 10 pt radius (a rounded rectangle, not a
  capsule), the knob a rounded square with an 8 pt radius. The same on both
  sides and on zoom.
- **Sizes: easy to hit without looking.** Every cell is at least 44 pt high
  and 46 pt wide (Android 48 dp), the whole selector and mode switch about
  100 pt wide; they never shrink below that. **The rocker has a fixed, moderate
  size:** 56 pt wide and exactly as tall as the selector and mode row beside
  it (about 150 pt), the same on every screen, phone or iPad, both sides
  alike; it never grows to fill the screen. The block and the rocker are
  bottom-aligned. The value and the K / Tint row sit beside the block on its
  inner side, level with the mode row, so the whole side control is no
  taller than the rocker. Lens buttons share the mode row's width; the
  A | M cells stay full size. **Nothing moves
  when a group changes:** every slot has a fixed size worked out from the
  screen once, and the K / Tint row is kept (empty) for every group.
- **Rocker.** The same component as zoom, same size, with the direction
  written at its ends. Three gestures, the same on every rocker:
  - **Drag** for a move: speed rises with the square of the deflection, about
    10 changes per second, clamped to the range and snapped to the step;
    letting go stops it and the knob springs back.
  - **Tap an end** for one step, for fine trimming on air.
  - **Double tap the knob** for the neutral value.
- **Haptics.** A light tick on each step of a value and each tap, a firmer
  one when a value reaches the end of its range; the zoom drag ticks only at
  its ends.
- **While adjusting** the value label grows and turns bright, and the value
  in the values row follows.

| Group | Ends | Drag, full speed | Tap step | Double tap |
|---|---|---|---|---|
| Exposure, auto | `+` / `−` | `ev_bias` 2 EV/s | 0.3 EV | EV 0 |
| Exposure, manual | `+` / `−` | `iso` 2 stops/s (log) | 1/3 stop | back to auto |
| White balance, K | `K+` / `K−` | `wb_temperature` 1500 K/s | 100 K | white balance once |
| White balance, Tint | `M` / `G` (magenta, green) | `wb_tint` 60/s | 1 | tint 0, in manual white balance (nothing in auto) |
| Focus | `Far` / `Near` | `focus_position` 0.4/s | 0.02 | back to auto |
| Zoom | `T` / `W` | 2 doublings/s | 1/4 doubling | the lens's own 1× (no digital zoom) |

The K/Tint switch is back on K each time white balance is chosen. On zoom
the value is the zoom factor (`2.3×`), counted against the wide lens like
the lens buttons, which follow the active lens.

- **White balance once:** the `◎` cell, or a long press on the mode switch.
- The shutter is shown, not adjusted: it is the production's (docs/SHADING.md,
  "Shutter").

## Picture

- Tap: focus and expose at that point (and release an AE/AF lock).
- Touch and hold: hold focus and exposure there (AE/AF lock, as in Apple's
  camera apps): both go to `M` at the values they had, shown as numbers
  (`F 0.42`, the ISO), with a short "AE/AF LOCK" badge. A tap anywhere
  returns both to auto.
- iPhone 16 and later: the Camera Control button offers zoom and exposure
  (EV in auto, ISO in manual) as app controls; the same values as the
  rockers.
- Aids drawn over the picture only, each switch persisted: grid, horizon
  level, safe areas (93 % / 90 %), centre cuts (4:3, 9:16), focus peaking
  (edges in red). Grid and horizon level are on from the first launch, the
  others off.
- The aids menu also holds **Torch** (on/off, only where the lens has one).
  It is a light on the subject, not an aid, but it belongs with the
  switches the operator flips while filming. The hub can switch it too.

## States

- **Screen lock.** Everything stays visible but the rockers, which hide; only
  the slide to unlock takes touches (phones: see 6; iPad: the bottom row's
  centre in landscape, its own row above the bottom row in portrait). A
  toast suggests it at Go Live.
- **Not connected.** A centred card: "Scan QR code" (the in-app scanner),
  "Enter manually", "Not now". The QR button stays with the tools.
- **Scanning.** A viewfinder square, the instruction and Cancel over the
  picture.
- **Joining a hub.** A connect link (scanned in the app or opened from the
  Camera app) for a different control server than the current one, or when
  none is set, asks first: "Join <hub host>?", with "Control hub:
  <host[:port]>" and "Stream to: <SRT host[:port]>" ("Stream: unchanged" when
  the link has none; while live, "The stream URL applies on the next Go
  Live"), and Join / Cancel. A link for the hub already in use applies
  without asking. A restored or recreated screen never asks again.
- **Live.** Settings that cannot change mid-stream (resolution, frame rate,
  video and audio codec, mains) are disabled with a note; the values row and
  the rockers work as always.

## Settings

The phone can change every camera setting itself; the hub can too, and the
last change wins (as for shading). Settings goes through the same control
table as the hub's sets, so both get the same checks and clamping.

A sheet (iOS page sheet, Android full screen), "Settings" with Done
(Android's back does the same).
Sections in this order: who the camera is, where it connects and streams,
the production, then diagnostics. Footers are full sentences; a disabled
row's footer says why instead of the usual text.

| Section | Rows | Footer |
|---|---|---|
| **Camera name** | Text field, the default name (model and the start of the id) as placeholder | "How the hub and Strom show this camera. The operator can rename it too; empty uses the model." |
| **Control hub** | Link dot and state (Connected, Reconnecting, Not set up) with the host; the URL field (`ws://host:8090/lcp/v0/device`), reconnecting when submitted or on Done if changed; "Scan QR code" (disabled while live); "Disconnect and forget server" (only when a hub is set; asks "Forget the control hub?", "The phone disconnects from the hub. Scan its QR code to connect again.", Forget server / Cancel) | Idle: "To connect, scan the QR code in the control hub here or with the Camera app." Live: "Stop the stream to scan here; the Camera app works while live." |
| **Stream (SRT)** | The URL field (disabled while live); SRT latency as a stepper, "− 120 ms +" in 20 ms steps (0 to 8000; it is the URL's `latency`, so the field and the stepper show the same value; a change applies on the next Go Live); Adaptive bitrate switch; while live: Encoder now (with adaptive bitrate), Round trip, Retransmitted / dropped | Idle: "Set by the QR code. Edit only if you know the address." Live: "Stop the stream to change the address." Then always: "Adaptive bitrate lowers the video bitrate when the network cannot carry it, and raises it again up to the bitrate set." |
| **Picture** | Segmented pickers labelled Lens, Resolution, Frame rate (what the active lens offers: the `resolution` and `frame_rate` controls' lists) Video codec and Audio codec (Opus, AAC); Bitrate as a stepper, "− 6000 kbit/s +" in 500 kbit/s steps within the control's range, "(auto)" while unset; all disabled while live | "Stop the stream to change." while live; otherwise none |
| **Sync** | Genlock switch with its state (Locked, Locking, Off) and the phase error when locked; Genlock offset as a stepper in µs (−50000 to 50000, 100 µs steps, double tap for 0), shown only while genlock is on; Stabilization as a segmented picker of the modes the lens has (Off, Standard, Cinematic, ...) | "Genlock aligns frames with other cameras on the same clock. The offset moves where this camera's frames land, for calibration. Stabilization off for multi-camera sync tests." |
| **Exposure** | Mains: 50 Hz / 60 Hz / Off, segmented, showing the mains in use. Disabled only while live; under a hub's production a change holds for this camera until the hub sends a different production (last write wins). Left out when the camera has no mains control | "The shutter follows the mains frequency so mains-lit light does not flicker. Off outdoors, where the shutter is free." Under a hub production: "The control hub sets this for all cameras; a change here applies to this camera until the hub changes it." Live: "Stop the stream to change." |
| **Orientation** | Auto / Landscape / Portrait, segmented (where the app can stream portrait) | "Auto follows how the phone is held." While a change waits: "The stream keeps its geometry while live; the new setting applies when it stops." |
| **Clock** | Health (dot and word), Synchronised, Estimated error, Maximum error, Reference ("name (address)"), Stratum, Sources reachable ("3 of 4"), then the findings as small text; "Clock not running" without a clock | "The phone's own NTP-disciplined clock, used to timestamp frames." |
| **Diagnostics** | Collapsed by default ("Diagnostics", a disclosure row). Torch probe (runs the `torch_probe` command and shows its summary when done); Share clock log (`clock_log`, through the share sheet); in Debug builds also Frame duration (`frame_duration`, µs) | "For measurements and support. Nothing here is needed to film." |
| **About** | App version ("0.2.0 (12)"), Camera name, Model, the OS and its version ("iOS", "Android"), Stream id, Device key (first 16 hex characters in groups of 4, monospaced; the hub's camera page shows the same) | None |

## Look

- Round buttons: 46 pt. Every surface is see-through (see "Side control"):
  black at 15 % with a 1 pt white border at 40 %; a rocker's outline and
  its knob's ring are in the group colour.
- Top rows: 34 pt high, tabular digits, white text with a soft shadow.
- Group colours (controls only): zoom blue `#3B82F6`, exposure red
  `#EF4444`, white balance violet `#A855F7`, focus green `#22C55E`.
- Colours: yellow manual, red program and clipping, green preview,
  orange warnings and connecting, cyan a change from the hub (a 1 s flash).
- Spacing from one set of constants (iOS `Chrome`, Android the same names):
  tally clearance 22, minimum edge 8, gap 10, row gap 6, zone gap 10.
  Distance from a screen edge: max(safe-area inset + 8, 22), so chrome is
  8 pt clear of the Dynamic Island, a cutout or the home indicator, and
  22 pt in where there is none.
