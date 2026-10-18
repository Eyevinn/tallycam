# TallyCam

TallyCam turns iPhones, iPads and Android phones into cameras for live
production. A phone scans a QR code, streams camera and microphone over SRT,
shows tally from the vision mixer, is remote controlled from the gallery
(exposure, white balance, focus, lens, format, codec, bitrate), and stamps
every frame with its capture time from its own disciplined clock. With
software genlock, phones line up their frames on a shared time grid without
cables.

It is built for [Strom](https://github.com/Eyevinn/strom), which embeds the
server side, and also runs on its own with the standalone control hub.

## Status

A prototype, not released and not stable. Protocol, APIs and storage formats
may change without notice. What works today:

- **iOS** (iPhone 16, iPad 10): H.264 or HEVC up to 2160p60, Opus or AAC, over
  SRT with AES; about 45 to 50 ms from capture to the network plus the SRT
  latency ([LATENCY](docs/LATENCY.md)).
- **Android**, a first version: the same design and features on the same
  core, tested on the emulator so far ([APPS](docs/APPS.md)).
- **Adaptive bitrate** in the core: the bitrate follows the network, up to
  the operator's.
- **Provisioning** by QR code, optionally invite-only with device keys;
  remote control and tally from the hub or from Strom. The camera operator
  and the hub both change any value; the last change wins.
- **Time**: a chrony-like clock on the phone, TAI capture time in every
  frame, and software genlock within ±0.25 ms ([GENLOCK](docs/GENLOCK.md)).

## Quick start

1. Start the control hub on a computer on the phone's network:
   `cd control-hub && cargo run --release`.
2. Open http://localhost:8090. It shows a QR code with the hub's address.
3. Scan it in TallyCam ("Scan QR code"), or with the phone's Camera app, and
   join the hub. The camera appears in the hub with preview, tally, status
   and its controls.
4. Go Live, from the phone or the hub. The phone streams SRT to the hub's
   host on port 9000, or to any SRT listener such as a Strom SRT input.

Building the apps, the hub's options, a simulated phone and testing sync
without phones: [BUILD](docs/BUILD.md).

## What is in the repository

| Path | What |
|---|---|
| `ios/` | The iOS and iPadOS app (Swift) |
| `android/` | The Android app (Kotlin, Jetpack Compose, Camera2, MediaCodec) |
| `core/` | The shared Rust core: clock, timestamps, MPEG-TS, SRT, genlock, adaptive bitrate, device keys, protocol types; Swift and Kotlin bindings |
| `hub/` | The server side of the Live Contribution Protocol (LCP) as a library, which Strom embeds, with the operator web page |
| `control-hub/` | The same hub as a standalone server, and `fake_device`, a simulated phone |
| `tools/` | Timing and sync tools that need no phone |
| `scripts/` | Builds of the core for Apple and Android; TestFlight upload |

## Documentation

| Document | About |
|---|---|
| [BUILD](docs/BUILD.md) | Build, run and test |
| [ARCHITECTURE](docs/ARCHITECTURE.md) | The direction: layers, what is native and what is shared, how TallyCam fits Strom and Open Live |
| [APPS](docs/APPS.md) | One design and one feature set on every platform, and where the apps differ today |
| [UI_DESIGN](docs/UI_DESIGN.md) | The phone's shooting screen and Settings, the same on iOS and Android |
| [PROTOCOL](docs/PROTOCOL.md) | The Live Contribution Protocol (LCP) v0: provisioning, device keys, messages, controls, timestamps |
| [SHADING](docs/SHADING.md) | Exposure, colour and focus control, with or without a shader: the production shutter, auto ISO, image statistics, the hub's shader view |
| [PROVISIONING](docs/PROVISIONING.md) | A draft of provisioning v1 |
| [LATENCY](docs/LATENCY.md) | Where the time goes, from glass to receiver, with measurements |
| [GENLOCK](docs/GENLOCK.md) | Software genlock: research, implementation notes and results |
| [STROM_INTEGRATION](docs/STROM_INTEGRATION.md) | The design for TallyCam cameras in Strom |
| [CAMERA_READOUTS](docs/CAMERA_READOUTS.md) | What camera apps and shading panels show and how they adjust, research behind the phone's values row |
| [DEVICE_CAPABILITIES](docs/DEVICE_CAPABILITIES.md) | Phone capabilities worth using (HDR, gyro, timecode and more) |
| [IDEAS](docs/IDEAS.md) | Direction notes and the feature order, not decisions |

## License

MIT, see [LICENSE](LICENSE). The first MPEG-TS muxer was adapted from
[Moblin](https://github.com/eerimoq/moblin) (MIT), which builds on HaishinKit
(BSD-3-Clause); it has since been replaced by the core's own. Dependencies and
their licenses: [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
