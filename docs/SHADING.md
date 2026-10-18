# Shading: exposure, colour and focus control

Status: design agreed 2026-10-16 (decisions at the end), ownership dropped
2026-10-18 (D13). Phase 1 in progress.
PROTOCOL.md gets the message definitions as they are built, and the code
becomes the source of truth for how this works.

## Goal

Exposure and colour must look right in a production that has no shader (the
person who sets iris and colour on every camera from one desk). A central
shader must still be possible when there is one. The shader is optional and
never required.

The phone has per-control modes (`exposure_mode`, `wb_mode`, `focus_mode`:
auto, locked, manual). **The shader (hub page) and the camera operator
(phone) can both change exposure, colour and focus at any time. Nothing is
locked either way: the last write wins** (D13). Both see the values the
other set: the phone in its readout and controls, the hub from `state`.

## Control groups

Three groups per camera, for display. The group of a control is the `group`
in its descriptor, which the phone already sends.

| Group | Controls | Commands |
|---|---|---|
| `exposure` | `exposure_mode`, `iso`, `ev_bias` | the exposure side of `focus_at` (metering point) |
| `white_balance` (shown as "Colour") | `wb_mode`, `wb_temperature`, `wb_tint` | `wb_one_shot` |
| `focus` | `focus_mode`, `focus_position` | `focus_at` |

Not in any group:

- **Shutter** becomes a production setting (see "Shutter").
- **Lens, zoom and stabilisation** stay as they are: the camera operator and
  the server both set them.
- **Resolution, frame rate and codec** cannot be changed while the camera is
  live, from the phone or from the server (see "Locked while live").

## Modes

Each group is in one of the descriptor's modes, set by whoever wrote last:

| Mode | Values from | Phone readout |
|---|---|---|
| **auto** (default) | the phone's auto exposure (the core's auto ISO loop at the production shutter, see "Shutter"), auto white balance, autofocus | plain |
| **locked** | held where auto left them | yellow |
| **manual** | set by the operator or the shader | yellow |

- `ev_bias` works on top of auto exposure, from either side.
- Tap to focus changes only the focus group, plus the exposure metering
  point when exposure is auto. A tap never changes the exposure mode.
- A phone that never meets a hub works the same: there is nothing to own.

## LCP messages

Protocol version 2: `hello.protocol: 2` says the phone understands
`production` and `stats` (and sends `image_stats`). Version 1 phones keep
working against a version 2 hub, without them.

Earlier version 2 builds had ownership (`own`, `release`, `ownership`, a
`holder` on controller requests). It is gone, the version number stays: the
hub ignores an `ownership` message from such a phone (an unknown type) and a
`holder` field from such a controller. A phone ignores `own` and `release`
from an old hub.

### Server → device

```json
{ "type": "production", "mains_hz": 50, "shutter": "mains" }

{ "type": "stats", "enabled": true, "fps": 1 }
```

- `production`: see "Shutter". Sent after admission and when it changes.
- `stats`: see "Image statistics".

### Controller ↔ hub

- `set` and `command` are forwarded as before, for every group.
- `stats` (`device`, `enabled`, `fps?`) sets that device's preference. The
  hub sends `stats` to protocol 2 devices only while a controller is
  connected.
- `production` (`mains_hz`, `shutter`, no `device`) changes the production.
  Accepted any time (values are checked: mains 50, 60 or 0, a fixed shutter
  above 0). The hub sends it to every connected protocol 2 camera,
  broadcasts it to every controller, puts it in `snapshot` and keeps it in
  its store (`--state-file`), so it survives a restart. `--mains-hz` and
  `--shutter`, when given, replace the stored values at startup.
- Refusals are `error` messages to the sending controller only.
- The hub forwards `image_stats` as `device_image_stats`. The snapshot
  carries each device's `protocol` and latest `image_stats`. A device's
  `status` and `image_stats` are dropped when it goes offline (so a stale
  stream state does not read as live after a reconnect), and `image_stats`
  when its statistics are switched off.

### What a protocol 2 device must do

1. On `production`: apply `mains_hz` and the shutter, or keep them until the
   stream stops if live (see "Shutter").
