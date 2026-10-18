# Genlock

Research notes and results, October 2026. The software genlock (the
"older phones" path below) is implemented and measured on one iPhone 16: see
"Results". The hardware genlock and `activeLockedVideoFrameDuration` are not
tried yet; those API facts come from Apple's documentation and forums (links
at the end).

## What genlock is

In a broadcast studio a sync generator sends a reference (black burst for SD,
tri-level sync for HD) over BNC to every camera. Each camera locks its frame
clock to it with a PLL:

- **Frequency:** exactly as many frames per second as the reference.
- **Phase:** every frame starts at the same instant as the reference pulse,
  with a phase trim for cable lengths.

In IP studios (ST 2110) the reference is PTP, and **SMPTE ST 2059** defines
where frames sit relative to the PTP epoch: frame *n* starts at
`epoch + n × frame duration`. A software genlock in TallyCam is the same idea
over our disciplined clock (TAI) instead of PTP.

## The problem with phones

Every phone runs its sensor on its own crystal.

```
Grid (TAI):  |-------|-------|-------|-------|     33.3 ms per frame at 30p
Phone A:     |-------|-------|-------|-------|     in phase
Phone B:        |-------|-------|-------|-----    12 ms late (phase)
Phone C:     |--------|--------|--------|---      slightly slow (frequency)
```

1. **Frequency (drift):** a receiver must now and then drop or repeat a frame,
   which shows as a periodic stutter.
2. **Phase (offset):** even at the same rate, picking the nearest frame leaves
   an error of up to half a frame, ±16.7 ms at 30p.

## Without genlock: timestamps

Every frame carries its TAI capture time in the LCP SEI (PROTOCOL.md). Strom
knows how the cameras relate and can keep lip sync and pick the nearest frame,
but the error itself is still there. Phone and receiver clocks agree within a
few ms once settled (LATENCY.md), which bounds how precise any software lock
can be.

With the `genlock` control off, the app pins min and max frame duration to
the same value (`ios/TallyCam/Control/CameraControls.swift`), so the rate is
fixed against the phone's own clock, not against TAI, and the phase is
whatever the sensor starts with. With it on, the software genlock below
steers the frame duration onto the TAI grid.

## What a genlock buys

- Cuts and composites in the vision mixer without a frame synchronizer: lower
  latency, no judder at the cut.
- No periodic dropped or repeated frames from drift.
- Angles of fast action (a ball, a finish line) show the same instant, which
  matters for replays and multiviews.

## Hardware genlock on iPhone (iOS 26)

- **`AVExternalSyncDevice`**: a USB-C accessory that delivers sync pulses.
  According to an Apple DTS engineer, devices conforming to **SMPTE ST 2139**
  are recognised as such. ST 2139 uses a USB-C Alternate Mode to carry timing
  pulses, because ordinary USB data adds too much latency and jitter.
- **`AVCaptureDeviceInput.follow(_:videoFrameDuration:delegate:)`**: the
  sensor follows the accessory's pulses. The frame duration must match the
  pulse duration, or the request times out and the session stops.
- **`AVCaptureDeviceInput.activeLockedVideoFrameDuration`**: runs the sensor
  at *precisely* the requested frame duration, no accessory needed. Check
  `isLockedVideoFrameDurationSupported` and
  `minSupportedLockedVideoFrameDuration` first (setting it otherwise throws).
  It resets when `activeFormat` changes, is unsupported with auto frame rate
  or spatial video, and "may cause a lengthy reconfiguration", so it cannot be
  nudged frame by frame. Apple recommends a manual exposure of at most half a
  frame with it.

**Blackmagic Camera ProDock** (about USD 295, iPhone 17 Pro and Pro Max) is
such an accessory. It takes genlock and timecode over BNC and passes the sync
to the phone over USB-C, plus audio inputs and SSD recording. That makes the
phone a studio camera on the house reference, cabled.

Field observations from one developer (forum, unverified):

- Without the lock, with fully manual exposure, capture ran at about
  29.964 fps instead of 29.97, which is one dropped frame every three minutes
  or so. With the lock it was exactly 30000/1001.
