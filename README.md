# TallyCam

TallyCam turns iPhones and iPads into cameras for live production. A phone
scans a QR code, streams camera and microphone over SRT, shows tally from
the vision mixer, is remote controlled from the gallery (exposure, white
balance, focus, lens, format, codec, bitrate), and stamps every frame with
its capture time from its own disciplined clock. With software genlock,
phones line up their frames on a shared time grid without cables.

It is built for [Strom](https://github.com/Eyevinn/strom), which embeds the
server side, and also runs on its own with the standalone control hub.

The repository holds:

- **The iOS and iPadOS app** (`ios/`, Swift): capture, encoding, the operator
  screen with tally, and the device side of the protocol.
- **The Android app** (`android/`, Kotlin), early: provisioning, the
  protocol with device keys, preview images, tally, camera names, H.264
  plus Opus over SRT through the same core, and the camera controls with the
  iOS ids (lens, zoom, resolution, frame rate, stabilization, torch, exposure
  with ISO, shutter and bias, white balance, focus, `focus_at`,
  `wb_one_shot`), each offered only where Camera2 reports it. It streams in
  a foreground service (screen off, other app in front), has an in-app QR
  scanner, H.264 or HEVC with Opus or AAC, `srt_latency_ms`, `mains_hz`
  (auto exposure anti-banding), `clock_log`, and soft genlock through the
  core's loop (manual exposure only: Camera2 applies a frame duration only
  with auto exposure off). Tested on the emulator so far; manual exposure,
  genlock, zoom and lens switching wait for a real phone.
- **A shared Rust core** (`core/`): the clock discipline, frame timestamps,
  the MPEG-TS muxer, the SRT caller, genlock and auto ISO, device keys and
  the protocol types. The Android app uses the same core.
- **The hub** (`hub/`): the server side of the Live Contribution Protocol
  (LCP) as a library, which Strom embeds, and **`control-hub/`**, the same
  library as a standalone server with a web page for operators.
- **Tools** (`tools/`) for testing timing and sync without phones.

## Status

A prototype, not released and not stable. Protocol, APIs and storage
formats may change without notice.

What works today, on an iPhone 16 and an iPad 10:

- H.264 or HEVC up to 2160p60 and 1080p240, Opus or AAC audio, as MPEG-TS
  over SRT with AES encryption; about 45 to 50 ms from capture to the network
  plus the SRT latency ([LATENCY](docs/LATENCY.md)).
- Provisioning by QR code, optionally invite-only with a per-device key in
  the Secure Enclave; remote control and tally from the hub or from Strom;
  camera names set by the operator; operator messages on the phone.
- A chrony-like clock in the app (about 0.5 ms estimated error on Wi-Fi), TAI
  capture time in every frame, and software genlock: frames within ±0.25 ms
  of the shared grid, two cameras 0.1 ms apart ([GENLOCK](docs/GENLOCK.md)).
  The same capture stamp still means a slightly different instant on
  different models; calibration per model is the next step.

## Repository layout

| Path | What |
|---|---|
| `ios/` | The iOS/iPadOS app (`project.yml` for XcodeGen, `Config/`, `TallyCam/`, `TallyCamTests/`) |
| `android/` | The Android app (Gradle, Kotlin, Jetpack Compose, Camera2, MediaCodec) |
| `core/` | `tallycam-core` (Rust), `core/srt` (SRT caller), `core/ffi` (Swift bindings: UniFFI and a small C ABI), `core/android` (the same for Android, plus JNI wrappers of the C ABI) |
| `hub/` | `tallycam-hub`: the LCP server side as a library, with the operator web page in `hub/web/` |
| `control-hub/` | The standalone hub binary, and `fake_device`, a simulated phone |
| `tools/` | `fake-camera`, `sync-probe`, `clockd-probe`, `srt-spike`, `torch-pair` |
| `scripts/` | `build-core-apple.sh` and `build-core-android.sh`: build the core and its Swift or Kotlin bindings |
| `docs/` | Design notes and measurements (see below) |
| `design/` | App icon sketches |

