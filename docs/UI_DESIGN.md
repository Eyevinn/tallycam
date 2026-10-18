# Shooting screen design

One design for the iOS and the Android app. Both follow this document; where a
platform cannot (a camera API limit), docs/APPS.md lists the difference.
Background: [CAMERA_READOUTS](CAMERA_READOUTS.md).

## Principles

- **Information in the middle, controls at the sides.** The top centre shows
  state and values; the thumbs reach the controls on the left and right edges.
- **The value is the control.** Tap a value to adjust it, as on cinema camera
  apps. No separate mode chips.
- **One control per thumb.** Zoom on the left edge, the adjustment on the
  right edge, both the same vertical rocker, both always there. Selecting a
  value (a reach to the top) is rare; adjusting it (a thumb on the rocker) is
  what happens during a shot.
- **Chrome over the black bars.** A 16:9 picture leaves bars at the sides of a
  wide phone in landscape and at the top and bottom of an iPad; the edge
  zones and the rows sit there, so the picture stays clear.
- **Nothing covers a control.** Messages have their own slot; every element
  has a fixed place on one grid.
- **Manual stands out.** A value set by hand (manual or locked) is yellow, an
  automatic one white. Warnings are orange or red.
- **The phone and the hub are equals.** Either can change any value at any
  time; the last change wins, and both show it at once. A value the hub just
  changed flashes cyan for a second, so the operator sees the shader act.

## Zones

Landscape, the main orientation:

```
┌──────────────────────────────────────────────────────────────┐
│ ● STANDBY        ON AIR · Kamera 2               ((•)) ◷  ⚙  │ 1 status row
│      1080p30 │ ISO 26 │ 1/100 │ +0.3 EV │ 5200 K +4 │ AF     │ 2 values row
│                    [ message slot ]                          │ 3 messages
│ [4×]                                                  [A|M]  │
│ [1×] ┌─┐                                         ┌─┐         │
│      │T│                                         │+│         │
│      │●│  4 left edge          picture           │●│ 5 right │
│      │W│                                         │−│         │
│      └─┘                                         └─┘ +0.3 EV │
│ ▦  ⌗  🔒                                   ▮▮      ( GO LIVE ) │ 6 bottom row
└──────────────────────────────────────────────────────────────┘
```

Portrait: the same zones. Rows 1 and 2 stack at the top (the tally chip may
take its own line), the rockers sit on the long edges at the same height,
mirror images of each other, and the live button is bottom centre with the
bottom row above it.

1. **Status row.** Left: stream state (STANDBY, CONNECTING, LIVE with the
   bitrate, yellow while adaptive bitrate holds it down). Centre: the tally
   chip, the camera name in red with "ON AIR", green with "PREVIEW", dark when
   off. Right: hub link dot, clock dot, battery and thermal state when it
   matters, an orientation-pending icon, the Settings gear.
2. **Values row.** `format │ ISO │ shutter │ EV │ WB │ focus`, each a button
   (see "Values").
3. **Message slot.** Centred under the values row: operator messages (yellow
   banner, 8 s) and toasts (stream errors and hints, 4 s). Nothing else goes
   there and nothing else shows messages.
4. **Left edge.** Lens buttons (longest on top) above the zoom rocker,
   vertically centred.
5. **Right edge.** The adjust control, always: mode switch above the rocker,
   the value label below. It adjusts the selected group, exposure until
   another is chosen; the choice is remembered.
6. **Bottom row.** Left: aids menu, QR scan (only while no hub is connected and
   not live), screen lock. Right: microphone meter, then (landscape) the live
   button in the bottom-right corner, at a fixed place that never moves.

The tally border (14 pt, red on program, green on preview) frames the screen;
all chrome stays inside it and the safe areas.

## Values