- With the lock and fully manual exposure, a 20 ms shutter was clamped to
  16.7 ms (half a frame). At 30p that is the usual 180° shutter, so likely
  not a problem in practice.

## Three levels

| Level | How | Where |
|---|---|---|
| Hardware genlock | `follow()` with an `AVExternalSyncDevice` (ProDock) | Studio, newest Pro phones, cabled to a sync generator |
| Software genlock | Frequency lock plus our own phase steering onto the TAI grid (implemented, see "Results") | Every other phone, including retired ones |
| Timestamps only | The SEI capture time, genlock off | Fallback |

The phone reports its level to Strom, so Strom knows which cameras it can cut
between without a frame synchronizer.

Hardware genlock is cheap to support: if an `AVExternalSyncDevice` is present,
call `follow()` and report "genlocked, hardware", behind
`#available(iOS 26, *)`.

## Software genlock: open questions

- **Frequency:** `activeLockedVideoFrameDuration` gives an exact rate against
  the phone's crystal. What remains is the crystal against TAI, a few ppm,
  about one frame an hour. Measure it with our clock; Strom can absorb it, or
  we re-lock now and then.
- **Phase:** there is no API to set it, and changing the locked duration is a
  lengthy reconfiguration. Can we choose the phase by timing when the lock is
  switched on?
- **Older phones** (no iOS 26, or no locked duration): does nudging the old
  `activeVideoMinFrameDuration` / `activeVideoMaxFrameDuration` for a few
  frames slide the phase without dropped frames, flicker or exposure jumps?
  This is the path for retired phones.
- **Which models** report `isLockedVideoFrameDurationSupported`?
- **What the timestamp means:** how close is the sample buffer's PTS to the
  real exposure? With a rolling shutter, top and bottom rows differ by
  perhaps 10 to 30 ms, which bounds how much precision is meaningful.
- **Clock quality:** a few ms on Wi-Fi (measured); 5G is unknown. A few ms is
  still far better than ±16.7 ms.

## Results, 2026-10-16 (iPhone 16, iOS 26.6, 1080p30, Wi-Fi)

Code: the loop is `core/src/genlock.rs` (C API `lc_genlock_*`), the app side
`ios/TallyCam/Control/GenlockController.swift`, switched by the `genlock` control.
Phases are measured from the SEI capture stamps (`sync-probe --frames-csv`,
then `--analyze`), so they are against the phone's own TAI estimate.

- **Free running:** the sensor ran 75 ppm fast against TAI at a requested
  1/30 s (33.3308 ms), steady over 18 minutes: the phase moved -4.5 ms a
  minute, a full frame every 7.4 minutes. Jitter around the line: p50 32 us,
  max 118 us.
- **The sensor follows `activeVideoMin/MaxFrameDuration` only with custom
  (manual) exposure.** With continuous auto exposure, requests from 33333 to
  33500 us all gave 33339 us, and 34000 gave 34481 us: iOS auto exposure owns
  the frame duration. With manual exposure the delivered duration tracks the
  request in steps of about 9.6 us (a sensor line) and takes effect within
  0.11 to 0.14 s (3 to 4 frames), with no dropped or odd frames.
- **The format needs headroom:** a format whose maximum is 30 fps refuses any
  duration shorter than 1/30 s, so genlock picks a format that also runs
  faster (here the 1-60 fps one). 1/30 s in whole ns is just below such a
  format's minimum; clamp as CMTime.
- **Locked:** with manual exposure the loop locked in 8 s. Over 5075 frames
  after the lock the phase error was p50 0.09, p95 0.21, p99 0.25, max 0.35 ms,
  mean +0.004 ms, with no interval off by more than a quarter frame. The loop
  settled at +55 to +170 ppm and follows the sensor's slow drift.
- **Auto ISO:** turning genlock on with auto exposure moves the camera to
  auto ISO (custom exposure, the shutter auto exposure had chosen, ISO from
  the metered offset four times a second). It locked in 13 s and stayed
  locked, phase under 0.17 ms, while the ISO followed exposure bias steps of
  +1 and -1 EV.

