# TallyCam

Prototype iOS app that turns an iPhone into a contribution camera for
[Strom](https://github.com/Eyevinn/strom): camera and microphone over SRT to a
Strom server, provisioned by QR code and remote controlled from Strom.

Status: local prototype. Not released, not stable. Phase 1 implemented: the
app sends camera (H.264 High or HEVC Main with the `video_codec` control;
1080p30 at 6 Mbit/s by default, 720p to 2160p and up to 240 fps where the lens
has them, with `resolution` and `frame_rate`) and microphone (Opus,
10 ms frames, 48 kHz mono; AAC-LC with the `audio_codec` control) as
MPEG-TS over SRT (caller) to a Strom SRT input. Verified locally
against a GStreamer `srtsrc` listener; on-device picture check in Strom
pending. SRT encryption (AES-128/192/256) with `passphrase` and `pbkeylen` in
the stream URL (docs/PROTOCOL.md, "Stream URL").

## Plan

0. App runs on a device and shows the camera.
1. Encode H.264/HEVC and send MPEG-TS over SRT to a Strom SRT input.
2. Provisioning: scan a QR code from Strom (server URL + one-time token), then
   fetch the camera configuration from Strom.
3. Remote control: the phone opens a WebSocket to Strom (outbound, so it works
   behind 5G carrier NAT). Status up (battery, thermal, bitrate, link), commands
   down (live, exposure, ISO, shutter, white balance, focus, zoom, lens,
   bitrate), tally and operator messages.
4. Time sync: the phone runs its own chrony-like NTP discipline (Strom's chrony
   configuration, four public pools, `minsources 3`) and stamps every frame
   with its TAI capture time.

Later: Android (same control protocol), SRTLA bonding, talkback, return video.

## Build

Requires Xcode, [XcodeGen](https://github.com/yonaskolb/XcodeGen)
(`brew install xcodegen`) and Rust via [rustup](https://rustup.rs) with the iOS
targets:

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
cd ios
cp Config/Local.template.xcconfig Config/Local.xcconfig   # set DEVELOPMENT_TEAM
xcodegen generate
open TallyCam.xcodeproj
```

The repository holds one app per platform next to the shared core: `ios/`
(the iPhone and iPad app), `android/` later, `core/` (Rust, used by both and
by the server side), `hub/` and `control-hub/` (server), `tools/`.

The Xcode project is generated from `ios/project.yml` and is not committed. A
pre-build phase runs `scripts/build-core-apple.sh $PLATFORM_NAME`, which builds
the Rust core for the platform being built and generates its Swift bindings
into `build/core/` at the repository root (also not committed).
`scripts/build-core-apple.sh xcframework` builds
`build/core/TallyCamCore.xcframework` for other consumers.

## Shared core (Rust)

`core/` (`tallycam-core`) holds what other apps and servers share: the clock
discipline (a chrony-like NTP client as a pure state machine, plus a std
driver behind the `driver` feature), the LCP timestamp SEI and the LCP device
protocol types. `core/ffi` exposes it to Swift (UniFFI for the clock service,
a small C ABI for per-frame calls). Shared test vectors for other
implementations are in `core/tests/vectors/`.

```sh
cargo test                                           # core, FFI and probe
cargo check -p tallycam-core --target wasm32-unknown-unknown   # pure part
cargo run --release --bin clockd-probe -- --minutes 10          # discipline on this machine
```

`clockd-probe` runs the discipline against the real pools and prints
`chronyc`-like `sources`, `sourcestats` and `tracking` reports every 30 s,
with the disciplined time against the machine's own clock.

## Use

1. Start the control hub on a computer on the same Wi-Fi as the phone (see
   below): `cd control-hub && cargo run --release`.
2. Open http://localhost:8090. With no cameras it shows "Add a camera": a QR
   code, the control URL and the SRT URL, using the computer's LAN address
   (never `localhost`). The "Add camera" button in the header shows it again.
3. Scan the code with the iPhone Camera app and open it in TallyCam. The
   app saves the control and SRT URLs, connects to the hub ("Connected to
   <host>") and the camera appears in the hub.
4. Tap Go Live (or use Go Live in the hub). The SRT URL points at a Strom SRT
   input in listener mode, by default `srt://<hub address>:9000` with
   `streamid=cam-<device>`; start the hub with `--srt-url` to hand out another.

The URLs can also be typed in the app, for example
`srt://192.0.2.10:9000?streamid=cam1&latency=100`. They are remembered. The
link format is in [docs/PROTOCOL.md](docs/PROTOCOL.md), "Provisioning v0".

## Control hub (prototype)

The server side of the Live Contribution Protocol (LCP)
([docs/PROTOCOL.md](docs/PROTOCOL.md)) is a library, `hub/` (`tallycam-hub`),
so it runs standalone from this repo and can be embedded in Strom:

- Without features it has no web server dependency: the device registry,
  tally, preview gating, provisioning (control and SRT URLs, deep link) and a
  `DeviceStore` trait for keeping known devices and their keys across
  restarts (`remove` has a default no-op; a store that persists should
  implement it, or a forgotten device comes back after a restart).
- `SrtTargets` decides which SRT URL a phone gets; `Hub::set_tally` takes tally
  from an operator or from production (a vision mixer); `Hub::subscribe` gives
  typed events for an embedding server to map to its own.
- Feature `server` adds the Axum routes, the WebSocket sessions
  (`device_session`, `controller_session`, usable behind the embedder's own
  routes) and the operator web UI. Features `lan` and `qr` add LAN address
  discovery and the QR code.

`control-hub/` is the standalone binary: phones connect to
`ws://<host>:8090/lcp/v0/device`, and the operator UI at `http://<host>:8090/`
shows every camera with preview, tally, status and a control panel generated
from the controls the phone reports.

```sh
cd control-hub
cargo run --release                     # listens on 0.0.0.0:8090
cargo run --release -- --listen 0.0.0.0:9000   # or LCP_HUB_LISTEN=...
cargo run --release -- --srt-url 'srt://192.0.2.20:9000?streamid={device}'
cargo run --release -- --public-url wss://hub.example.com/lcp/v0/device
cargo run --release -- --state-file devices.json   # keep known devices
cargo run --release -- --preview-fps 5             # previews per second (default 2)
cargo run --release -- --invites                    # invite-only; logs the operator page link with its token
```

Open http://localhost:8090 (append `#<device id>` to open a camera's panel) and
scan the "Add a camera" QR code with the phone. `--srt-url` (`LCP_HUB_SRT_URL`)
is the SRT URL handed to phones; `--public-url` (`LCP_HUB_PUBLIC_URL`) replaces
the advertised control URL when the hub sits behind a proxy. `--state-file`
(`LCP_HUB_STATE_FILE`) keeps the known devices, so they are listed offline
after a restart; tally is not kept.

To test without a phone, run a simulated iPhone in a second terminal. It sends
the iOS prototype controls, applies `set` with clamping and `enabled_when`,
reports status every second and sends generated JPEG previews:

```sh
cargo run --release --bin fake_device -- --id cam2 --name "Camera 2"
```

`--url` points it at another hub (default `ws://127.0.0.1:8090/lcp/v0/device`).

`http://<host>:8090/clapper` is a digital clapper: a full-screen flash and a
beep on every whole second, to film with a camera for A/V sync
([docs/LATENCY.md](docs/LATENCY.md)).
`cargo test` in `control-hub/` runs an end-to-end test of hub, fake device and
controllers; `cargo test` in `hub/` (add `--features server`) the library
tests.

## Testing sync without phones

`tools/fake-camera` is a simulated camera for multi-camera sync tests. It sends
test video and audio as MPEG-TS over SRT (caller), like the app, with the LCP
capture timestamp SEI ([docs/PROTOCOL.md](docs/PROTOCOL.md), "Time sync and
frame timestamps") in every H.264 access unit. The picture shows the camera
name and the UTC capture time with milliseconds, a box that lights on the
frame that covers each whole second, and a ruler with one cell per frame of the
second. The audio beeps for 50 ms at the start of every second. All cameras on
one machine share its clock, so mixed side by side the clocks, boxes and beeps
show the frame and A/V alignment directly.
With `--core-ts` it muxes with the core's MPEG-TS muxer and SRT caller, the
same code path as the app (default: GStreamer's `mpegtsmux`).

`tools/sync-probe` is the receiver: it listens on one or more SRT ports and
prints, per camera and second, stamped frames, latency, capture interval and
the capture-time offset between cameras. Its receive times come from the
shared core's clock discipline (the same pools as the phones), so latency is
glass to receive on one TAI timescale; the receiver's own clock health is
printed alongside. Latency is shown at three points, p50/p95/max per second:
`glass->srt` (the SRT payload handed to the application, parsed by the tool
itself, video and audio), `demux` (after `tsdemux` and `h264parse`) and
`playout` (when a syncing sink would render it), plus the PTS lead over the
PCR. See [docs/LATENCY.md](docs/LATENCY.md).

```sh
cd tools/sync-probe && cargo run --release -- 9101 9102     # terminal 1

cd tools/fake-camera                                         # terminals 2 and 3
cargo run --release -- "srt://127.0.0.1:9101?streamid=cam-a" --name CAM-A --fps 25 --phase-ms 10
cargo run --release -- "srt://127.0.0.1:9102?streamid=cam-b" --name CAM-B --fps 25 --clock-offset-ms 5
```

sync-probe then reports CAM-B 5 ms before CAM-A (`offset 9102 vs 9101: ms
-5.0`): CAM-A captures 10 ms after each frame grid point, and CAM-B stamps its
frames 5 ms late.

Options (`--help` lists them all):

- `--phase-ms MS`: capture the frames MS after the whole second (cameras whose
  frame grids are not aligned).
- `--clock-offset-ms MS`: add MS to the stamped capture time, may be negative
  (a phone whose clock is wrong). Picture and beep stay on true time.
- `--unsynced`: clear the synced flag and report unknown uncertainty.
- `--fps 25|30|50|60`, `--width`, `--height` (default 1280x720), `--latency-ms`
  (SRT, default 200), `--bitrate-kbps`, `--encoder auto|x264|vt`,
  `--duration-s` (stop after a while), `--streamid`.
- `--local` renders the picture (full-frame flash) and the beep on this
  machine's screen and speakers instead of sending them, `--audio-only` only
  the beep: a reference for a phone to film or hear (docs/LATENCY.md).

Capture times are stamped in TAI from the core's clock discipline (it waits
until that is healthy); `--system-clock` stamps the OS clock in UTC instead.
It uses `x264enc` (`vtenc_h264` if missing) and the first AAC encoder found
(`avenc_aac`, `fdkaacenc`, `voaacenc`), or with `--audio-codec opus`
`opusenc` (10 ms frames, restricted low delay). The audio PTS is moved
back by the encoder's priming (1024 samples for `avenc_aac`, 120 for
`opusenc`), since decoders do not trim it on MPEG-TS: `sync-probe --av` then
reports flash and beep at 0.0 ms.

sync-probe options: `--latency-ms` (SRT, default 120), `--raw` (only the SRT
view), `--no-playout`, `--tsdemux-latency MS`, `--av` (decode flash and beep
from the stream's own PTS, AAC or Opus, and print the audio - video offset),
`--system-clock`, `--wait-sync` (wait for a healthy clock), `--duration-s`
(then a summary with p50/p95/p99), `--quiet`.

## Credits

The media path is informed by [Moblin](https://github.com/eerimoq/moblin)
(MIT), which in turn builds on HaishinKit (BSD-3-Clause). The first MPEG-TS
muxer was adapted from them; it has since been replaced by the core's own
(`core/src/ts`). Dependencies and their licenses:
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
