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

| Part | Both apps |
|---|---|
| Picture | Full bleed, 16:9; tap to focus and expose at that point (`focus_at`) |
| Tally | A 14 pt border, red on program, green on preview; none when off |
| Camera name | Always shown, large, in a chip at the top under the status line: "ON AIR · Kamera 2" in red, "PREVIEW · …" in green, the name alone in a dark chip when tally is off |
| Operator message | A yellow banner under the chip, for 8 s |
| Status line | One translucent line on top: a dot and STANDBY / CONNECTING / LIVE (with the bitrate while live), the hub link dot (green connected, orange connecting, grey not set up), the clock dot (green healthy, orange degraded, red bad), the battery, and the Settings button. iOS also shows the thermal state when it matters and an icon while an orientation change waits |
| Live button | A ringed circle on the right edge in landscape: dark at standby, orange while connecting, red while live; a haptic when it goes live or stops |
| Not connected | A card: "Not connected", how to scan the hub's QR code |

## Settings

Same sections, order and words: **Camera name** (the field, the default as
placeholder), **Control hub** (link state and host, the URL, "Disconnect and
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
| Auto ISO (`exposure_mode` `auto_iso`) | Yes | No | Camera2 gives no metered offset with auto exposure off; needs metering from the picture, to build with a real phone |
| Genlock | With auto ISO or manual exposure | Manual exposure only | Camera2 applies a frame duration only with auto exposure off; follows auto ISO |
| `mains_hz` | Rounds the auto ISO shutter | The auto exposure's anti-banding | No auto ISO on Android yet |
| `wb_temperature` | Any Kelvin | The nearest AWB preset (2700 to 7500 K) | Camera2 offers presets; colour gains per sensor would need calibration |
| `wb_tint` | Yes | No | As above |
| QR scanning | The system Camera app | The system camera, or the in-app scanner | Not every Android camera app opens `tallycam://` links |
| `torch_probe` | Yes | No | A measurement tool (docs/GENLOCK.md); add when Android genlock is measured |
| `frame_duration` (Debug) | Yes | No | A measurement tool |
| Adaptive bitrate | In progress | No | Follows once it lands in the core and iOS |
| Protocol 2 (ownership, docs/SHADING.md) | In progress | No (sends 1; the hub accepts it) | Through the core's state machine once iOS has it |
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