### Two cameras, 2026-10-16 (iPhone 16 and iPad 10th generation)

Both genlocked at 1080p30 H.264, manual exposure (ISO 400, 1/250), filming
the hub's `/clapper?silent&sweep` on a MacBook Pro display (120 Hz), received
by `sync-probe --av` on two ports. The sweep moves each second's flash 1 ms
later (`second mod 34`), so the flash edge crosses a whole frame every 34 s.

- **Frame grids:** the two cameras' capture stamps were 0.1 ms apart, every
  second of the run. Both stayed locked within about ±0.25 ms of the grid.
  A phone call took the iPhone off for 16 s; it came back live by itself and
  relocked 15 s later.
- **Without the sweep** (flash at a fixed phase), both cameras saw 77 of 81
  flashes in the same frame, with stamps ±0.4 ms apart. In a run where the
  flash edge sat near a frame boundary, 31% of flashes landed one frame later
  on the iPad.
- **With the sweep**, the iPad moved to the later frame at a flash offset of
  about 10 to 17 ms and the iPhone at 20 to 26 ms, over three cycles: 8, 16
  and 9 ms apart. These differences sit at the display's 8.3 ms refresh
  step, so they **do not establish an offset between the models**: an iPad
  exposing earlier for the same capture stamp is not proven. Display
  scan-out plus rolling-shutter geometry (where the display sits in each
  picture) explain them as well.

**Calibration attempt:** with the genlock target held still and the flash
fixed, sweeping both cameras' target offset 0 to 34 ms in 1 ms steps showed
them disagreeing over a band about 7 ms wide. A 7 ms correction for the iPad
(`ios/TallyCam/CaptureCalibration.swift`) did not narrow the band (8.6 ms
afterwards, at another offset), so the value is not used. The display is not
a stable enough light: its flash sometimes moved by one refresh (8.3 ms) and
the Mac's own clock drifts against TAI during a run. Rolling shutter is
probably the largest error: if the display sits at different heights in the
two pictures, identical cameras see it at different times, by up to the
readout time (10 to 30 ms).

The fixed-flash runs also disagreed with each other, and the 7 ms correction
did not move the disagreement band (8.6 ms, about one refresh), which points
at the display rather than the cameras.

So genlock aligns the stamps to a fraction of a millisecond, but whether a
capture stamp means the same instant on every model is still open. If it
does not, one fixed delay per model will not model it: the right model is a
first-row offset per model and format plus the readout time, which makes the
offset depend on the row (where the subject is in the frame). It is a
per-model property, not a genlock error.

Next measurements, in order: swap the two cameras' positions in front of the
display clapper (a geometry effect follows the position, a model offset
follows the camera), then mains-flicker bands on a lit sheet, then a GPS-PPS
driven LED.

Open:

- **Calibration per model and format:** measure each model's offset between
  its capture stamp and the light it records, store it per model, format and
  frame rate, and apply it both to the SEI stamp and to the genlock target
  (`GenlockConfig::target_offset_ns`). Measuring to under 1 ms needs a light
  with a sharp edge instead of a display: an LED switched on the TAI second
  by a microcontroller with GPS or NTP, or the torch of a genlocked phone.
  Put the light at the same height in both pictures, or measure the row
  where the edge appears, which also gives each model's readout time. Use two
  units of the same model to see the spread between units; expect the offset
  to depend on format, frame rate, lens and iOS version.
- The phone's clock bounds the real error: about 0.4 ms estimated, 2.8 ms
  maximum on Wi-Fi today.
- Auto ISO overshot on a +1 EV step (ISO 448, 632, 545, 566); see "Auto ISO"
  below for the loop that replaced it.
- 25, 50 and 60 fps, 4K and HEVC, and other phone models are not tried.
- `activeLockedVideoFrameDuration` and hardware genlock are not tried.

## Formats with no room to shorten

