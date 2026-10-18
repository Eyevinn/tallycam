# Device capabilities worth using

Research notes, October 2026. These are the parts of an iPhone (and its camera)
beyond "picture and sound over SRT" that could matter for TallyCam. Nothing
here is implemented unless it says so. API facts come from Apple's
documentation and WWDC material (links at the end); model and latency details
need checking on devices.

## At a glance

| Capability | Value for TallyCam | Effort | When |
|---|---|---|---|
| Local ISO recording | Full-quality backup on the phone, survives the network | Medium | Soon |
| Gyro and lens data per frame | Camera tracking (pan, tilt, zoom, focus) for graphics; "camera is moving" for the director | Low to medium | Soon |
| Tally and operator controls | Screen border, torch, haptics; Camera Control button, Action button, AirPods stem | Low | Soon |
| Timecode | SMPTE timecode from our TAI clock in stream and recordings | Low | Soon |
| Virtual PTZ | Capture 4K, send a remotely steered 1080p crop | Medium | Later |
| Two lenses at once | Two angles from one phone (wide and tele) | Medium | Later |
| HDR (HLG) | Better picture, broadcast standard | Low in the app, high in Strom | When Strom does HDR |
| On-device ML | Auto framing, subject tracking, lens smudge warning, live captions | Medium | Later |
| Audio | Stereo/spatial capture, voice isolation, AirPods as a wireless mic, USB-C mics | Low to medium | Later |
| High frame rate | 120 fps locally for slow-motion replays | Medium | Later |
| Cinematic mode | Shallow depth of field, focus pulls from the gallery | Unknown | Spike |
| Network | Wi-Fi and 5G at once (bonding), USB-C Ethernet | Medium | With SRTLA |

## Details

### Local ISO recording

Record a high-quality copy on the phone (HEVC at a high bitrate, or ProRes on
Pro models) while streaming a contribution-quality version. Stamped with TAI
and timecode, the copy lines up with the live programme afterwards. It covers
network drops and gives post-production clean ISO feeds. Pro iPhones from
the 15 Pro have USB 3, which lets them record to an external SSD. Thermal load
and storage are the limits.

### Gyro and lens data per frame

Core Motion gives the phone's attitude at 100 Hz or more. Together with the
lens state (focal length or zoom factor, focus distance, and the camera's
intrinsic matrix, which AVFoundation can deliver per frame), a phone on a
fluid head reports pan, tilt, roll, zoom and focus: the same fields as FreeD,
the de facto camera tracking protocol for virtual studios and AR graphics.

- Carry it per frame in our SEI, so it is time-aligned with the picture and
  survives any transport. Strom could output FreeD to graphics engines.
