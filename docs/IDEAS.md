# Ideas and direction notes

Thoughts from a review of the prototype, October 2026. Not decisions:
ARCHITECTURE.md holds those. Genlock has its own notes in GENLOCK.md.

## The thesis

iPhones as professional cameras holds for **many professional angles at low
cost**, not for **replacing the A camera**. The picture is good enough in good
light at wide angles; optics (small sensor, fixed aperture, no real zoom),
heat over long sessions, network reliability and audio inputs are the limits.
The missing piece is software: provisioning, remote control, tally, clock
sync and integration with the production. That is what we build, so pitch it
as "every phone becomes a managed, time-synced camera node in your
production".

Where we can differ from Blackmagic Camera, Larix, Moblin and NDI apps:

1. An open protocol across platforms (iOS, Android, edge boxes).
2. Time as a first-class value: TAI per frame, sync without hardware
   (GENLOCK.md).
3. Tight integration with Strom: tally from the vision mixer, cameras as
   blocks in the flow.

## Priorities

1. **Prove the whole chain** before widening: the picture in Strom from a
   real phone, with tally and remote control. A 30-second demo (scan the QR
   code, the camera appears in Strom, cut to it, the phone turns red) sells
   more than the architecture.
2. **The SRT spike** (libsrt over FFI or `srt-protocol`). Without encryption
   nothing goes over the internet, and the 5G story with it. Done: the app
   sends through the core's `srt-protocol` caller, with encryption.
3. **A week on software genlock** (GENLOCK.md), before more is built around
   time. Done on 2026-10-16: locked within ±0.25 ms on an iPhone 16 and an
   iPad 10, with auto ISO.
4. **Genlock calibration per model and format** (GENLOCK.md, "Two cameras"):
   the same capture stamp meant about 11 ms apart on the iPhone 16 and the
   iPad 10. Build an LED reference on the TAI second, measure each model, and
   apply the offset to the SEI stamp and the genlock target.

## Other notes

- **Avoid two servers.** The control hub is a good prototype, but Strom is
  meant to implement the protocol. Move the device registry and tally into
  Strom once the protocol settles.
- **Open the protocol.** A published, versioned LCP with the existing test
  vectors (`core/tests/vectors/`) lets others build devices. That makes it a
  platform rather than an app.
- **Day-to-day operation decides "professional".** iOS stops the camera when
  the app is backgrounded (recommend Guided Access); screen lock,
  notifications and thermal throttling need handling. A start-up checklist
  ("Do Not Disturb, charger in, temperature OK") would help.

## Retired phones

Turn the old phone in the drawer into a remote camera. Also in the pitch.

| Minimum iOS | Oldest iPhone | Cost |
|---|---|---|
| iOS 17 (today) | XS, XR, SE 2 (2018) | none |
| iOS 16 | 8, X (2017) | two API fallbacks: `videoRotationAngle` → `videoOrientation`, `.sensoryFeedback` → `UIImpactFeedbackGenerator` |

Every iPhone in that range encodes H.264 1080p30 in hardware, and an old
flagship's camera beats a new budget Android. Audience: schools, churches,
local sports clubs, small events, local TV. Caveats: worn batteries (run on
power), no security updates on old iOS (the phone only connects out), no 5G
before iPhone 12 (Wi-Fi or USB Ethernet). Not scheduled.

## Android

Cheaper, but fragmented. Many budget phones expose Camera2 only at
`LEGACY`/`LIMITED` (no real manual ISO or shutter), and MediaCodec encoders
vary in rate control and latency. Strengths: kiosk mode (device owner),
foreground services, USB-C Ethernet on every model. The phone describes its
own controls (`CameraControlTable.swift`), so a weak phone advertises fewer
of them and still works. Keep a list of tested models (Pixel, Samsung S/A)
and degrade at runtime.
