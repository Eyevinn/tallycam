# Shading: exposure, colour and focus control

Status: design agreed 2026-10-16 (decisions at the end). Phase 1 in progress.
PROTOCOL.md gets the message definitions as they are built, and the code
becomes the source of truth for how this works.

## Goal

Exposure and colour must look right in a production that has no shader (the
person who sets iris and colour on every camera from one desk). A central
shader must still be possible when there is one. The shader is optional and
never required.

Today the phone has per-control modes (`exposure_mode`, `wb_mode`,
`focus_mode`: auto, locked, manual), and any server `set` can change any of
them at any time. Nothing says who is in charge of a camera's picture. The
camera operator and a shader can fight without either one seeing it. This
design adds that missing piece: an **owner** per group of controls.

## Control groups

Ownership applies to three groups per camera. The group of a control is the
`group` in its descriptor, which the phone already sends.

| Group | Controls | Commands |
|---|---|---|
| `exposure` | `exposure_mode`, `iso`, `ev_bias` | the exposure side of `focus_at` (metering point) |
| `white_balance` (shown as "Colour") | `wb_mode`, `wb_temperature`, `wb_tint` | `wb_one_shot` |
| `focus` | `focus_mode`, `focus_position` | `focus_at` |

Not in any group:

- **Shutter** becomes a production setting (see "Shutter").
- **Lens, zoom and stabilisation** stay as they are: the camera operator and
  the server both set them. Lens and zoom are not a
  group in phase 1 (D5).
- **Resolution, frame rate and codec** cannot be changed while the camera is
  live, from the phone or from the server (see "Locked while live").

## The three modes

Each group, on each camera, has exactly one owner at a time.

| Owner | Who sets the values | Phone indicator | Phone controls for the group |
|---|---|---|---|
| **auto** (default) | the phone's own auto exposure, auto white balance and autofocus | `AUTO` | usable; touching one makes the group local |
| **local** | the camera operator on the phone (locks, or sets values) | `LOCKED` | usable |
| **remote** | the hub: a person at the shader view, or later the auto-shader | `REMOTE`, or `REMOTE · disconnected` | read-only, values shown |

Auto needs no setup at all, and a phone that never meets a hub stays in auto
or local. Remote is the only owner that comes from outside the phone.

What each owner may use:

- **auto**: `exposure_mode` auto (the core's auto ISO loop at the production
  shutter, see "Shutter"), with `ev_bias` 0. `wb_mode` and `focus_mode` auto.
- **local**: `locked` or `manual` modes and their values, plus `ev_bias` on
  top of auto exposure. A bias other than 0 counts as local: the operator
  chose it.
- **remote**: anything the phone offers, including the auto modes. A shader
  may own exposure and leave it on auto ISO with a bias, so the camera keeps
  following the light but at a level the shader picked.

## Ownership state machine

Per group, on the phone:

```
                 operator locks or sets a value
        ┌──────────────────────────────────────────────┐
        │                                              ▼
     ┌──────┐   operator taps AUTO              ┌───────────┐
     │ AUTO │ ◀──────────────────────────────── │   LOCAL   │
     └──────┘                                   └───────────┘
       ▲  │                                            │
       │  │ hub: own                         hub: own  │
       │  ▼                                            ▼
       │ ┌──────────────────────────────────────────────────┐
       │ │                     REMOTE                       │
       │ │   connected ◀──── reconnect ────  disconnected   │
       │ │             ──── link lost ────▶  (values held)  │
       │ └──────────────────────────────────────────────────┘
       │        │ hub: release              │ new control server
       └────────┴───────────────────────────┘ provisioned (scan)
```

Rules:

1. **One owner per group.** The phone is the only source of truth for who
   owns what (as with all state in LCP), and reports it in `ownership`.