| Item | Shows | Tap opens |
|---|---|---|
| Format | Stream size and frame rate: `1080p30`, `4K30`, `720p60` | Settings |
| ISO | `ISO 26`; with a warning word from the image statistics: `CLIP` in red, `DARK` in orange | Exposure |
| Shutter | `1/100` | Exposure |
| EV | `+0.3 EV`, only in auto exposure and when not 0 | Exposure |
| WB | `5200 K`, tint after it when not 0: `5200 K +4` (+ magenta, − green) | White balance |
| Focus | `AF` in auto, `AF-L` when locked, `F 0.42` in manual | Focus |

The values of the selected group are underlined. On a narrow screen the row
drops focus first, then white balance; it never wraps.

## Adjust control

```
 EXP        which group the control adjusts (EXP, WB, FOCUS)
 ┌─┐
 │A│        mode switch: two segments, tap the other one to switch;
 │M│        the active one is filled (white auto, yellow held)
 └─┘
 ┌─┐
 │+│        rocker: push up for more, down for less
 │●│        speed rises with the square of the deflection
 │−│        release: stops and springs back; tap an end: one step
 └─┘        double tap the knob: back to the neutral value
 +0.3 EV    what the rocker changes, and its value
```

- **Mode switch.** A vertical two-segment switch, `A` over `M`, 46 wide with
  two 34 pt segments, the same look as the K/Tint switch. It shows the state,
  not an action: the filled segment is the current mode. Tap the other segment
  to switch; the values are kept. `M` stands for any held mode: a group the
  hub locked, focus locked by a long press on the picture, and a camera
  without a manual mode for the group (it locks instead) all show `M`. The
  difference does not matter to the operator; the rocker turns locked into
  manual anyway. Moving the rocker in auto switches white balance and focus to
  manual; exposure stays in auto and changes the compensation.
- **Caption.** The group's short name above the switch (`EXP`, `WB`,
  `FOCUS`), so the rocker's job is never a guess.
- **Rocker.** The same component as zoom, same size, with the direction
  written at its ends. Three gestures, the same on every rocker:
  - **Drag** for a move: speed rises with the square of the deflection, about
    10 changes per second, clamped to the range and snapped to the step;
    letting go stops it and the knob springs back.
  - **Tap an end** for one step, for fine trimming on air.
  - **Double tap the knob** for the neutral value.
- **Haptics.** A light tick on each step and each tap, a firmer one when a
  value reaches the end of its range.
- **While adjusting** the value label grows and turns bright, and the value
  in the values row follows.

| Group | Ends | Drag, full speed | Tap step | Double tap |
|---|---|---|---|---|
| Exposure, auto | `+` / `−` | `ev_bias` 2 EV/s | 0.3 EV | EV 0 |
| Exposure, manual | `+` / `−` | `iso` 2 stops/s (log) | 1/3 stop | back to auto |
| White balance, K | `K+` / `K−` | `wb_temperature` 1500 K/s | 100 K | white balance once |
| White balance, Tint | `M` / `G` (magenta, green) | `wb_tint` 60/s | 1 | tint 0 |
| Focus | `Far` / `Near` | `focus_position` 0.4/s | 0.02 | back to auto |
| Zoom (left edge) | `T` / `W` | 2 doublings/s | 1/4 doubling | the lens's own 1× (no digital zoom) |

The K/Tint switch sits above the WB rocker and is back on K each time white
balance is chosen. The zoom rocker shows the zoom factor (`2.3×`) beside its
knob while it moves; the lens buttons follow the active lens.

- **White balance once:** long press on the mode switch.
- The shutter is shown, not adjusted: it is the production's (docs/SHADING.md,
  "Shutter").

## Picture

- Tap: focus and expose at that point (and release an AE/AF lock).
- Touch and hold: lock focus and exposure there (AE/AF lock, as in Apple's
  camera apps): focus shows `AF-L`, exposure goes to `M` at the values it
  had. A tap anywhere releases both back to auto.
- iPhone 16 and later: the Camera Control button offers zoom and exposure
  (EV in auto, ISO in manual) as app controls; the same values as the
  rockers.
- Aids drawn over the picture only: grid, horizon level, safe areas, centre
  cuts, focus peaking.

## States

