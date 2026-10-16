# Latency and sync

Code is the source of truth — this may have drifted; read the code for the
current implementation. The numbers below were measured once, on one setup
(an iPhone 16 on home Wi-Fi, a MacBook receiver on the same network, 1080p30
H.264 at 6 Mbit/s plus AAC; Opus is the default audio since, see "Audio
budget"), in October 2026. Treat them as orders of
magnitude, and measure again with the tools when something changes.

## How it is measured

- **Both ends on one clock.** Every video frame carries its capture time in
  TAI (LCP SEI, see PROTOCOL.md), from the phone's own chrony-like clock
  discipline. `tools/sync-probe` takes its receive times from the same
  discipline (the core, same pools) running on the receiving machine, and
  prints that clock's health. The Mac's own OS clock was 6 ms ahead to 65 ms
  behind during these tests, so measuring against the OS clock would be
  wrong by as much as the effects we look for. With both disciplines settled,
  phone and receiver agree within a few ms (glass to SRT minus the phone's
  own capture-to-send was the SRT latency plus 3 to 7 ms in every run).
- **Wait for a small max error, not only "healthy".** Right after a start,
  both ends report `healthy` with a max error of 70 to 145 ms, and one run
  just after reinstalling the app read 70 ms too much. The frames carry the
  sender's max error (`unc` in sync-probe's output), and `--wait-sync` now
  waits for a receiver max error below 10 ms.
- **Three points on the receiver**, p50/p95/max per second:
  - `glass->srt`: when `srtsrc` hands the SRT payload to the application.
    The probe parses TS, PES and the SEI itself (no `tsdemux`), so this is
    the phone plus the network plus the SRT receive buffer (TSBPD).
  - `demux`: after `tsdemux` and `h264parse` (appsink, `sync=false`).
  - `playout`: when a syncing sink renders the frame (appsink, `sync=true`),
    as a GStreamer receiver that plays out on its clock does.
- **Stages on the phone**, on its host clock: `status.stream.latency`
  (PROTOCOL.md), shown in the control hub (the LAT chip and the Latency panel).
- **A/V**: `sync-probe --av` decodes the stream with the PTS exactly as sent
  (`decodebin`, AAC or Opus) and times a flash and a beep against the frame
  capture times. `fake-camera` is the zero reference (flash and beep at
  0.0 ms of each other after its priming correction, `--audio-codec aac`
  or `opus`).

## Budget