2. **Remote takes over only by an explicit `own`** from the hub, sent when a
   person presses Take over (or the auto-shader starts, in phase 2). A plain
   `set` never takes a group: a `set` on a group the remote does not own is
   refused for that group (`result` `ok: false`, "exposure: not under remote
   control"). The same `set` still applies its other keys.
3. **Remote may take over from local.** The phone shows a banner ("Exposure
   taken over by Shader") so the operator knows why the controls stopped
   working. No consent prompt for now (D4).
4. **Release goes back to auto,** never to the last remote value or to the
   previous local values. Exposure goes to auto with bias 0, colour to auto
   white balance, focus to continuous autofocus. A release is explicit (`release`
   from the hub).
5. **Another remote holder may take over** a group held by a remote holder,
   explicitly (a second shader, or a person taking from the auto-shader). The
   hub tells the previous holder.
6. **Hub connection lost: keep the last remote values.** The group stays
   remote with the values it had, and the phone shows `REMOTE ·
   disconnected` for each remote group. It never jumps back to auto in the
   middle of a show. Auto exposure that the remote left running (auto ISO
   with a bias) keeps running. "Lost" means the control channel is closed or
   silent for 10 s (the existing offline rule).
7. **Reconnect:** after `hello` the phone sends `ownership` as it is. The
   groups are still remote, with the holder it had. The hub takes that as
   the truth, also after a hub restart (the hub keeps no ownership of its
   own). The holder may be gone. The hub then shows "held by Shader
   (offline)", and any controller can take over or release.
8. **A new control server** (a QR code from another hub scanned) releases
   every remote group to auto. A different server is a different
   production.
9. **The camera operator cannot take a remote group back while connected.**
   While disconnected, see D2.

Phase 2 adds the **auto-shader** as one more remote holder (`id:
"auto-shader"`). It follows the same rules: it starts with `own`, a person can
take over from it, and its release goes to auto.

## LCP messages

Protocol version 2: `hello.protocol: 2` says the phone understands ownership.
Version 1 phones keep working against a version 2 hub, without ownership. The
hub then shows no Take over buttons for them and forwards `set` as today.

Breaking change, intentional: a version 2 phone refuses `set` on group
controls it does not hold for the remote, so a version 1 hub (or operator
page) can no longer change exposure, colour or focus on it. App and hub ship
from the same repository, so this is acceptable before LCP v1. The fix is to
update the hub.

### Server → device

```json
{ "type": "own", "id": 21, "groups": ["exposure", "white_balance"],
  "holder": { "id": "c7f3a1", "label": "Shader" } }

{ "type": "release", "id": 22, "groups": ["exposure"] }

{ "type": "production", "mains_hz": 50, "shutter": "mains" }
```

- `own`: the remote takes the listed groups. The values stay as they are at
  that moment (no jump). The hub follows with a `set` if it wants other
  values. A group the phone does not have (no focus control on a fixed-focus
  camera) is skipped and named in `result.error`, with `ok: false`, while the
  other groups are taken: the same rule as a clamped `set` (applied, `ok:
  false` with the reason).
- `release`: the listed groups go to auto (rule 4). Releasing a group that
  is not remote does nothing and is `ok`.
- `holder.id`: chosen by the controller, opaque to the phone, at most 64 bytes.
  `holder.label`: shown on the phone ("REMOTE · Shader"), at most 64 bytes.
- `production`: see "Shutter". Sent after admission and when it changes.

### Device → server

```json
{ "type": "ownership",
  "groups": {
    "exposure":      { "owner": "remote", "holder": { "id": "c7f3a1", "label": "Shader" } },
    "white_balance": { "owner": "local" },
    "focus":         { "owner": "auto" } } }
```

- Sent in full right after the full `state` on admission, and again in full on
  every change (it is small). Only groups the phone has are listed.
- `state` keeps reporting the values and modes as today, so a UI shows
  `exposure_mode: manual, iso: 400` next to `owner: remote`.

### Controller ↔ hub

- `own` (`device`, `groups`, `holder {id, label}`) and `release` (`device`,
  `groups`) are forwarded to a protocol 2 device. For a protocol 1 device
  the hub refuses them ("update the app"). The holder id must not be empty,
  and id and label are at most 64 bytes.
- `set` and `command` may carry `holder`; the hub removes it before
  forwarding. For a protocol 2 device the hub refuses the whole request when
  any key's group is not held by that holder ("exposure is held by Shader 2:
  take over first", "white_balance is not under remote control: take over
  first"). Keys outside the groups pass. Protocol 1 devices get requests
  unchanged.
- `stats` (`device`, `enabled`, `fps?`) sets that device's preference. The
  hub sends `stats` to protocol 2 devices only while a controller is
  connected.
- `production` (`mains_hz`, `shutter`, no `device`) changes the production;
  refused while any camera is live or connecting. The hub broadcasts it to
  every controller and puts it in `snapshot`.
- Refusals are `error` messages to the sending controller only.
- The hub forwards `ownership` as `device_ownership` and `image_stats` as
  `device_image_stats`. The snapshot carries each device's `protocol`,
  `ownership` and latest `image_stats`.
- The embedding server's typed calls (`Hub::set`, `Hub::command`) take no
  holder yet; the phone still enforces ownership. Strom adds the holder from
  its user session when it gets a shader view.

### What a protocol 2 device must do

A device that sends `hello.protocol: 2` (iOS, Android, fake devices) must:

1. Run the core's ownership state machine (`tallycam_core::ownership`) for
   the groups it has, persisted across restarts (D3).
2. After admission, send `state` in full, then `ownership` in full. Send
   `ownership` again, in full, whenever an owner or holder changes.
3. On `own`: take the groups, keep the values, answer `result` (with the
   skipped group names in `error` if any). Show a banner for groups taken
   from local.
4. On `release`: put the released groups into auto on the camera (exposure
   auto with bias 0, auto white balance, continuous autofocus), answer
   `result`.
5. On `set` and `command`: check every key's group (`control_group`,
   `command_group`) with `check_remote`. Refuse refused keys by name in
   `result.error`, apply the rest. Keys outside the groups work as before.
6. On local touches: call `local_change` or `local_auto`, and do nothing if
   they refuse. Keep the group's controls read-only in the UI while remote.
7. On `production`: apply `mains_hz` and the shutter, or keep them until the
   stream stops if live.
8. Call `set_connected` on admission and when the control channel closes,
   and `new_server` when a new control URL is provisioned.

A version 1 device needs none of this. The hub accepts both versions, and
warns only about a version newer than its own.

### Test vectors

`core/tests/vectors/lcp_ownership.json`, used by the core's Rust tests and by
the iOS tests (the Android tests later):

- Message round trips for `own`, `release`, `ownership` and `production`,
  including unknown fields that must be ignored.
- State machine scenarios as event sequences, each with the expected
  `ownership` after each step: own from auto, own from local, release, set
  refused on a group not held, link lost then reconnect (values held), new
  server provisioned, take over by a second holder.

The state machine is control logic that must behave the same on every
platform, so it goes into the Rust core (ARCHITECTURE.md, "What goes into the
core"). Applying a group's values to the camera stays native.

## Shutter

iPhones have a fixed aperture and no ND filter. The shutter is a flicker
setting, not an exposure knob: under mains-powered light, any shutter that
is not a whole number of light pulses flickers. So:

- **The shutter is a production setting,** not a per-camera control. The
  default is the mains period: 1/50 at 50 Hz, 1/60 at 60 Hz. Options: `mains`
  (default), a fixed flicker-free value (`100` for 1/100, for 100 fps or
  sharper motion), or `free` (no mains-lit light in the picture, see D1).
- `production.mains_hz` replaces the per-phone `mains_hz` control when a hub
  supplies it. Without a hub, the phone is its own production: mains and
  shutter are in the phone's Settings (default from the region, as today).
- **Not changeable live.** The hub refuses to change `production` while any
  of its cameras is live. A phone that gets a different shutter while live
  (an older hub, a race) keeps the current one until the stream stops and
  logs a warning.
- **The shutter must fit the frame.** 1/50 does not fit at 60 fps. The phone
  then uses the nearest flicker-free value that fits (1/100), as the core's
  rounding already does. It reports the shutter it uses in `state`, and the
  hub shows the difference as a warning.
- `shutter` stays in `state` as a read-only value.

With the shutter fixed, **ISO is the only exposure control** (the "iris").
So:

- Exposure **auto** is the core's auto ISO loop at the production shutter.
  It already exists for genlock. iOS continuous auto exposure moves the
  shutter (and fights genlock), so it is no longer an exposure mode. Under
  `free` the core loop also shortens the shutter once ISO is at its minimum
  (D1).
- `exposure_mode` options become `auto`, `locked` and `manual`. `auto_iso`
  merges into `auto`. The phone still accepts `auto_iso` as `auto` for one
  version.
- `manual` sets ISO only, and the shutter too under `free`.

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
| **Group indicators** | Three small chips, `EXP`, `WB`, `FOC`, each reading `AUTO`, `LOCKED` or `REMOTE` (`REMOTE · disconnected` in amber). Tap a chip for the group's controls, read-only when remote. Always visible, also with the screen locked |
| **Grid, horizon, frame guides** | Rule of thirds; a horizon line from Core Motion (gravity vector), green when level; 16:9 action and title safe areas, and 4:3 and 9:16 centre-cut guides. Drawn over the preview only, never in the stream |
| **Zoom and lens** | A vertical rocker at the edge for smooth zoom (speed by deflection, for a usable zoom move on air), lens buttons (0.5×, 1×, 2×/3×/5×, those that exist), pinch as well |
| **Focus** | Tap to focus (exists), long press to lock focus (and `LOCKED` on the chip), focus peaking |
| **Audio meter** | A small peak/RMS meter of the microphone, with a clip marker |
| **Exposure warning** | "Too dark" or "Clipping" from the image statistics (below) and the auto ISO limits (`TooDark`, `TooBright`) |
| **Screen lock** | One button locks all touch input except the unlock gesture (a slide). Suggested at Go Live. It blocks touches only: remote control, tally and the stream keep running. Guided Access is still the recommendation against leaving the app |

Tap to focus changes only the focus group, plus the exposure metering point
when exposure is auto. Today it also switches a locked exposure back to
continuous auto exposure. That ends: a tap never changes the exposure owner
or mode.

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

After the phone's phase 1:

- **All cameras side by side,** each with its preview, a waveform or
  histogram from `image_stats` (switchable), clip and crush shares, tally,
  and the three ownership chips.
- **Per camera:** ISO (log slider, with the production shutter shown next to
  it), white balance temperature and tint, Take over and Release per group
  (and both groups at once). Controls are enabled only for groups this
  controller holds. Who holds the others is shown.
- **Copy colour to all:** sets this camera's `wb_temperature` and `wb_tint`
  on every camera that the copying controller holds for colour, or that it
  first takes over after a confirmation.
- **Match to camera N:** in phase 1 the same as copy, but for one camera.
  Kelvin values do not give the same colour on different phone models (each
  ISP converts differently), so between models this is only a starting
  point. A measured match (both cameras see the same grey card) comes with
  the auto-shader (D8).
- The hub keeps no ownership of its own (rule 7). It shows what the phones
  report.

## Auto-shader (phase 2: design only)

The hub sees all cameras, so it can make them agree:

- **Holder** `auto-shader`. It takes the groups it is enabled for, per camera,
  with `own`, and follows every ownership rule. A person taking a camera over
  stops it for that camera.
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
| ISO, white balance, focus, zoom on the sensor | Ownership display, take over and release | Black level (pedestal), gamma, knee, saturation |
| Auto exposure (core auto ISO loop), AWB, AF | Shader view, copy and match | Lift, gamma, gain per camera, LUTs |
| Ownership state machine (core), indicators | Production settings (mains, shutter) | Matching what the phone cannot (for example mixed models), on the decoded picture |
| Image statistics (core) | Auto-shader (phase 2) | Tally source (vision mixer), as today |
| Operator aids: guides, peaking, meters, screen lock | | |

Black level, gamma and knee cannot be set in the iPhone's capture pipeline
(AVFoundation offers none of them for live video). They belong downstream,
in Strom, as a per-camera colour correction in the flow. That is not
designed here. When it exists, the shader view in Strom can show those
controls next to the phone's.

## Decisions

Taken on 2026-10-16. The references D1 to D12 above point here.

- **D1. Bright light: a `free` shutter.** At 1/50 and f/1.8 a sunny exterior
  needs about ISO 1, far below the sensor's minimum (32 or so), and without
  ND the picture clips. With the production shutter `free` (no mains-lit
  light in the picture), the shutter joins the `exposure` group: auto (the
  core loop) shortens it only once ISO is at its minimum, and a remote or
  local owner may set it. With `mains` or a fixed value, ISO stays the only
  exposure control.
- **D2. Escape hatch while disconnected.** While a group is `REMOTE ·
  disconnected`, and only then, a long press on its chip and a confirmation
  take it local. While connected the operator cannot take it back.
- **D3. Ownership survives an app restart.** Owner, holder and the group's
  values are persisted. A relaunch in the middle of a show comes back as
  `REMOTE · disconnected` with the same values, not in auto.
- **D4. No consent prompt for a take over** until trust levels exist
  (PROVISIONING.md). Any admitted hub may take over from local, and the
  phone shows a banner. Studio phones on tripods have nobody to ask. Trust
  levels later gate `own` per invite.
- **D5. No lens group in phase 1.** Lens and zoom stay shared between the
  operator and the hub.
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

## Phase 1 plan (after review)

In this order, each one tested before the next:

1. Core: the ownership state machine and message types (`own`, `release`,
   `ownership`, `production`), protocol version 2, the vectors in
   `core/tests/vectors/lcp_ownership.json`.
2. Phone: ownership applied to `set` and to local touches. Production
   shutter, `exposure_mode` auto as auto ISO, and the live lock on format
   and codec.
3. Phone UI: group indicators first, then the guides, zoom and lens, focus
   lock and peaking, audio meter, screen lock.
4. Image statistics (core function, phone sampling) and the exposure
   warning.
5. Hub: ownership in the controller protocol, holder checks, and the
   production settings.
6. Hub shader view (side by side, take over and release, copy and match).

The auto-shader is not built in phase 1.