- **Screen lock.** Everything stays visible but the rockers, which hide; only
  the slide to unlock (in the bottom row's place) takes touches.
- **Not connected.** A centred card: "Scan QR code" (the in-app scanner),
  "Enter manually", "Not now". The QR button stays in the bottom row.
- **Scanning.** A viewfinder square, the instruction and Cancel over the
  picture.
- **Joining a hub.** A connect link (scanned in the app or opened from the
  Camera app) for a different control server than the current one, or when
  none is set, asks first: "Join <hub host>?", with "Control hub:
  <host[:port]>" and "Stream to: <SRT host[:port]>" ("Stream: unchanged" when
  the link has none; while live, "The stream URL applies on the next Go
  Live"), and Join / Cancel. A link for the hub already in use applies
  without asking. A restored or recreated screen never asks again.
- **Live.** Settings that cannot change mid-stream (format, mains) are disabled
  with a note; the values row and the rockers work as always.

## Settings

A full-screen sheet, "Settings" with Done (Android's back does the same).
Sections in this order: who the camera is, where it connects and streams,
the production, then diagnostics. Footers are full sentences; a disabled
row's footer says why instead of the usual text.

| Section | Rows | Footer |
|---|---|---|
| **Camera name** | Text field, the default name (model and the start of the id) as placeholder | "How the hub and Strom show this camera. The operator can rename it too; empty uses the model." |
| **Control hub** | Link dot and state (Connected, Reconnecting, Not set up) with the host; the URL field (`ws://host:8090/lcp/v0/device`), reconnecting when submitted or on Done if changed; "Scan QR code" (disabled while live); "Disconnect and forget server" (only when a hub is set; asks "Forget the control hub?", "The phone disconnects from the hub. Scan its QR code to connect again.", Forget server / Cancel) | Idle: "To connect, scan the QR code in the control hub here or with the Camera app." Live: "Stop the stream to scan here; the Camera app works while live." |
| **Stream (SRT)** | The URL field (disabled while live); Video: codec and bitrate ("H.264, 6000 kbit/s", "(auto)" when not set); Adaptive bitrate switch; while live: Encoder now (with adaptive bitrate), Round trip, Retransmitted / dropped | Idle: "Set by the QR code. Edit only if you know the address." Live: "Stop the stream to change the address." Then always: "Adaptive bitrate lowers the video bitrate when the network cannot carry it, and raises it again up to the bitrate set." |
| **Exposure** | Mains: 50 Hz / 60 Hz / Off, segmented. Disabled while a hub's production applies (showing the hub's value) and while live. Left out when the camera has no mains control | "The shutter follows the mains frequency so mains-lit light does not flicker. Off outdoors, where the shutter is free." Set by the hub: "Set by the control hub." Live: "Stop the stream to change." |
| **Orientation** | Auto / Landscape / Portrait, segmented (where the app can stream portrait); Picture: lens, size and frame rate | "Auto follows how the phone is held." While a change waits: "The stream keeps its geometry while live; the new setting applies when it stops." |
| **Clock** | Health (dot and word), Synchronised, Estimated error, Maximum error, Reference ("name (address)"), Stratum, Sources reachable ("3 of 4"), then the findings as small text; "Clock not running" without a clock | "The phone's own NTP-disciplined clock, used to timestamp frames." |
| **About** | App version ("0.2.0 (12)"), Camera name, Model, the OS and its version ("iOS", "Android"), Stream id, Device key (first 16 hex characters in groups of 4, monospaced; the hub's camera page shows the same) | None |

## Look

- Round buttons and rockers: 46 pt wide, black at 35 % with a thin white
  outline at 25 %; rockers 180 pt long (shorter on a landscape phone, both the
  same).
- Rows: 34 pt high, dark capsules, tabular digits, white text.
- Colours: yellow manual or locked, red program and clipping, green preview,
  orange warnings and connecting, cyan a change from the hub (a 1 s flash).
- Spacing from one set of constants (iOS `Chrome`, Android the same names):
  tally clearance 22, minimum edge 8, gap 10, row gap 6, zone gap 10.