Genlock trims the frame duration around the nominal period, but a format
whose shortest frame is the nominal one (min frame duration = 1/fps) can only
lengthen frames. A sensor that runs fast is held by lengthening. One that runs
slow can not be: it falls behind the grid by its rate error (75 ppm is 2.5 µs
a frame at 30 fps), and nothing short of a slip brings it back.

The loop treats a duration held at the format's minimum as saturated, so
its integral does not unlearn the trim (a small late step is trimmed back in
instead of slipping). When three windows in a row at the minimum drift later,
it judges the format too slow for the sensor (`format_too_slow` in the
status) and stops slipping: the phase drifts late, wraps at half a period
and is pulled back in from the early side. At +75 ppm and 30 fps that is
one whole-frame wrap about every 227 s instead of a slip every 14 s, and the
state is Locked only while the error is within the lock threshold. Three
windows catching up clear the verdict. The app should prefer a format with
shorter frames (most phones have one at the same size) when it sees it;
`format_too_slow` is not yet in the FFI status or the apps.

## Auto ISO

Genlock needs the frame duration, which iOS auto exposure moves, so exposure
`auto` is the core's auto ISO loop at a fixed shutter (SHADING.md,
"Shutter"), with or without genlock. The loop is in the core
(`exposure` module, `lc_auto_iso_*`); the app samples `exposureTargetOffset`,
the ISO and the shutter at 8 Hz and applies what the loop returns.

- **The metering's gain is not 1.** The loop that ran in the app until
  2026-10-16 (4 Hz, half the error per step) went ISO 448, 632, 545, 566 on a
  +1 EV bias step: the offset moved about 3 EV per EV of ISO. The loop now
  learns that gain from its own steps, starts high (3), so a first step falls
  short instead of overshooting, and waits after each step until the metering
  has followed and settled before it looks again.
- **Flashes are ignored.** Scene changes count once they fill half of a
  1.2 s median window, so spikes up to about half a second (a flashing
  screen in shot) leave the ISO alone. Bias changes act at once.
