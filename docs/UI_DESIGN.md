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
 ┌─┐
 │A│        mode switch: two segments, tap the other one to switch;
 │M│        the active one is filled (white auto, yellow manual or locked)
 └─┘
 ┌─┐
 │+│        rocker: push up for more, down for less
 │●│        speed rises with the square of the deflection
 │−│        release: stops and springs back
 └─┘
 +0.3 EV    what the rocker changes, and its value
```

- **Mode switch.** A vertical two-segment switch, `A` over `M`, 46 wide with
  two 34 pt segments, the same look as the K/Tint switch. It shows the state,
  not an action: the filled segment is the current mode. Tap the other segment
  to switch; the values are kept. The second segment reads `L` when the group
  is locked (set by the hub, or a long press on the picture for focus) or when
  the camera has no manual mode for it. Moving the rocker in auto switches white balance and focus to
  manual; exposure stays in auto and changes the compensation.
- **Rocker.** The same component as zoom, same size. About 10 changes per
  second, clamped to the control's range and snapped to its step.

| Group | Rocker changes | Full speed |
|---|---|---|
| Exposure, auto | `ev_bias` | 2 EV/s |
| Exposure, manual | `iso`, log scale | 2 stops/s |
| White balance | `wb_temperature`, or `wb_tint` with the K/Tint switch above the rocker (K each time it opens) | 1500 K/s, 60/s |
| Focus | `focus_position`, up is far | 0.4/s |

- **White balance once:** long press on the mode switch.
- The shutter is shown, not adjusted: it is the production's (docs/SHADING.md,
  "Shutter").

## Picture

- Tap: focus and expose at that point.
- Long press: lock focus there (AF-L).
- Aids drawn over the picture only: grid, horizon level, safe areas, centre
  cuts, focus peaking.

## States

- **Screen lock.** Everything stays visible but the rockers, which hide; only
  the slide to unlock (in the bottom row's place) takes touches.
- **Not connected.** A centred card: "Scan QR code" (the in-app scanner),
  "Enter manually", "Not now". The QR button stays in the bottom row.
- **Scanning.** A viewfinder square, the instruction and Cancel over the
  picture.
- **Live.** Settings that cannot change mid-stream (format, mains) are disabled
  with a note; the values row and the rockers work as always.

## Look

- Round buttons and rockers: 46 pt wide, black at 35 % with a thin white
  outline at 25 %; rockers 180 pt long (shorter on a landscape phone, both the
  same).
- Rows: 34 pt high, dark capsules, tabular digits, white text.
- Colours: yellow manual or locked, red program and clipping, green preview,
  orange warnings and connecting, cyan a change from the hub (a 1 s flash).
- Spacing from one set of constants (iOS `Chrome`, Android the same names):
  tally clearance 22, minimum edge 8, gap 10, row gap 6, zone gap 10.
