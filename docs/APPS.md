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
- **Same screens.** The same layout, words, colours and order
  (docs/UI_DESIGN.md). Platform conventions win only where users
  expect them (Android's back gesture, notification, permission prompts).
- **Same core.** Clock, timestamps, MPEG-TS, SRT, genlock, auto ISO, device
  keys, camera defaults and protocol types come from the Rust core, never
  rewritten per app. What is native is capture, encoding and the UI.
- **A difference is written down.** Anything one app has and the other lacks
  is listed in the table below, with the reason, until it is gone. A change
  to one app's design or features updates this file, and the other app
  follows or the gap is listed.

## Screens

The shooting screen and Settings are specified in docs/UI_DESIGN.md; both
apps follow it, on one grid of shared measures (iOS `Chrome`, Android the
same names). Where an app cannot, the table below says so.

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
| White balance in the values row while auto | The temperature in K | "AWB" | Camera2 reports no colour temperature for its auto white balance; the K shows once white balance is manual (a preset) |
| In-app QR scanner | An overlay over the picture: viewfinder square, instruction, Cancel | The zxing scanner activity: viewfinder and the instruction, back to cancel | The overlay would need the camera's frames for decoding while the preview runs; to close with a decoder on the small YUV stream |
| White balance `M` | Manual | A lock: what auto white balance found (the rocker still sets a preset, which is manual) | Manual white balance is a Camera2 preset, so holding the auto result means locking it |
| `torch_probe` | Yes | No | A measurement tool (docs/GENLOCK.md); add when Android genlock is measured |
| `frame_duration` (Debug) | Yes | No | A measurement tool |
| Microphone meter | Always (the microphone runs with the camera) | While live only | Android opens the microphone for the stream only: holding it at standby costs power and keeps the system's microphone indicator on |
| Focus peaking source | The streamed frames, 9 per second | The small YUV stream (about 10 frames per second, the same as the statistics) | The same 16:9 picture; nothing extra for the ISP |
| Image statistics source | Every frame offered, measured up to 4 per second | The small YUV stream, which Camera2 fills on about 10 frames per second (a repeating burst), measured up to 4 per second | Power: the ISP writes the extra stream only on the frames that are sampled |
| In-app QR scanner | Over the app's own picture (a metadata output on the capture session, so it also works on A10 devices such as the iPad 6, where VisionKit's scanner does not) | A separate scanner screen (`zxing-android-embedded`) | A scanner fed by the app's own frames would close it |
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