## Build

### The app

Requires Xcode, [XcodeGen](https://github.com/yonaskolb/XcodeGen)
(`brew install xcodegen`) and Rust via [rustup](https://rustup.rs) with the
iOS targets:

```sh
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
cd ios
cp Config/Local.template.xcconfig Config/Local.xcconfig   # set DEVELOPMENT_TEAM
xcodegen generate
open TallyCam.xcodeproj
```

The Xcode project is generated from `ios/project.yml` and is not committed. A
pre-build phase runs `scripts/build-core-apple.sh $PLATFORM_NAME`, which builds
the Rust core for the platform being built and generates its Swift bindings
into `build/core/` at the repository root (also not committed).
`scripts/build-core-apple.sh xcframework` builds
`build/core/TallyCamCore.xcframework` for other consumers.

For latency comparisons, the launch argument `-videoLowLatency NO` turns
VideoToolbox's low-latency rate control off.

### The Android app

Requires a JDK 17, the Android SDK with platform 35 and an NDK, Rust with
`aarch64-linux-android`, and `cargo-ndk` (`cargo install cargo-ndk`):

```sh
cd android
echo "sdk.dir=$HOME/Library/Android/sdk" > local.properties
./gradlew assembleDebug testDebugUnitTest      # builds the core first (scripts/build-core-android.sh)
adb install -r app/build/outputs/apk/debug/app-debug.apk
```

`-PskipCore` reuses the core already built. The core is one library,
`libtallycam_android.so`: the UniFFI API of `core/ffi` (Kotlin bindings
generated into `app/build/generated/uniffi`) and JNI wrappers of its
per-frame C ABI, so frames reach the muxer in direct buffers. Android's host
clock is `CLOCK_BOOTTIME`, the clock of Camera2 and AudioRecord timestamps.
On the emulator the hub is `10.0.2.2`; provision with
`adb shell am start -a android.intent.action.VIEW -d 'tallycam://connect?control=ws%3A%2F%2F10.0.2.2%3A8090%2Flcp%2Fv0%2Fdevice&srt=srt%3A%2F%2F10.0.2.2%3A9000%3Fstreamid%3D%7Bdevice%7D'`.
Measure latency with a release build (`assembleRelease`, signed with the
debug key unless `android/keystore.properties` names a release key): a
debuggable build runs the app's code with little JIT and queues about a
second of video. On the emulator a release build sends video about 160 ms
after capture (p50, 720p, software encoder), audio about 150 ms. Without a
hardware H.264 encoder the app streams 720p.

### The core and the hub

```sh
cargo test                                                     # core, FFI, SRT, hub and tools
cargo check -p tallycam-core --target wasm32-unknown-unknown   # the pure part
cargo run --release --bin clockd-probe -- --minutes 10         # the clock discipline on this machine
cd control-hub && cargo test                                   # end to end: hub, fake device, controllers
```

`clockd-probe` runs the discipline against the real NTP pools and prints
`chronyc`-like `sources`, `sourcestats` and `tracking` reports every 30 s.
`tools/srt-spike` tests the Rust SRT sender against libsrt (interop, loss
through a lossy UDP proxy, latency). Shared test vectors for other
implementations are in `core/tests/vectors/`.

## Run

1. Start the control hub on a computer on the same network as the phone:
   `cd control-hub && cargo run --release`.
2. Open http://localhost:8090. With no cameras it shows "Add a camera": a QR
   code with the hub's LAN address. The "Add camera" button shows it again.
3. Scan the code with the iPhone Camera app and open it in TallyCam. The
   camera appears in the hub with preview, tally, status and a control panel
   built from the controls the phone reports.
4. Go Live, from the phone or the hub. By default the phone streams to
   `srt://<hub address>:9000?streamid={device}&latency=100`, an SRT listener
   such as a Strom SRT input; the app replaces `{device}` with its stream id.

```sh
cd control-hub
cargo run --release -- --listen 0.0.0.0:9000                  # or LCP_HUB_LISTEN
cargo run --release -- --srt-url 'srt://192.0.2.20:9000?streamid={device}'
cargo run --release -- --public-url wss://hub.example.com/lcp/v0/device
cargo run --release -- --state-file devices.json              # keep known devices across restarts
cargo run --release -- --preview-fps 5                        # preview images per second (default 2)
cargo run --release -- --invites                              # invite-only; logs the operator link with its token
cargo run --release -- --operator-token SECRET                # the page, invites and forgetting need the token
```

`--help` lists every flag and its environment variable. Append
`#<device id>` to the page URL to open a camera's panel. `/clapper` on the hub
is a full-screen flash and beep on every whole second, for A/V sync tests.

To try the hub without a phone, run a simulated one:

```sh
cargo run --release --bin fake_device -- --key cam2.key --name "Camera 2"
```

`--key` keeps its device key in a file, so it is the same camera on the next
run; `--url` points it at another hub.

## Testing sync without phones

`tools/fake-camera` sends test video and audio as MPEG-TS over SRT, like the
app, with the capture timestamp in every frame; the picture shows the UTC
capture time, a box that lights on each whole second and a ruler of frames,
and the audio beeps every second. `tools/sync-probe` receives one or more
SRT streams and prints, per camera and second, latency from glass to receive,
capture interval and the capture-time offset between cameras. Both need
GStreamer.

```sh
cd tools/sync-probe && cargo run --release -- 9101 9102     # terminal 1

cd tools/fake-camera                                         # terminals 2 and 3
cargo run --release -- "srt://127.0.0.1:9101?streamid=cam-a" --name CAM-A --fps 25 --phase-ms 10
cargo run --release -- "srt://127.0.0.1:9102?streamid=cam-b" --name CAM-B --fps 25 --clock-offset-ms 5
```

sync-probe then reports CAM-B 5 ms before CAM-A (`offset 9102 vs 9101: ms
-5.0`). `--help` on either tool lists the options (frame rate, phase, clock
offset, codecs, A/V measurement with `--av`, summaries with `--duration-s`).
`tools/torch-pair` analyses torch-probe recordings from two cameras
([GENLOCK](docs/GENLOCK.md), "Torch probe").

## Documentation

| Document | About |
|---|---|
| [ARCHITECTURE](docs/ARCHITECTURE.md) | The direction: layers, what is native and what is shared, how TallyCam fits Strom and Open Live |
| [PROTOCOL](docs/PROTOCOL.md) | The Live Contribution Protocol (LCP) v0: provisioning, device keys, messages, controls, timestamps |
| [PROVISIONING](docs/PROVISIONING.md) | A draft of provisioning v1 |
| [LATENCY](docs/LATENCY.md) | Where the time goes, from glass to receiver, with measurements |
| [GENLOCK](docs/GENLOCK.md) | Software genlock: research, implementation notes and results |
| [STROM_INTEGRATION](docs/STROM_INTEGRATION.md) | The design for TallyCam cameras in Strom |
| [DEVICE_CAPABILITIES](docs/DEVICE_CAPABILITIES.md) | Phone capabilities worth using (HDR, gyro, timecode and more) |
| [IDEAS](docs/IDEAS.md) | Direction notes, not decisions |
| [APPS](docs/APPS.md) | One design and one feature set on every platform, and where the apps differ today |

## License

MIT, see [LICENSE](LICENSE). The first MPEG-TS muxer was adapted from
[Moblin](https://github.com/eerimoq/moblin) (MIT), which builds on HaishinKit
(BSD-3-Clause); it has since been replaced by the core's own. Dependencies and
their licenses: [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
