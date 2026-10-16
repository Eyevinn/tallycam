# Architecture direction

Status: direction agreed in October 2026, for a prototype. It records the
decisions and the reasons, not how the code works. The code is the source of
truth for the implementation.

## What we are building

Not a phone app as such, but a **device role**: a device that is provisioned,
keeps its own disciplined clock, describes its controls, takes remote control
and tally, and sends media to Strom. The phone is the first device. Android,
desktops, small capture boxes and browsers can take the same role.

The range of use goes from crowd contribution (anyone scans a QR code and
sends a stream) to a studio of phones on tripods, logged in, time synced and
controlled from one place. The phone always gives its consent to a level of
control, and always shows who controls it.

## Layers

```
                ┌──────────────── Rust core ─────────────────┐
                │ provisioning · control protocol · clock     │
                │ discipline · SEI timestamps · MPEG-TS · SRT │
                └──────────────┬──────────────────────────────┘
       ┌───────────────┬───────┴────────┬──────────────────┬──────────────┐
   iOS (Swift)    Android (Kotlin)   Desktop          Capture box       Browser
   AVFoundation   CameraX            Strom edge       GStreamer         WASM core
   VideoToolbox   MediaCodec         (GStreamer)      (e.g. HDMI in)    + WHIP
```

The media path splits into three layers:

| Layer | Phones | Desktop / box |
|---|---|---|
| 1. Capture and encode, camera control | Native (AVFoundation + VideoToolbox, CameraX + MediaCodec) | GStreamer |
| 2. Packaging and transport (MPEG-TS, SRT, SRTLA, WHIP) | Rust core | GStreamer elements or the Rust core |
| 3. Device role (provisioning, control, clock, SEI) | Rust core | Rust core |

### Decisions

- **Capture and encode stay native on phones.** Camera control (manual ISO,
  shutter, white balance, focus, lens choice) is the point of the app, and only
  the native APIs give it. GStreamer's mobile sources expose almost none of it.
- **No GStreamer in the phone apps.** It would not help with layer 1, its iOS
  SDK links statically (LGPL in an App Store app is a grey area), and it adds
  tens of MB and a cerbero build for what the core does in a few thousand
  lines.
- **One Rust core, shared by every platform.** Protocol types, the clock
  discipline and the SEI timestamp code exist once, so the phones, the tools
  and Strom cannot drift apart. Swift and Kotlin bindings come from UniFFI.
  Calls made per frame use a cheap path without allocation.
- **What goes into the core, and what does not (agreed 2026-10-16).** Into
  the core: behaviour that must be identical on every platform (protocol,
  handshake, timestamps and SEI, MPEG-TS, SRT, clock), logic that is hard to
  get right and is best tested once (control loops such as genlock and auto
  ISO, connection state machines), and code large enough that writing it
  twice is real waste. Native: thin glue to the OS (camera, encoders, audio,
  Secure Enclave or Keystore, lifecycle, battery), code built on the
  platform's own types that would have to be mirrored across FFI, small
  logic that is cheaper to write twice than to call, and anything that
  would cross the boundary often with a lot of data for no gain. Not
  everything moves just because it could.
- **The core is platform neutral.** Host clock, network, DNS, timers and
  storage are supplied by each platform. The pure logic compiles to
  `wasm32-unknown-unknown`.
- **MPEG-TS: our own Rust muxer.** It is small and well bounded, and GStreamer
  and ffmpeg serve as the reference to test against.
- **SRT: do not write our own; `srt-protocol` in the core.** Retransmission,
  flow control, encryption and interop with libsrt are hard to get fully
  right, and encryption is required over the internet. A spike in October 2026
  chose the pure-Rust `srt-protocol` crate over libsrt through FFI: it
  interoperates with libsrt (plain, AES-128/192/256, key refresh, streamid,
  loss, receiver restart, reconnect within a second) and needs no C build for
  iOS and Android. The crate is unmaintained since 2024, and our driver layer
  works around its faults (handshake timestamp, timer retransmits, panics on
  some control packets, a tiny send buffer), so we own those fixes. Decided
  on 2026-10-16 to stay on it rather than switch to libsrt or the newer pure-Rust
  crates: when the next fault turns up, move the fixes from the driver layer
  into a fork of the crate and offer them upstream.