Video, p50 in ms. "Before" is the app as it was (SRT latency 500 from the
hub's default URL); "after" is the current app at 120 ms (the SRT default)
and at 60 ms (the lowest clean setting measured here).

| Stage | Before, 500 ms | After, 120 ms | After, 60 ms |
|---|---:|---:|---:|
| Camera: capture time to encoder input | (not instrumented) | 15 to 38 (varies between runs) | 15 to 25 |
| Encoder (VideoToolbox) | | 10.2 | ~10 |
| MPEG-TS mux | | 0.6 | 0.6 |
| SRT send queue (first packet) | | 0.1 | 0.1 |
| **Phone total, capture to first packet sent** | **~49** | **39 to 49** | **26 to 34** |
| Network + SRT receive buffer (TSBPD) | ~500 | 124 to 126 | 65 to 67 |
| **Glass to SRT payload (`glass->srt`)** | **549** | **165** | **92 to 99** |
| `tsdemux` + `h264parse` (not syncing) | +222 | +34 | +34 |
| Glass to demux | 771 | 198 | 126 to 133 |
| GStreamer syncing receiver: srtsrc reports the SRT latency again, `tsdemux` `latency` 700, PCR lead | +1098 | +922 | +867 |
| **Glass to playout, GStreamer defaults** | **1869** | **1120** | **1000** |
| Glass to playout, `tsdemux latency=0` | | | **301** |

Audio: see "Audio budget" below.

The phone's video path did not get much faster: the old app already sent a
frame about 45 to 50 ms after capture, and the same scene at 120 ms gave 167
ms before and 165 ms after. The camera stage moves by 20 ms between runs
(15 to 38 ms at the same 1/40 s shutter), and audio with it, so compare
stages within a run. Where the time went in the old setup:

1. **SRT latency 500 ms** (the hub's default URL then), paid in full: SRT holds
   every packet until its send time plus the latency (TSBPD), whatever the
   network does.
2. **The receiver.** A GStreamer pipeline `srtsrc ! tsdemux ! ... ! sink
   sync=true` adds `tsdemux`'s `latency` property (default 700 ms) and the
   SRT latency a second time, because `srtsrc` reports it as its latency
   while its buffers are already timestamped after TSBPD. At 500 ms that is
   1.2 s of the 1.87 s.
   The 2.1 s measured earlier with the old sync-probe (`tsdemux`,
   `h264parse`, appsink `sync=false`, OS clock) is not reproduced: the same
   old binary read about 0.6 s against today's app at 500 ms, and the
   non-syncing demux view here is 0.77 s. What made the earlier run slower
   is not known; 2.1 s is close to the syncing playout figure (1.87 s plus
   the Mac clock's 60 ms).
3. **The PCR lead**: a receiver holds each sample for `PTS - PCR`. It was a
   fixed 100 ms behind the video DTS, which left the AAC audio short (as
   little as -0.9 ms: some audio arrived late).
4. **`tsdemux` + `h264parse`** add one frame (33 ms) at 120 ms and below,
   and 220 ms at 500 ms (not investigated further; it is receiver side).

## What changed

- **Measurement**: `sync-probe` and `fake-camera` use the core (SEI writer
  and parser, TAI handled, the clock discipline as their time reference);
  `sync-probe` has the SRT-layer view, the playout view and `--av`; the app
  reports its per-stage latency in `status.stream.latency`; the hub shows it.
- **SRT latency is a stream setting**, `srt_latency_ms` in the hub. It sets
  the URL's `latency` parameter, and the phone reports the latency the
  listener agreed to (`status.stream.srt_latency_ms`, read from the
  listener's handshake response). The sender's drop thresholds follow the
  agreed latency.
- **MPEG-TS timing**: PTS is the capture time on the host clock (no time
  origin, no 1 s offset). The PCR is the time of muxing minus a fixed PCR
  delay, sized to the slowest stream: 150 ms with AAC (audio is 120 to
  128 ms from capture to mux), 80 ms for video only (and, since the audio
  unit and Opus, with Opus: see "Audio budget"). Samples that miss it
  are counted (`late_video_frames`, `late_audio_frames`); 0 in every run
  since. Video PTS - PCR went from 100 to about 110 ms (audio needs it),
  audio from 27 ms (min -0.9) to 46 ms (min 22). The 1 s offset itself never
  added latency: it was on PTS and PCR alike.
- **Pacing**: a token bucket at 50 Mbit/s with a 32-packet burst instead of
  sending a tenth of the queue per call. A frame's last packet now leaves
  0.3 ms (p50) after it was queued, p95 0.5 ms, max 7 to 10 ms (keyframes).
  This keeps the SRT latency budget for retransmissions.
- **VideoToolbox low-latency rate control** (Constrained High),
  `MaxFrameDelayCount` 0, `PrioritizeEncodingSpeedOverQuality`. Measured
  A/B: encode 10.8 ms p50 without, 10.3 ms with. The hardware encoder in
  RealTime mode without frame reordering already emits every frame at once;
  the gain is small but there was no downside.
- **Audio I/O buffer** 5 ms (`setPreferredIOBufferDuration`): capture
  buffers are now about 5 ms long. It hardly changed the audio latency,
  which is in AVCaptureSession's own delivery (see below).
- Landscape is 4 ms faster than portrait (the capture connection rotates
  portrait frames, and the encoder gets them in another layout): 44.6 vs
  48.5 ms capture to first packet.

## Audio budget

Before: the microphone came from AVCaptureSession
(AVCaptureAudioDataOutput) and was encoded to AAC-LC. After: the
microphone is a RemoteIO audio unit outside the capture session (audio
session mode `measurement`, no voice processing, 5 ms I/O buffers, 48 kHz
mono), encoded to Opus with 10 ms frames (Apple's encoder,
AVAudioConverter, `kAudioFormatOpus`). AAC stays available
(`audio_codec`), now also fed from the audio unit.

p50 in ms (p95 in brackets where useful), iPhone 16, Wi-Fi, SRT latency 0,
30 s per run, `sync-probe --raw --latency-ms 0`. "Before" was measured
earlier with AVCaptureSession + AAC. The two "after" columns were measured
on 2026-10-15 with the audio unit, once per codec (`audio_codec`). There
were 0 late audio frames and 0 late video frames in both runs.

| Stage | Before: AVCaptureSession + AAC | After: audio unit + AAC | After: audio unit + Opus |
|---|---:|---:|---:|
| Capture to buffer at the encoder (`audio_capture_to_encode_ms`) | ~61 | 7.9 | 8.0 (p95 8.1) |
| Encode (`audio_encode_ms`) | (in the 63) | 1.0 | 0.7 |
| Encoder delay in the PTS | 44.0 (2112 samples) | 44.0 | 6.5 (312 samples) |
| **Audio PTS to TS packets (`audio_capture_to_mux_ms`)** | **~124** | **69.1** | **22.4 (p95 24.6, max 25.2)** |
| **Audio glass to SRT payload (`glass->srt` a)** | **~137** | **81.0** | **28.7 (p95 43.7)** |
| Video glass to SRT payload, same run | ~50 | 52.1 | 50.6 |
| PCR delay | 150 | 150 | 80 |
| Video PTS - PCR (receiver buffer), min | | 83.8 | 12.1 |

The audio unit saves about 53 ms (AVCaptureSession's late hand-over). Opus
saves another 47 ms (a shorter frame and a smaller encoder delay). Audio now
reaches the receiver about 22 ms before video. With Opus, video sets the PCR
delay: video PTS - PCR reached a minimum of 12 ms with 80 ms, so video
takes up to about 68 ms from capture to mux. 80 ms stays; a smaller value
would make video frames late.

`audio_capture_to_mux_ms` and sync-probe's `glass->srt a` count from the
audio PTS, which is the capture time of the frame's first sample minus the
encoder delay, so both include that delay.

Checked without the phone (the app's own muxer and Apple's encoders,
compiled for macOS, writing a TS with a flash and a tone at the same
capture instant every second):

| Decoder | Opus: audio - video | AAC: audio - video |
|---|---:|---:|
| GStreamer `tsdemux ! decodebin` (opusdec, avdec_aac) | +0.02 ms | +0.01 ms |
| FFmpeg 9.0 (`-copyts`, native opus and aac decoders) | +0.02 ms | +0.01 ms |

So the PTS formula and Apple's 312-sample Opus pre-skip match what both
decoders output (0.02 ms is one sample of the tone detector). `ffprobe` and
`gst-discoverer` report Opus, 48 kHz, 1 channel, and both decode with no
errors.

### Audio timestamps

Each microphone buffer from the audio unit's input callback carries an
`AudioTimeStamp`. The capture time of its first sample, on the host clock
(the clock of the camera's sample buffer times and of the core's clock
mapping):

```text
capture_ns(buffer) = host_ns(AudioTimeStamp.mHostTime) - AVAudioSession.inputLatency
capture_ns(sample k of the buffer) = capture_ns(buffer) + k * 1e9 / 48000
```

`host_ns` converts mach ticks to ns (`HostClock.ns(fromTicks:)`, the
core's `lc_host_ticks_to_ns`). `mHostTime` is the host time Core Audio
assigns to the buffer's first sample at the I/O unit; the input latency
(re-read on every route change) is the converter and driver latency before
that, so the result is when the sound reached the microphone.

Each encoder frame (480 samples for Opus, 1024 for AAC) is stamped with the
capture time of its first sample, interpolated within the buffer it starts
in (`AudioFramer`): every frame is stamped from its own buffer's time, so
the stamps follow the microphone clock against the host clock without
drift. Then:

```text
audio PTS = capture_ns(first sample of the frame) - priming / 48000 s
priming   = 312 (Opus pre-skip, as the encoder reports) | 2112 (AAC)
```

A decoder that does not skip the priming (MPEG-TS has no pre-skip field;
no trim flags are sent) outputs input sample k of the frame at
`PTS + (priming + k) / 48000`, which is its capture time.

What has not been checked: whether `mHostTime - inputLatency` is the true
acoustic instant on an iPhone (the clapper test, with a person present),
and how the input latency reported for other routes (wired headset) holds.

### Notes

- Opus in MPEG-TS costs more TS overhead than AAC: 100 PES per second, each
  in its own 188-byte TS packet (about 150 kbit/s on the wire for an
  80 kbit/s stream, silence included). Two frames per PES would halve it
  at 10 ms more latency.
- Apple's Opus encoder has no setting for the application mode. It reports
  a 312-sample pre-skip (6.5 ms), the look-ahead of the standard modes; the
  restricted low-delay mode (2.5 ms) is not available through
  AVAudioConverter. libopus in the core would allow it (4 ms less), not done.
- The microphone runs whenever the app runs (as before with
  AVCaptureSession), so starting a stream does not wait for the audio unit.

## SRT latency on Wi-Fi

Phone to MacBook on the same home Wi-Fi, RTT about 6 ms, 2 minutes each,
plus a 5-minute soak at 60 ms: 9000 frames, 0 lost, 0 dropped and 0
retransmitted on both ends, glass to SRT 92 / 98 / 120 ms (p50 / p95 / max).

| SRT latency | Glass to SRT p50 / p95 / max | Receiver lost / dropped | Sender retransmits | Sender drops |
|---:|---|---|---:|---:|
| 500 | 549 / 555 / 580 | 0 / 0 | 0 | 0 |
| 200 | 245 / 248 / 276 | 0 / 0 | 0 | 0 |
| 120 | 165 / 170 / 191 | 0 / 0 | 0 | 0 |
| 80 | 121 / 127 / 151 | 0 / 0 | 0 | 0 |
| 60 | 99 / 104 / 124 | 0 / 0 | 0 | 0 |
| 40 | 80 / 86 / 115 | 0 / 0 | 0 | 217 |
| 20 | 60 / 66 / 151 | 0 / 0 | 0 | 3110 |

At 40 ms and below the receiver still got everything, but the sender gave
up packets that were not acknowledged within 1.5 x the latency (the listener
acknowledges about every 10 ms), so a lost packet could no longer be resent.
60 ms is the lowest clean setting here; 80 to 120 ms leaves room for a Wi-Fi
that is busier than this one.

Recommendation: the hub hands out 100 ms by default
(`srt://<address>:9000?streamid={device}&latency=100`). On Wi-Fi, 60 ms is the
floor. 5G needs more: set the latency to roughly 4 x RTT or above, with
`--srt-url`. The default in the app (a URL without `latency`) is 120 ms.

## Receivers

What a receiver adds is not in this repository, but it is now the larger
part. For a GStreamer receiver such as Strom's SRT input:

- Set `tsdemux` `latency` to a small value (this stream's PCR delay already
  covers its own timing). Measured at 60 ms SRT latency: playout 1000 ms
  with the default 700, 301 ms with 0.
- Do not count the SRT latency twice: `srtsrc` timestamps buffers on
  arrival, after TSBPD, and still reports the SRT latency in the latency
  query.
- What is then left is the PCR lead (about 110 ms for video, to wait for
  the AAC audio) and one frame in `tsdemux` + `h264parse`.

## Audio and A/V sync

- Audio and video PTS are both capture times on the same host clock; the
  encoder delay (Opus 312 samples, AAC 2112) is taken off the audio PTS,
  because decoders do not skip it in MPEG-TS. The receiver tool confirms
  the relation on the wire (`--av` decodes with the sent PTS; fake-camera
  reads 0.0 ms with AAC and -0.1 ms with Opus, and an artificial 250 ms
  shift reads -228.7 ms, the shift plus fake-camera's then uncorrected
  21.3 ms priming).
- With AAC, audio was the slow stream and set the PCR delay (150 ms). With
  Opus from the audio unit, video is the slower stream again and the PCR
  delay is the video's 80 ms.
- Both are now replaced: see "Audio budget". The new timestamps need the
  physical A/V test below before they can be trusted on the phone.

### A/V test with a phone: the digital clapper

The control hub serves a clapper at `http://<hub>:8090/clapper` (the
"Clapper" button in the header): the whole screen flashes white and a 1 kHz
beep plays at every whole second of that device's clock, longer (300 ms
flash, 150 ms beep) on seconds divisible by 5. Open it full screen on a
laptop, tablet or phone with its built-in speaker (not Bluetooth), film it
with the camera under test from a metre or so, and run `sync-probe --av`.
For each event it prints flash and beep against the whole second and audio
minus video; the summary has the distribution.

- The beep is scheduled with `AudioContext.getOutputTimestamp()`, so the
  output latency the browser knows about is compensated; the flash is drawn
  in the animation frame shown at the whole second. What the clapper device
  does not know about (speaker latency beyond what the browser reports,
  display latency beyond one refresh) is an error in the reference; filming
  the same clapper with several cameras cancels it between them.
- The camera samples the flash once per frame, so a single event is only
  good to +-17 ms at 30 fps; take the median over a minute of events (the
  phase between display and camera drifts, so the median gets well below a
  frame).
- Which second: the probe's "beep to second" is modulo one second. Long
  beeps are timed against their 5-second mark ("marked beep to 5 s"), which
  is unambiguous within +-2.5 s; flash and beep are paired only when less
  than 0.5 s apart.
- `fake-camera --local [--audio-only]` does the same with GStreamer on the
  Mac. It prints its playout latency (230 ms here), but not the speakers'
  own latency.

Measured so far: the phone hearing `fake-camera --local --audio-only` from
across the room put the marked beeps at +348.5 ms (p50, p95 351.5) after
their 5-second mark, and the Mac emits them about 263 ms after it (OS clock
+33 ms plus 230 ms playout). The remaining 67 to 85 ms between two runs is
the Mac's unreported speaker latency, the air path and any phone timestamp
error together; this setup cannot separate them. So the phone's audio time
stamps are stable within a run (+-5 ms), but A/V on the phone is not yet
measured: that needs the clapper filmed, or a clap in front of the camera.

## SRT and WHIP

What WebRTC (WHIP) would change for this path:

- **No fixed receive buffer.** SRT holds every packet for the full latency,
  even on a clean network. A WebRTC receiver's jitter buffer adapts to the
  measured jitter (tens of ms on a LAN) and grows only when packets come
  late or are lost.
- **Congestion control.** SRT live mode sends at the encoder's rate and only
  drops what is too late; WebRTC (GCC/TWCC) lowers the encoder bitrate
  before queues build up, which matters more on cellular than here.
- **Timing.** RTCP sender reports map RTP time to NTP time, and the
  `abs-capture-time` header extension carries the capture time per frame,
  so a receiver can align cameras without our SEI (the SEI would still
  survive any transport). Audio would be Opus, which cuts the AAC delay.
- **Cost.** NACK-based recovery has the same RTT bound as SRT; FEC adds
  bandwidth. On a clean LAN, SRT at 60 to 80 ms and WebRTC end up close;
  the difference shows on networks with variable delay.

## Reproducing

```sh
cd tools/sync-probe && cargo run --release -- --latency-ms 120 --wait-sync 9101
# point a phone at srt://<this machine>:9101?streamid=cam-x&latency=120,
# or run tools/fake-camera against it; --av for A/V, --duration-s for a summary
```

Audio budget on the phone: set `audio_codec` (opus, then aac) in the hub,
go live against `sync-probe --raw --latency-ms 0 --duration-s 30 9101` with
the phone's URL at `latency=0`, and read the phone's `status.stream.latency`
(the hub's Latency panel) next to sync-probe's `glass->srt` video and audio
summary.

## Clapper test, 2026-10-15 (iPhone 16, Opus, 30 fps, SRT 100 ms over the USB link)

Reference: `/clapper` on the Mac's own screen and speakers, phone about one
metre away. Both clocks Healthy (phone max error about 3 ms).

| | p50 | p95 | n |
|---|---:|---:|---:|
| Beep stamped vs the true second | -7.1 ms | -7.0 ms | 65 |
| Marked beep vs its 5 s mark | -7.1 ms | -7.0 ms | 13 |
| Flash stamped vs the true second | +35.1 ms | +68.6 ms | 64 |
| Audio - video in the stream | -42.1 ms | -39.1 ms | 64 |

- Audio timestamps are accurate to within about 10 ms and stable to 0.3 ms.
  The small negative offset suggests the input latency correction or the
  clapper's output latency compensation slightly overshoots.
- The flash is bimodal (+35 / +68 ms): one 33 ms frame period apart, the
  frame phase relative to the flash. The figure also contains the Mac's
  unknown display latency (typically 15-35 ms), so the video stamp error cannot
  be separated here. Next: a clapper on an iPad or second iPhone, and a 60 fps
  run to halve the frame-phase spread.

## Formats and codecs, 2026-10-16 (iPhone 16, Wi-Fi, SRT 100 ms)

`sync-probe --raw --no-playout --latency-ms 100 --duration-s 20`, Opus audio,
bitrate the format's default (1080p30 H.264 at 3000 kbit/s, the phone's saved
value). Both clocks Healthy. Thermal state stayed `nominal` throughout,
including a 3-minute 2160p60 HEVC run (phone on the charger).

| Format | Codec | kbit/s | glass->srt p50 / p95 ms | encode p50 ms | delivered fps | lost |
|---|---|---:|---:|---:|---:|---:|
| 1080p30 | H.264 | 3000 | 147.1 / 148.9 | 8.3 | 30 | 0 |
| 1080p30 | HEVC | 4000 | 147.1 / 148.9 | 7.8 | 30 | 0 |
| 2160p30 | HEVC | 12000 | 161.5 / 163.7 | 21.5 | 30 | 0 |
| 2160p60 | HEVC | 18000 | 150.9 / 154.6 | 17 | 60 | 0 |
| 1080p120 | HEVC | 9500 | 132.5 / 135.2 | 7.2 | 120 | 0 |
| 1080p240 | HEVC | 14500 | 122.1 / 124.2 | 6.1 | 239 | 0 |
| 1080p120 | H.264 | 14000 | 132.5 / 135.9 | 7.9 | 120 | 0 |
| 2160p60 | H.264 | 25500 | 164.2 / 174.5 | 23.8 | 41 | 0 |

- H.264 runs with VideoToolbox low-latency rate control, which encodes one
  frame at a time: about 8 ms per 1080p frame and 24 ms per 2160p frame. That
  caps it near 120 fps at 1080p (240 delivers about 120) and 41 fps at 2160p.
  Without it H.264 buffers frames (encode 11 ms at 1080p30, 145 ms at 2160p60).
- HEVC has low-latency rate control on iOS 26.6 too, with the same one frame at
  a time limit (25 ms at 2160p: 2160p60 delivered 39 fps, 1080p240 115 fps).
  Its default rate control with `MaxFrameDelayCount` 0 emits every frame
  before the next one and keeps up, at the same or lower glass->srt latency,
  so HEVC runs without it.
- glass->srt falls with the frame rate (the camera hands frames over sooner)
  and rises with 2160p by the longer encode.
- The phone warns in the hub log when the camera drops frames because the
  encoder does not keep up.

## MPEG-TS muxer in the core, 2026-10-16 (iPhone 16, Wi-Fi, SRT 100 ms)

The muxer moved from Swift into the core (`core/src/ts`), which now also
queues the packets on its SRT caller. Before the Swift muxer was removed,
both ran in the same build (a launch switch), with the same frames: on the
simulator the output was byte-identical, chunk by chunk (synthetic frames of
every size around the packet boundaries, VideoToolbox H.264 and HEVC, Apple
Opus and AAC). On the phone, `sync-probe --raw --no-playout --latency-ms 100
--duration-s 35 --wait-sync` (about 30 s of stream), p50 / p95 in ms:

| Format | Muxer | glass->srt video | glass->srt audio | PTS - PCR video | PTS - PCR audio | phone mux | lost / late |
|---|---|---:|---:|---:|---:|---:|---:|
| 1080p30 H.264 + Opus | Swift | 139.7 / 141.5 | 125.8 / 128.7 | 43.5 / 44.5 | 74.4 / 89.7 | 0.3 / 0.3 | 0 / 0 |
| | core | 140.2 / 141.7 | 126.4 / 129.3 | 43.6 / 44.5 | 74.5 / 89.6 | 0.2 / 0.3 | 0 / 0 |
| 1080p30 H.264 + AAC | Swift | 141.9 / 144.7 | 174.7 / 177.8 | 113.6 / 114.3 | 97.7 / 112.8 | 0.3 / 0.3 | 0 / 0 |
| | core | 141.9 / 145.0 | 175.0 / 178.3 | 113.6 / 114.6 | 97.6 / 112.5 | 0.1 / 0.1 | 0 / 0 |
| 1080p30 HEVC + Opus | Swift | 139.2 / 140.6 | 126.0 / 128.9 | 44.2 / 44.9 | 74.2 / 89.5 | 0.2 / 0.4 | 0 / 0 |
| | core | 139.5 / 141.2 | 126.4 / 129.4 | 44.3 / 45.1 | 74.3 / 89.5 | 0.1 / 0.2 | 0 / 0 |
| 2160p60 HEVC + Opus | Swift | 151.1 / 153.5 | 125.5 / 128.6 | 32.5 / 34.5 | 66.0 / 74.6 | 0.5 / 0.9 | 0 / 0 |
| | core | 152.5 / 154.9 | 127.4 / 130.5 | 32.5 / 35.0 | 66.1 / 74.6 | 0.2 / 0.4 | 0 / 0 |

"phone mux" is the app's `mux_ms` stage: encoder output to the frame queued
for SRT (with the core it includes the copy onto the SRT queue). The
receiver-side differences are within the run-to-run spread (the phone's
capture-to-send was 47.6 ms with the core and 49.2 ms with Swift in the
2160p60 pair). The PTS - PCR leads, which show the PCR delay and the audio
timing, are the same.
