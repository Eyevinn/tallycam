# Build, run and test

How to build the apps, the core and the hub, run the hub, and test timing
without phones. The README has the short version.

## The iOS app

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

`scripts/testflight.sh` archives a Release build and uploads it to App Store
Connect for TestFlight (`--no-upload` exports an `.ipa` instead); its header
lists what it needs. The build number is the number of commits on HEAD.

## The Android app

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

Tested on the emulator so far; manual exposure, genlock, zoom and lens
switching wait for a real phone. Where the app differs from iOS:
[APPS](APPS.md).

## The core and the hub

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

## Running the hub

The README's quick start runs it with its defaults. Options:

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

By default a phone streams to
`srt://<hub address>:9000?streamid={device}&latency=100`, an SRT listener
such as a Strom SRT input; the app replaces `{device}` with its stream id.

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
([GENLOCK](GENLOCK.md), "Torch probe").