- **WebRTC/WHIP later, with `str0m`** (sans-IO, fits the core's design).
- **The core muxes and sends.** The app hands encoded frames and audio
  packets with their capture times to the core (`lc_ts_*`); the core builds
  the access units (AUD, parameter sets, timestamp SEI), muxes MPEG-TS and
  queues it on its SRT caller, without crossing back to Swift per packet.
  Done on 2026-10-16, replacing the Swift muxer after a byte-identical A/B.

## Time

- Every video frame carries its capture time in an SEI message, so a server
  can align cameras and keep lip sync. The SEI is in the video bitstream, so it
  survives any transport.
- Capture times are in **TAI**, the same timescale as Strom's PTP paths.
- On phones and browsers the core runs its own **NTP discipline that behaves
  like chronyd with the Ateliere-style broadcast config** Strom uses on its
  hosts (`scripts/setup/ntp/` in the Strom repo): the same pools, `minsources
  3`, `minpoll 6` / `maxpoll 8`, `iburst`, falseticker rejection, slewing
  instead of stepping, and the leap table. The phone's system clock is never
  touched: the core keeps a software clock on top of the host clock that also
  stamps the camera frames.
- The phone does **not** sync against Strom. It syncs against the internet
  (or a venue NTP server handed out at provisioning), as a Strom host does.
- On hosts that already run chrony, the core does not run a competing
  discipline. It uses the system clock and only reports its health.
- Clock health uses the same model and thresholds as Strom's clock panel
  (Healthy, Degraded, Bad).

## Strom, Open Live and the device

```
                control (WebSocket, opened by the device)
 Device ─────────────────────────────────▶ Open Live ─────▶ Strom
    │                                       public front,
    │                                       proxy and auth
    └──────── media (SRT / WHIP) ───────────────────────────▶ Strom (direct)
```

- **The device always connects out.** Phones sit behind 5G carrier NAT, so the
  server never connects to them.
- **Strom implements the device protocol and is fully usable alone.** Camera
  features are Strom components: a registry of connected devices, a control
  API, tally from the vision mixer, a camera block that takes a device's
  stream (and a pool that receives many streams without decoding them all, for
  crowd use), and thin provisioning with a QR code per camera slot.
- **Open Live is the public front when it is used.** It proxies or translates
  the control channel to Strom and adds users, roles, crowd moderation and
  productions spanning several Strom instances. Media goes directly to Strom.
- **Provisioning hands out public URLs** (control, media, NTP), because Strom
  cannot know them behind a proxy.
- **Device ids are global; the binding of a device to a camera block is keyed
  by (flow id, block id)**, since block ids are only unique within a flow.
- **The server side is one library, `tallycam-hub`, run standalone and
  embedded in Strom.** The standalone hub in this repo is the reference and the
  test rig (phones, fake devices and probes work against it without Strom).
  Strom depends on the same crate, so the two cannot drift. The library has no
  web server by default: Strom keeps its own routes (with its OpenAPI contract)
  and maps hub events to its own events. What Strom owns is plugged in through
  traits and calls: storage of known devices, which SRT input a phone sends
  to, and tally from the vision mixer. The Axum routes and the web UI are an
  optional feature.

## Desktop: Strom edge

On a desktop or capture box the device is effectively a Strom: it already has
GStreamer, cameras, capture cards, encoders and SRT out. What it lacks is the
device role. So the direction is a **Strom edge mode**: Strom embeds the core's
device role, registers with a central Strom or Open Live as a device, and
exposes its own controls (camera settings, inputs, flows). No separate desktop
app.

## Open questions

- The product name, and a platform-neutral name for the core. The protocol
  is the Live Contribution Protocol (LCP), named in October 2026:
  "contribution" is broadcast's word for feeds sent into a production, and
  it ties the protocol to no kind of device.
- SEI format: keep our own payload, or adopt an existing standard (MISB ST
  0604 precision time stamp, H.264/HEVC `time_code` SEI) where one fits.
- Software genlock is answered: a phone steers its frame timing onto the TAI
  grid within ±0.25 ms (GENLOCK.md, "Results"). Open is the calibration per
  model: the same capture stamp means a different instant of light on
  different models.
- Whether a camera block in Strom is always placed by the operator (current
  proposal) or created when a device connects.
- Talkback (IFB): an audio channel from the production back to the phone, so
  the camera operator hears directions in an earpiece. Low latency matters more
  than quality here; likely a return audio stream over the same connection
  model (the phone dials out), e.g. WebRTC/WHEP or SRT, mixed per camera or
  per group in Strom.
- A Moblin adapter in Strom, so Moblin users get a free app with a subset of
  the features today.