2. On `stats`: start or stop `image_stats` at the rate asked for.
3. Apply `set` and `command` on any group, as from the operator. Report the
   result in `state`, as for every control.

A version 1 device needs none of this. The hub accepts both versions, and
warns only about a version newer than its own.

## Shutter

iPhones have a fixed aperture and no ND filter. The shutter is a flicker
setting, not an exposure knob: under mains-powered light, any shutter that
is not a whole number of light pulses flickers. So:

- **The shutter is a production setting,** not a per-camera control. The
  default is the mains period: 1/50 at 50 Hz, 1/60 at 60 Hz. Options: `mains`
  (default), a fixed flicker-free value (`100` for 1/100, for 100 fps or
  sharper motion), or `free` (no mains-lit light in the picture, see D1).
- **The shutter is free** when the policy is `free`, or when it is `mains`
  and `mains_hz` is 0 (no mains light: nothing to flicker against). One rule
  for the core (`Production::shutter_free`), the phones and the hub page.
  When free, the shutter can be set per camera in manual exposure, and the
  auto ISO loop holds a 180° shutter (half the frame period: 1/60 at 30
  fps), shortening it only once ISO is at its minimum.
- **Mains on the phone.** Without a hub production the phone uses its own
  mains setting, saved in its Settings (default from the region). A hub's
  `production.mains_hz` overrides it without overwriting the saved value;
  forgetting the server returns the phone to its own.
- **Changed live, applied after.** The hub accepts a new `production` at any
  time. A phone that is live keeps its current mains and shutter until the
  stream stops, then applies the new ones (and logs that it waits). The hub
  page says so while any camera is live.
- **The shutter must fit the frame.** 1/50 does not fit at 60 fps. The phone
  then uses the nearest flicker-free value that fits (1/100), as the core's
  rounding already does. It reports the shutter it uses in `state`, and the
  hub shows the difference as a warning.
- `shutter` stays in `state`, read-only unless the shutter is free.

With the shutter fixed, **ISO is the only exposure control** (the "iris").
So:

- Exposure **auto** is the core's auto ISO loop at the production shutter.
  It already exists for genlock. iOS continuous auto exposure moves the
  shutter (and fights genlock), so it is no longer an exposure mode. With a
  free shutter the core loop holds 180° and shortens it only once ISO is at
  its minimum (D1).
- `exposure_mode` options become `auto`, `locked` and `manual`. `auto_iso`
  merges into `auto`. The phone still accepts `auto_iso` as `auto` for one
  version.
- `manual` sets ISO only, and the shutter too when the shutter is free.

## Locked while live

Resolution, frame rate, codec and shutter cannot be changed while live:

- The phone refuses a `set` of `resolution`, `frame_rate`, `video_codec` or
  `audio_codec` while live, with "not while live". Today the resolution is
  held as pending, frame rate applies live and the codecs apply on the next
  Go Live. All three become a refusal, so nobody thinks a change took effect
  when it did not (D11).
- The descriptors say so with a new `locked_while_live: true`, so a UI greys
  them out while `status.stream.state` is `live` or `connecting`.
- On the phone the settings are disabled while live, with a short "Stop the
  stream to change".

## Phone UI (camera operator)

The current minimal view stays. Additions, each one toggleable where it sits
on the picture:

| Addition | Notes |
|---|---|
| **Readout** | One line of the current values: `1080p30 · ISO 26 · 1/100 · +0.3 EV · 5200 K · AF`. A manual or locked group's value in yellow, auto plain. Always visible, also with the screen locked |
| **Grid, horizon, frame guides** | Rule of thirds; a horizon line from Core Motion (gravity vector), green when level; 16:9 action and title safe areas, and 4:3 and 9:16 centre-cut guides. Drawn over the preview only, never in the stream |
| **Zoom and lens** | A vertical rocker at the edge for smooth zoom (speed by deflection, for a usable zoom move on air), lens buttons (0.5×, 1×, 2×/3×/5×, those that exist), pinch as well |
| **Focus** | Tap to focus (exists), long press to lock focus (and `LOCKED` on the chip), focus peaking |
| **Audio meter** | A small peak/RMS meter of the microphone, with a clip marker |
| **Exposure warning** | "Too dark" or "Clipping" from the image statistics (below) and the auto ISO limits (`TooDark`, `TooBright`) |
| **Screen lock** | One button locks all touch input except the unlock gesture (a slide). Suggested at Go Live. It blocks touches only: remote control, tally and the stream keep running. Guided Access is still the recommendation against leaving the app |