- **Mains flicker.** The mains frequency (50, 60, or 0 for off; the hub's
  production, else the phone's own setting) rounds the shutter to whole
  light pulses: 1/100, 1/50, 30 ms at 50 Hz; 1/120, 1/60, 1/40, 1/30 at
  60 Hz, within the frame (with 2% room while genlock may shorten frames).
  The ISO compensates the rounding; at the lowest ISO the loop shortens the
  shutter to fewer pulses (1/100 from 1/50) before it reports too bright. At
  frame rates above twice the mains frequency no flicker-free shutter fits
  and the shutter is left alone. Off (0) leaves any shutter, for scenes
  without mains-lit light, such as outdoors.
- Simulated (camera model with the measured gain 3): a +1 EV bias step
  settles in about 0.25 s with no overshoot; across gains 0.7 to 4 and
  metering lags up to 0.5 s, within 2.2 s without overshoot (two steps while
  the gain is still unknown).
- Measured 2026-10-16 (iPhone 16, 1080p30, indoor scene; ISO from the
  `state` events at 4 Hz): bias 0 to +1 EV went ISO 381, 698, 758; back to 0
  went 758, 416, 391; to -1 went 391, 236, 202, 196. No overshoot, about 90%
  of the step within 0.5 s of the `set`, settled within 1.3 s. That scene's
  gain was about 1, not 3: the gain depends on the scene. With genlock on and
  locked (phase under 0.25 ms), +1 EV went 377, 636, 740, 764. The shutter
  went from 1/250 to 1/100 at 50 Hz and to 1/120 at 60 Hz. Flashes were only
  simulated.

## The spike (done)

The plan was: film `hub/web/clapper.html` with two phones, then add the
frequency lock and the phase steering, and see whether ±16.7 ms comes down to
a few ms without visible artefacts. It did, through `activeVideoMin/
MaxFrameDuration` with manual exposure (see "Results"); the frequency lock
through `activeLockedVideoFrameDuration` is still untried.

## Idea: community calibration (2026-10-16, not built)

Goal: per-model offsets (capture stamp against the light the sensor records)
measured by users, without special hardware. What is measured per model,
format and frame rate: the offset of the middle row's exposure from the
capture stamp, and the readout time (so any row's offset follows).

**The rig everyone has: a lamp that flickers on the mains.** Many LED and
fluorescent lamps flicker at twice the mains frequency (100 Hz at 50 Hz,
120 Hz at 60 Hz). With a fast shutter (1/1000 or shorter) the rolling shutter
turns the flicker into horizontal bands. Their position and spacing give, per
frame and to a fraction of a millisecond:

- the readout time (band spacing in rows against the 10 ms period), from
  one phone alone;
- the flicker phase at the capture stamp. Two genlocked phones filming the
  same evenly lit white sheet see the same lamp, so the difference of their
  phases is their relative offset. Mains frequency drift cancels, since both
  measure at once on the same TAI grid.

Catches:

- **Ambiguity modulo the flicker period** (10 or 8.3 ms): an 11 ms offset
  reads as 1 ms. Resolve it with a coarse measurement first (the display
  clapper is good to about ±4 ms, enough to pick the right period).
- **One lamp, not one room:** lamps on different mains phases are 120° apart
  (3.3 ms at 50 Hz). Light the sheet with a single lamp.
- **Not every lamp flickers** (many are flicker-free; incandescent barely
  does). The app can check for bands at a fast shutter and say whether the
  lamp works.

**A torch flash works too, and is simpler.** One shared light event seen by
both phones gives their relative offset, and the event's own latency cancels.
Phone A's torch lights a white sheet (better than a mirror: the whole frame
brightens, so the rolling shutter shows the edge at a row). Both phones film
the sheet with manual exposure and a short shutter. No period ambiguity.
Its own torch alone cannot calibrate a phone: the camera sees "camera offset
+ torch latency", two unknowns, and the torch command is asynchronous with
ms-level jitter.

Limits of the torch method:

- **Readout time is not measured by it.** Per flash, edge time = stamp +
  offset + edge row × row time, for each phone. Over many flashes that
  leaves the offset difference and both row times entangled: one row time
  must come from elsewhere (flicker bands, above).
- **The edge must fall inside both readouts**, not in the vertical blanking
  between frames, or it only resolves to a frame. Repeat flashes and keep the
  ones that land in readout.
- **Precision depends on the shutter:** a row integrating over the edge is
  partly lit, so the edge smears over shutter ÷ row time rows; fit the ramp's
  midpoint.
- **Unverified on device:** whether the torch ramps up softly, and whether it
  is PWM-dimmed below full level (which would add bands of its own). Use full
  level.

**Torch probe (built and run, see the runs below).** The `torch_probe` command
measures what one phone can measure about its own torch:
`ios/TallyCam/Control/TorchProbe.swift` (switching, per-frame luma profile along
the readout) and `TorchProbeAnalysis.swift` (fit, unit tested on a simulated
rolling shutter).

- Setup: the phone lies screen up on two supports a few cm above a white
  sheet, back camera and torch facing it, in a dim room. Manual exposure with
  a short shutter (1/2000 or shorter) and a fixed ISO that keeps the lit sheet
  below clipping. Stabilization off.
- Run: `torch_probe` from the hub (defaults: 40 cycles, 300 ms on, 300 ms off,
  about 26 s). Each switch-on moves by a golden-ratio fraction of a frame, so
  the switches cover the frame phase evenly. Auto ISO is paused during the run.
- Per switch it finds the frame whose readout holds the edge and the line
  where the level crosses 50%. Fitting line against (command - stamp) gives:
  - **line time and readout time**, with a standard error;
  - **torch latency minus the stamp's offset to the first line**, separately
    for on and off (one number each; the two parts need a second camera or
    the GPS LED);
  - **jitter** of the torch command (residuals around the line);
  - **edge width** 10 to 90% in lines and ms (the torch's rise, plus the
    shutter's own smear);
  - **ripple** along fully lit frames against dark frames (PWM or flicker).
- Output: a `log` message, and `Documents/torch-probe/<time>-<model>/` with
  `summary.txt`, `events.csv`, `commands.csv` and `frames.csv` (every 8th
  line of each frame's profile), in the Files app or Finder.
- Expect edges in the vertical blanking between readouts to be missed (the
  summary counts them).
- Pairs: `mode` `emit` on the phone with the torch (genlocked), `watch` on the
  other camera (genlock off, `frame_duration` 33400 us, a Debug-build command, so its phase slides
  through a frame in about 17 s), started together; then
  `python3 tools/torch-pair/analyze.py EMITTER_DIR WATCHER_DIR`. Frames and
  commands carry TAI stamps for this. The script fits the watcher's edge line
  against both the emitter's switched frame and the command, so the smaller
  scatter shows which one sets the light (`--selftest` checks it).

First run, 2026-10-16, iPhone 16, iOS 26.6.2, 1080p30, genlocked, 1/2000,
ISO 200, 40 cycles (`Documents/torch-probe/20261016-162733-iPhone17,3`):

- **The torch switches on frame boundaries, not when it is told.** In 784
  frames not one held both dark and lit lines: 0 of 80 edges inside a frame.
  Contrast was fine (luma 35 off, 190 on).
- Every switch, on and off, showed in the **first frame stamped after the
  command**, 2.2 to 24.9 ms after it, never a later one; the frame stamped
  just before the command stayed wholly in the old state even when the
  command came 8 ms into it, while its lines were still exposing. So the
  camera pipeline holds the change for the next frame (as for flash
  photography), and the latency within that is under about 2 ms.
- The commands did not spread over the frame phase as scheduled: they
  clustered 8 to 14 ms and 24 to 31 ms after a frame stamp. The `before`
  stamp is taken after `lockForConfiguration()`, which seems to wait on the
  capture pipeline (unverified).
- Levels drift after each switch (dark frames 22 to 43, lit 177 to 160 over
  four frames) with exposure fixed: the ISP's tone mapping adapts.

Second run, same day: filming with the ultra wide lens while the wide
camera's torch switched (`Documents/torch-probe/20261016-164205-iPhone17,3`).
The same: 1 frame of 781 held both states, every switch showed in the next
frame of the ultra wide camera. The torch is held for the frames of
whichever back camera runs, not of its own sensor.

Online (searched 2026-10-16), nothing documents this, but it fits:

- A strobe app developer found that a running capture session (device input
  plus video data output) cured an iOS 18 torch strobing bug, guessing that
  the session makes the torch behave as a camera flash. Without a session,
  `torchMode` changes went straight to the LED (10 Hz strobes worked; on
  iPhone 15 Pro some took up to 500 ms under the iOS 18.1 bug, fixed in 18.4).
- Since iPhone 11 the torch lingers a few ms after `.off` and frames after it
  come out dim (unanswered forum thread), which fits a light handled by the
  camera pipeline.
- No API turns the synchronisation off. The flash in `AVCapturePhotoSettings`
  is for stills.

Ways round it, untried: the front camera running while the back torch
switches (is the torch held for the front sensor's frames too?); no session
at all on the emitting phone (the light then follows the command, which
only a second camera can see); or the screen as the light, filmed through a
mirror, whose refreshes `CADisplayLink` stamps and which runs on the
display's clock, not the camera's (but it scans row by row and adds its own
latency).

So a phone cannot time its own rolling shutter with its own torch. The
finding may help the pair method instead: a genlocked phone's torch switches
at a fixed point of its own frame, on the TAI grid, which makes it a light
on the grid for other cameras, if that point is stable (to measure with a
second camera).

**Pairs build a network.** Each session measures model A against model B.
Combined over many users (least squares over all pairs), every model gets an
offset relative to the others, with the spread between units. A few anchor
models measured absolutely fix the whole network to true time.

**The anchor: a GPS-PPS LED.** An ESP32 with a GPS module drives an LED on the
PPS edge (microseconds from UTC; TAI is a fixed offset). It costs a few tens
of euros and can be open hardware. We, and keen users, measure anchor models
with it; everyone else calibrates against the network with a lamp and a
second phone.

**Flow in the app:** a calibration mode (two phones, genlocked, filming one
white sheet), coarse step with the clapper, fine step with the lamp, results
(model, format, frame rate, lens, iOS version, offset, readout time) uploaded
to a shared database. Phones and Strom fetch the offset for their model.

## Prior art (searched 2026-10-16)

No public software-only phase lock on iOS turned up. What exists:

- **Hardware genlock on iPhone 17 Pro:** Final Cut Camera 2.0 and Blackmagic
  Camera, with a sync generator and a dock (ProDock). Final Cut Camera adds a
  frame delay to trim the shutter against the reference.
- **iOS multi-phone apps** such as MultiCameraSync start and stop recording
  together, with no stated phase lock or accuracy.
- **Google Research, "Wireless Software Synchronization of Multiple
  Distributed Cameras"** (Ansari, Wadhwa, Garg, Chen, ICCP 2019), Android,
  open source as `libsoftwaresync`, tested on Pixel 1 to 4:
  - A leader phone on the same Wi-Fi; clients sync their clocks to it (NTP
    style, a minimum filter over 300 samples): mean error 32 us, max 121 us.
    The clock is the dominant error.
  - Phase by **frame injection**: Camera2 lets one high-priority request with
    a longer exposure into the stream, which shifts the phase in one step.
    The mapping from request to shift is modelled per phone model. 4 to 7
    injections, under 3 s, to 20 us.
  - Fallback, **reset sampling**: restart the camera after a random sleep
    until the phase happens to fall within tolerance. Portable but slow
    (about 120 s for 1 ms).
  - Total under 250 us between any two phones. Assumes symmetric network
    latency, up to 11 clients, no drift handling.
- **Akhmetyanov et al., "Sub-millisecond Video Synchronization of Multiple
  Android Smartphones"** (IEEE Sensors 2021) builds on it, with a
  timestamping model checked on 47 Android devices.

How ours differs: we lock to an **absolute TAI grid** rather than to a leader
phone, so any number of cameras on any network line up with each other and
with Strom, and the loop steers continuously through frame durations rather
than in one injected step (iOS has no per-request injection). Google's work
suggests two things for us: the clock is the error to attack (a local time
source on the LAN, such as the hub or Strom, should beat public pools by an
order of magnitude), and per-model calibration is expected, as found above.

## Sources

- [Apple Developer Forums: iPhone 17 Pro's Genlock feature](https://developer.apple.com/forums/thread/799739)
- [Apple: activeLockedVideoFrameDuration](https://developer.apple.com/documentation/avfoundation/avcapturedeviceinput/activelockedvideoframeduration)
- [Apple: follow(_:videoFrameDuration:delegate:)](https://developer.apple.com/documentation/avfoundation/avcapturedeviceinput/follow(_:videoframeduration:delegate:))
- [Apple: AVExternalSyncDevice](https://developer.apple.com/documentation/avfoundation/avexternalsyncdevice)
- [Apple Developer Forums: locked frame duration and manual exposure](https://developer.apple.com/forums/thread/848520)
- [SMPTE ST 2139 (GitHub)](https://github.com/SMPTE/st2139)
- [Photography News: ProDock brings genlock and timecode](https://www.photographynews.co.uk/news/prodock-brings-genlock-and-timecode-to-new-iphone/)
- [RedShark News: Blackmagic ProDock](https://www.redsharknews.com/blackmagic-prodock-iphone-17-pro-camera-accessory)
- [Apple: Set timecode and genlock in Final Cut Camera](https://support.apple.com/guide/final-cut-camera/devdbb68a124/ios)
- [Apple: Virtual production with Apple devices and genlock](https://support.apple.com/148292)
- [MultiCameraSync (App Store)](https://apps.apple.com/qa/app/multicamerasync/id6447541517)
- [Ansari et al., Wireless Software Synchronization of Multiple Distributed Cameras (arXiv)](https://arxiv.org/abs/1812.09366)
- [libsoftwaresync (GitHub)](https://github.com/FilippoAleotti/libsoftwaresync)
- [Akhmetyanov et al., Sub-millisecond Video Synchronization of Multiple Android Smartphones (arXiv)](https://arxiv.org/abs/2107.00987)
