# The apps: one design, every platform

Status: October 2026. The rule, and where the apps stand against it.

## The rule

TallyCam is one product on several platforms, not one app per platform. The
iOS app and the Android app (and later ones) have the same design and the
same features. An operator who knows one knows the other, and a hub or Strom
cannot tell them apart except by `hello.device.platform`.

- **Same protocol.** Same control ids, kinds, ranges where the hardware
  allows, the same `enabled_when`, the same commands and the same status
  fields (docs/PROTOCOL.md). A control a camera lacks is left out, never
  renamed.
- **Same screens.** The same layout, words, colours and order: the shooting
  screen and Settings below. Platform conventions win only where users
  expect them (Android's back gesture, notification, permission prompts).
- **Same core.** Clock, timestamps, MPEG-TS, SRT, genlock, auto ISO, device
  keys, camera defaults and protocol types come from the Rust core, never
  rewritten per app. What is native is capture, encoding and the UI.
- **A difference is written down.** Anything one app has and the other lacks
  is listed in the table below, with the reason, until it is gone. A change
  to one app's design or features updates this file, and the other app
  follows or the gap is listed.

## The shooting screen

The design, zones and measures are in docs/UI_DESIGN.md; both apps follow
it. In short, information in the middle and controls at the sides, on one
grid (iOS `Chrome`, Android the same names):

| Zone | Both apps |
|---|---|
| Picture | Full bleed, 16:9; tap to focus and expose at that point (`focus_at`, focus back to auto, never the exposure mode); long press to lock focus there (AF-L). Aids drawn over it only |
| Tally | A 14 pt border, red on program, green on preview; none when off. All chrome stays inside it and the safe areas |
| 1 Status row | One translucent line on top: a dot and STANDBY / CONNECTING / LIVE (with the bitrate while live), the camera name chip in the middle ("ON AIR · Kamera 2" in red, "PREVIEW · …" in green, the name alone in a dark chip when tally is off; on its own line under the status line on a narrow screen), the hub link dot, the clock dot, the battery, and the Settings button. iOS also shows the thermal state when it matters and an icon while an orientation change waits |
| 2 Values row | Centred under it, always shown: format ("1080p30"), ISO, shutter, EV (auto, not 0), white balance in K with the tint when not 0 ("5200 K +4"), focus (AF, AF-L, F 0.42). Each value is a button: ISO, shutter and EV choose exposure for the adjust control, K white balance, focus focus, the format opens Settings; the chosen group is underlined. Yellow when manual or locked; the exposure warning colours ISO and follows it ("ISO 26 CLIP" red, "DARK" orange); a value the hub just set (`set`) flashes cyan for a second. On a narrow screen focus goes first, then white balance; never wraps |
| 3 Message slot | Centred under the values row: operator messages (yellow, 8 s) and toasts (stream errors, hints, 4 s); nothing else shows messages |
| 4 Left edge | Lens buttons (longest on top) above the zoom rocker, vertically centred; beside it on a landscape phone too low for both |
| 5 Right edge | The adjust control, always shown, mirroring zoom with the rockers level: a vertical mode switch (`A` over `M`, where `M` is any held mode: manual, locked, or a lock on a camera without manual; the filled segment is the mode, a tap on the other switches, keeping the values), the rocker (the zoom rocker's component and length: speed by the square of the deflection, springs back, letting go stops) and the value. Exposure ramps `ev_bias` in auto, ISO on a log scale otherwise; white balance `wb_temperature`, or `wb_tint` with the K / Tint switch (K each time it is chosen), switching to manual first, and a long press on the mode switch is "white balance once"; focus ramps `focus_position` (up is far), switching to manual first. The group is remembered; exposure until another is chosen. Sets go out at most 10 times a second, within the descriptor's range and step |
| 6 Bottom row | Left: the aids menu (grid, horizon level, safe areas 93 % / 90 %, centre cuts 4:3 and 9:16, focus peaking in red, each persisted), the QR scanner while no hub is connected and not live, the screen lock. Right: the microphone meter, then the live button in the bottom-right corner (landscape; bottom centre under the row in portrait), at a fixed place. The live button: dark at standby, orange connecting, red live, held to stop with a hint beside it; a haptic when it goes live or stops |
| Screen lock | Everything stays visible but takes no touches, the rockers hide, and a slide to unlock takes the bottom row's centre (suggested in a toast at Go Live) |
| Not connected | A centred card: "Not connected", "Scan QR code" (the in-app scanner; the system Camera app works too), "Enter manually", "Not now" |

## Settings

Same sections, order and words: **Camera name** (the field, the default as
placeholder), **Exposure** (mains 50 Hz / 60 Hz / Off, the phone's own
production; disabled with "Set by the control hub" while a hub's production
applies, and while live), **Control hub** (link state and host, the URL, "Disconnect and
forget server" with a confirmation), **Stream (SRT)** (the URL, editable only
while idle; video codec and bitrate; round trip and losses while live),
**Orientation**, **Clock** (health, synchronised, estimated and maximum
error, reference, stratum, sources reachable, findings), **About** (app
version, camera name, model, OS version, stream id, the device key's first
16 hex characters).

## Icon

One icon (ios/TallyCam/Assets.xcassets/AppIcon.appiconset/AppIcon-1024.png):
on Android an adaptive icon with that picture at two thirds of the canvas on
its own dark (#16181E), so launchers' masks keep the tally dot.

## Where they differ today

| Feature | iOS | Android | Why, and what closes the gap |
|---|---|---|---|
| Portrait streaming (`orientation`) | Yes | Landscape only | Needs rotating frames before the encoder (a GPU pass); Settings says so |
| Either landscape side | The picture stays upright | One side only (the app is locked to landscape) | Camera2 hands the encoder sensor frames unrotated; the same GPU pass closes it |
| Exposure `auto` (the core's auto ISO at the production shutter) | Metered by AVFoundation (`exposureTargetOffset`) | Metered from the picture: mean luma of the small YUV stream against mid grey (a fifth of the frame around a tapped point at ¾ weight). Cameras without `MANUAL_SENSOR` (the emulator) use Camera2's auto exposure | Camera2 gives no metered offset with its auto exposure off. Not yet run on a phone: the metering needs checking on a Pixel and a Samsung |
| Genlock | In every exposure mode | In every exposure mode with `MANUAL_SENSOR` | Camera2 applies a frame duration only with its auto exposure off, which it always is there |
| `wb_temperature` | Any Kelvin | The nearest AWB preset (2700 to 7500 K) | Camera2 offers presets; colour gains per sensor would need calibration |
| `wb_tint` | Yes | No | As above |
| White balance `M` | Manual | A lock: what auto white balance found (the rocker still sets a preset, which is manual) | Manual white balance is a Camera2 preset, so holding the auto result means locking it |
| `torch_probe` | Yes | No | A measurement tool (docs/GENLOCK.md); add when Android genlock is measured |
| `frame_duration` (Debug) | Yes | No | A measurement tool |
| Protocol 2 (production, image statistics, live lock) | Yes | Yes | Same core statistics; tested on the emulator against the hub |
| Microphone meter | Always (the microphone runs with the camera) | While live only | Android opens the microphone for the stream only: holding it at standby costs power and keeps the system's microphone indicator on |
| Focus peaking source | The streamed frames, 9 per second | The small YUV stream (about 10 frames per second, the same as the statistics) | The same 16:9 picture; nothing extra for the ISP |
| Image statistics source | Every frame offered, measured up to 4 per second | The small YUV stream, which Camera2 fills on about 10 frames per second (a repeating burst), measured up to 4 per second | Power: the ISP writes the extra stream only on the frames that are sampled |
| Device key store | Secure Enclave | Android Keystore (StrongBox where present) | Platform equivalents |
| Thermal state in the status line | When serious | Not shown (reported in `status.thermal`) | To add |
| Background streaming | While the app is in front | A foreground service (screen off, other app in front) | iOS stops the camera in the background |

## How to keep it

- A pull request that changes one app's screens, words or features changes
  the other app too, or adds the gap to the table above.
- New controls and commands go into docs/PROTOCOL.md first, with the same id
  on every platform.
- Logic that both apps need goes into the core (`core/`), with test vectors
  in `core/tests/vectors/` where another implementation could get it wrong.