Focus peaking needs the frames, and the preview today is a capture preview
layer that the app cannot draw into. The light approach: an overlay drawn
from the video output at a low rate (edge detection on a downscaled luma
plane), on top of the preview layer. (D10)

## Image statistics

The phone needs them for its exposure warning, the hub view needs them for
its exposure display, and the auto-shader needs them in phase 2. The same
numbers serve all three, so they are part of phase 1:

```json
{ "type": "image_stats",
  "luma": { "mean": 0.43, "p01": 0.04, "p50": 0.41, "p99": 0.96,
            "clipped": 0.012, "crushed": 0.003,
            "histogram": "<32 bins, one byte each, base64>",
            "waveform":  "<32 columns x 16 levels, one byte each, base64>" },
  "colour": { "neutral_rg": 0.98, "neutral_bg": 1.04, "neutral_fraction": 0.21 },
  "camera": { "iso": 400, "shutter": 50, "wb_temperature": 5150, "wb_tint": 4,
              "exposure_offset_ev": -0.2 } }
```

- Computed on the phone from the picture that is encoded (after the ISP), on
  a downscaled copy (about 240×135 for 1080p), at most 4 times a second, off
  the capture queue. It works like the preview JPEG: one buffer retained, the
  work done on a utility queue. The cost is small next to the preview JPEG.
- `clipped` and `crushed`: the share of pixels at the top and bottom of the
  range. `neutral_*`: the mean R/G and B/G of near-grey pixels and how much
  of the picture they cover, so colour matching can tell whether there is
  anything neutral to match on.
- `camera`: the settings at the moment of measurement, so a reader does not
  have to line up `state` with the statistics by time.
- Sent to the hub only while a controller asks for them (as with `preview`),
  at 1 per second by default: about 1 KB/s per camera.
- The computation is a pure function of a luma and chroma plane, so it goes
  into the core and Android gets the same numbers. The downscale stays native.

**Why the phone computes them, not the hub:** the proposal in the brief, and
the recommendation. Decoding every camera's SRT stream on the hub is what
the crowd pool deliberately avoids. The preview JPEG is lossy, scaled,
converted to sRGB (clipping is hard to read from it), and only sent while
someone watches. The phone has the exact picture, needs the numbers itself,
and sends about 1 KB/s. The cost is one more message type and one function
in the core.

## Hub UI (shader view, optional)

- **All cameras side by side,** each with its preview, a waveform or
  histogram from `image_stats` (switchable), clip and crush shares, tally,
  the readout line (as on the phone) and an exposure warning badge (auto ISO
  at its limit, from the phone's warn log; clipping or too dark from the
  statistics). The Cameras view shows the readout and badge too.
- **Per camera:** exposure and colour modes, ISO (log slider, with the
  production shutter shown next to it; the shutter is editable in manual
  exposure when it is free), white balance temperature and tint. Every
  control of an online camera is editable, within its descriptor
  (`readonly`, `enabled_when`, `locked_while_live`).
- **Copy colour to all:** sets this camera's `wb_temperature` and `wb_tint`
  on every other online camera.
- **Match to camera N:** in phase 1 the same as copy, but for one camera.
  Kelvin values do not give the same colour on different phone models (each
  ISP converts differently), so between models this is only a starting
  point. A measured match (both cameras see the same grey card) comes with
  the auto-shader (D8).
- **Production bar:** mains and shutter, usable whenever the page is
  connected to the hub. While a camera is live it notes that live cameras
  apply a change when their stream stops.

## Auto-shader (phase 2: design only)

The hub sees all cameras, so it can make them agree:

- Enabled per camera and group on the hub. It writes with `set` like a
  person; the last write wins, so a person who changes a camera it steers
  stops it for that camera (the hub sees the change in `state`).
- **Exposure:** steer each camera's ISO (or `ev_bias` on auto ISO) so that a
  chosen statistic (mid-tone level `p50`, or the level of a chosen region) is
  the same on all cameras, or the same as a reference camera.