- Position (X/Y/Z) needs ARKit world tracking, but ARKit takes over the camera
  and limits manual control. Rotation and lens data alone work with our
  normal capture session. Existing apps (FREED Tracker, Epic's Live Link VCam)
  show the demand.
- Also useful without graphics: Strom and the director see when a camera is
  moving ("don't cut to camera 3, it is panning"), and the phone can warn
  about a crooked horizon.

### Tally and operator controls

- Tally: screen border (done), the torch as a front tally light, and haptics
  for cues to the operator.
- Physical controls: iOS 26 lets capture apps map the Camera Control button,
  the volume buttons and AirPods stem clicks (H2 AirPods) to actions
  (`AVCaptureEventInteraction`). Uses: "call the gallery", "I'm ready",
  "go live" with consent. The Camera Control's zoom and exposure sliders are
  in (docs/UI_DESIGN.md, "Picture").

### Timecode

iOS 26 has a timecode generator for capture (`AVCaptureTimecodeGenerator`).
Driving it from our disciplined TAI clock gives every camera the same SMPTE
timecode, in the stream and in local recordings, without a timecode box.
How it is fed and whether it reaches our own encoder output needs checking.

### Virtual PTZ

Capture 4K and send a 1080p crop that the gallery steers: pan, tilt and zoom
without a motor. Combined with subject tracking (ML), the crop can follow a
speaker. Costs: more power and heat, and our encoder input becomes a scaled
crop (VideoToolbox can do the scaling).

### Two lenses at once

`AVCaptureMultiCamSession` (iPhone XS/XR and later) captures two cameras at
the same time, for example wide and tele, as two streams from one phone. It
fits "many angles cheaply". Limits: lower resolutions and frame rates in
multi-cam formats, two encoders and twice the bitrate. Only one input can
follow an external sync device or run at a locked frame duration (GENLOCK.md).

### HDR (HLG)

iPhone 12 and later capture HDR as 10-bit HEVC, and Apple's Dolby Vision
stream stays decodable as HLG by decoders without Dolby Vision. HLG
(BT.2100) is the broadcast HDR standard and shows acceptably on SDR displays.
`AVCaptureColorSpace.HLG_BT2020` selects it in capture. Pro models from the
15 Pro also record Apple Log, a flat profile for grading.

Why not now:

- 10-bit means HEVC Main 10. H.264 in hardware is 8-bit only.
- The production must be HDR end to end. Strom can encode H.265 Main 10, but
  no HLG or BT.2020 handling turned up in a search of its code: the vision
  mixer, graphics and SDR down-conversion would need to understand HDR.
- Mixed SDR and HDR cameras need consistent conversion, or cuts change look.

Suggestion: an HLG option in the camera configuration from Strom, off by
default. Turn it on when Strom handles HDR. Apple Log with a shared LUT
applied on the phone is a related idea: the same look on every camera, which
helps camera matching.

### On-device ML (Vision, Neural Engine)

- Face and person detection for auto framing (with virtual PTZ) and for
  metadata to the director ("two people in shot").
- Lens smudge detection (new in iOS 26): warn the operator and the gallery.
- On-device speech recognition for live captions or a transcript as
  metadata.
- Cost: power and heat on top of encoding; measure.

### Audio

- iOS 26 adds spatial audio capture and "studio-quality" AirPods microphones
  in camera apps: a reporter's AirPods as a wireless mic.
- Voice isolation and wind noise reduction for field use.
- USB-C microphones and interfaces for professional audio.

### High frame rate

Pro models capture 4K at 120 fps. Record locally at 120 fps while streaming
at 30 or 60, and send replay clips on request.

### Cinematic mode

iOS 26 opens a Cinematic capture API: simulated aperture, tracking focus and
focus changes. A remote rack focus from the gallery would be striking. The
latency, resolution and frame rate limits are unknown; needs a spike.

### Network

- Network.framework can bind a connection to an interface, so Wi-Fi and 5G
  can be used at the same time: the basis for SRTLA bonding.
- USB-C Ethernet on every USB-C iPhone and iPad.

### Status (partly done)

Thermal state and battery are reported already. Location could place cameras
on a map (news, crowd contribution).

## Cautions

- **Stabilization adds latency and crops the picture.** The stronger modes
  buffer frames. Keep it off or standard for live, measure each mode.
- **ARKit takes over the camera.** Full tracking and full manual control are
  hard to combine.
- **Heat is the shared budget.** HDR, 4K, two cameras, ML and local recording
  all add load on top of encoding and 5G.

## Sources

- [Apple: AVCaptureColorSpace](https://developer.apple.com/documentation/avfoundation/avcapturecolorspace)
- [Apple: Incorporating HDR video with Dolby Vision into your apps (PDF)](https://developer.apple.com/av-foundation/Incorporating-HDR-video-with-Dolby-Vision-into-your-apps.pdf)
- [AVFoundation HDR and Apple Log (blog)](https://blakecrosley.com/blog/avfoundation-hdr-apple-log)
- [WWDC25 Camera & Photos group lab summary](https://developer.apple.com/forums/thread/791234)
- [WWDC25 session 253: capture controls](https://developer.apple.com/videos/play/wwdc2025/253/)
- [Apple Developer Forums: iPhone 17 Pro's Genlock feature (timecode sources)](https://developer.apple.com/forums/thread/799739)
- [FREED Tracker (App Store)](https://apps.apple.com/ml/app/freed-tracker/id6771544166)
- [Epic Games: Live Link VCam](https://www.cgchannel.com/2021/09/epic-games-releases-free-ios-app-live-link-vcam-for-ue4/)