- **Colour:** from `neutral_rg` and `neutral_bg`, only when
  `neutral_fraction` is large enough to trust. Cameras that see different
  scenes cannot be matched by a grey-world average. The likely workflow is a
  calibration step (all cameras on one grey card), then lock, then hold
  (D8).
- **On air:** a camera on program tally is changed slowly (at most 1/6 EV per
  second, no colour steps) or frozen. Corrections go in while it is off air
  (D12).
- Rate limit and hysteresis, so the picture does not pump on a moving scene.

## Phone, hub and Strom: who does what

| Phone | Hub (standalone, or embedded in Strom) | Strom (downstream) |
|---|---|---|
| ISO, white balance, focus, zoom on the sensor | Shader view, copy and match | Black level (pedestal), gamma, knee, saturation |
| Auto exposure (core auto ISO loop), AWB, AF | Production settings (mains, shutter), kept across restarts | Lift, gamma, gain per camera, LUTs |
| Readout, exposure warning | Readout and warnings per camera | Matching what the phone cannot (for example mixed models), on the decoded picture |
| Image statistics (core) | Auto-shader (phase 2) | Tally source (vision mixer), as today |
| Operator aids: guides, peaking, meters, screen lock | | |

Black level, gamma and knee cannot be set in the iPhone's capture pipeline
(AVFoundation offers none of them for live video). They belong downstream,
in Strom, as a per-camera colour correction in the flow. That is not
designed here. When it exists, the shader view in Strom can show those
controls next to the phone's.

## Decisions

Taken on 2026-10-16, D13 on 2026-10-18. The references above point here.

- **D1. Bright light: a free shutter.** At 1/50 and f/1.8 a sunny exterior
  needs about ISO 1, far below the sensor's minimum (32 or so), and without
  ND the picture clips. With the shutter free (policy `free`, or mains 0),
  auto (the core loop) holds 180° and shortens the shutter only once ISO is
  at its minimum, and manual exposure may set it. With `mains` at 50 or 60
  Hz, or a fixed value, ISO stays the only exposure control.
- **D2 to D4** (escape hatch while disconnected, ownership across restarts,
  no consent prompt for a take over): dropped with ownership (D13).
- **D5. Lens and zoom** stay shared between the operator and the hub, as
  every control now is.
- **D6. Where a production lives.** The standalone hub is one production.
  In Strom the production settings belong to the flow: cameras bound to a
  block in that flow get the flow's settings. A camera not bound to any flow
  gets Strom's default (mains from the configured region, shutter `mains`).
- **D7. Frame rate per production: later,** with the same live lock. Not in
  phase 1.
- **D8. Colour across models.** Phase 1 copies values. A measured match
  (all cameras on one grey card, then lock) comes with the auto-shader. No
  per-model colour table.
- **D9. Image statistics in phase 1,** 1 per second by default, up to 4 on
  request, with histogram and waveform both (under 1 KB per message).
- **D10. Focus peaking as an overlay** drawn from the video output at a low
  rate on top of the system preview layer. No Metal preview in phase 1.
- **D11. Refuse while live.** Resolution, frame rate and codec changes while
  live are refused, not held as pending.
- **D12. Auto-shader on program: slow, not frozen.** At most 1/6 EV per
  second on exposure and no colour changes while on program; full speed off
  air.
- **D13. No ownership; the last write wins.** The shader and the camera
  operator can both change exposure, colour and focus at any time, and
  neither locks the other out. Ownership (an owner per group, take over and
  release, remote groups read-only on the phone) cost more in confusion than
  it saved in fights. Production settings and image statistics stay.

## Phase 1 plan (after review)

In this order, each one tested before the next:

1. Core: message types (`production`, `stats`, `image_stats`), protocol
   version 2.
2. Phone: production shutter, `exposure_mode` auto as auto ISO, and the live
   lock on format and codec.
3. Phone UI: the readout first, then the guides, zoom and lens, focus lock
   and peaking, audio meter, screen lock.
4. Image statistics (core function, phone sampling) and the exposure
   warning.
5. Hub: production settings (accepted any time, persisted) and the stats
   preference.
6. Hub shader view (side by side, copy and match).

The auto-shader is not built in phase 1.
