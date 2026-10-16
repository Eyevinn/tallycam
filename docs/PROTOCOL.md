# Live Contribution Protocol (LCP) v0

Draft, prototype. Breaking changes are expected until v1.

LCP lets a server (Strom, Open Live, or the standalone `control-hub`) see and
control a camera phone. It is platform neutral: an Android app speaks the same
protocol.

## Principles

- **The phone connects out.** The phone opens the WebSocket to the server, so
  it works behind 5G carrier NAT. The server never connects to the phone.
- **The phone describes what it can do.** On connect it sends a list of
  controls with types and ranges, read from the actual hardware. The server
  renders its UI from that list and never assumes a control exists.
- **State is owned by the phone.** The server asks for a change with `set`. The
  phone applies what the hardware allows and reports the real result in
  `state`. The UI shows `state`, not what it asked for.
- **Media is separate.** Video and audio go over SRT (or WHIP). LCP only
  carries control, status, tally and a low-rate preview image.

## Transport

- WebSocket, one JSON object per text frame, each with a `type` field.
- Device endpoint: `ws(s)://<host>/lcp/v0/device`
- Controller endpoint (operator UI): `ws(s)://<host>/lcp/v0/control`
- Authentication: the phone proves its device key (see "Device keys"); a
  server may also require a one-time invite token for phones it does not
  know yet. No TLS yet: use `wss://` behind a proxy where it matters.
- Unknown `type` values and unknown fields are ignored, never an error.

## Provisioning v0

How a phone learns where to connect: the server shows a QR code, the phone's
Camera app opens it in the app. On an open server anyone on the network who
sees the code can join (as with the open device endpoint itself); an
invite-only server puts a one-time invite in the link instead (see
"Admission").

### Deep link

```
tallycam://connect?control=<ws url>&srt=<srt url>
```

- The scheme `tallycam` is one constant on each side: `DEEP_LINK_SCHEME` in
  the hub library, `ProvisioningLink.scheme` in the app, and
  `CFBundleURLTypes` in `ios/project.yml`.
- Both values are percent-encoded (everything but RFC 3986 unreserved
  characters).
- `control` (required): the device endpoint, `ws://` or `wss://`.
- `srt` (optional): the SRT URL to send media to. A literal `{device}` in it is
  replaced by the phone with its own stream id, `cam-` and the first 8
  alphanumerics of its device id, lowercased (`cam-3f2a9c1d`). A URL without
  `{device}` is used as is (a fixed `streamid`).
- Unknown parameters are ignored.

The phone saves both URLs (they replace any earlier ones) and reconnects the
control channel. While it is live, the new SRT URL applies on the next Go
Live. A link with an unusable `control` (not ws/wss) or `srt` (not srt) is
ignored as a whole.

### Stream URL

```
srt://<host>:<port>?streamid=<id>&latency=<ms>&passphrase=<secret>&pbkeylen=<16|24|32>
```

The phone is always the SRT caller; the receiver (Strom) listens.

- `streamid` (optional): sent in the handshake (SRTO_STREAMID), up to 512
  bytes. Percent-encode reserved characters (`#!::r=cam1` is
  `%23!%3A%3Ar%3Dcam1`).
- `latency` (optional, ms, default 120): SRTO_LATENCY. SRT plays out with the
  larger of the phone's and the listener's, reported in
  `status.stream.srt_latency_ms`. The `srt_latency_ms` control sets it.
- `passphrase` (optional, 10 to 79 characters): turns on AES encryption. The
  listener must have the same passphrase; a listener with another passphrase,
  or without one, rejects the connection and the phone stops with the reason.
  Percent-encode it in the URL (`+` decodes to a space). `status.stream.url`
  and the phone's log lines show it as `***`; the `stream_url` value keeps it.
- `pbkeylen` (optional): AES key length in bytes, 16 (AES-128, the default
  with a passphrase), 24 or 32 (AES-256). libsrt listeners take the key length
  from the caller.
- `mode` may only be `caller`; other parameters are ignored.

A stream that lost its connection while live keeps encoding and connects
again every second (`status.stream.state` `connecting` with the reason in
`stream.error`) until it is back or stopped. A first connection that fails
(nobody listening, wrong passphrase, stream id rejected) stops the stream.

### Server side (control hub)

- `GET /api/info`: what the server advertises.

  ```json
  {
    "port": 8090,
    "addresses": ["192.0.2.10"],
    "targets": [{
      "label": "192.0.2.10",
      "control_url": "ws://192.0.2.10:8090/lcp/v0/device",
      "srt_url": "srt://192.0.2.10:9000?streamid={device}&latency=100",
      "deep_link": "tallycam://connect?control=ws%3A%2F%2F192.0.2.10..."
    }],
    "scheme": "tallycam"
  }
  ```

  `addresses` are the LAN addresses: IPv4 except loopback and link-local, plus
  global IPv6 when listening on `::`; physical interfaces before VPN tunnels and
  VM bridges. A server bound to one address advertises only that one, never
  `localhost`. One target per address, or exactly one when a public control
  URL is configured.
- `GET /api/connect.svg?target=<index>` (default 0): the target's deep link as
  a QR code (`image/svg+xml`, one unit per module, scale it with CSS).
- `POST /api/invite?target=<index>` (default 0): a one-time invite,
  `{deep_link, control_url, srt_url, qr_svg, expires_in_secs}`; the control
  URL carries the token (`?token=`). With `--invites` the operator page shows
  this instead of the plain deep link.
- `DELETE /api/devices/{id}`: forget (revoke) a device, 204, or 404 for an
  unknown id. The page's "Forget" button.
- Operator token: with `--operator-token` (or `--invites`, which makes one and
  logs the operator page link `http://<host>:<port>/#token=...`), creating
  invites and forgetting devices need the `X-Operator-Token` header, and the
  operator socket `?token=`. A header, so another site cannot forge these
  from the operator's browser. The page takes the token from its URL
  fragment, which never reaches the server. Without a token the hub is open,
  for a trusted LAN.
- Configuration: `--public-url` replaces the derived control URL (behind a
  proxy, see "Deployment" below); `--srt-url` sets the SRT URL handed out
  (default `srt://<address>:9000?streamid={device}&latency=100`);
  `--invites` requires an invite (and so a device key) from new phones;
  `--state-file` keeps known devices and their keys (the key file of the
  hub's own tools is written owner-only, 0600, and never overwritten).

## Device keys

A phone proves which camera it is with a P-256 key pair; the server never
trusts a claimed device id alone. Code is the source of truth
(`core/src/device_key.rs`, shared by the app, the hub and Strom).

### Key and id

- The app creates the key on first launch: in the Secure Enclave on a
  device, a software key in the Keychain on the simulator, stored "this
  device only" (no backup, no sync). A reinstall creates a new key, so a
  reinstalled app is a new camera. Only a key that is missing or unusable
  is replaced. A Keychain that cannot be read yet (before the first unlock
  after a reboot) keeps the stored key, and the app does not connect until
  it can read it, so it never shows up as another camera with a key of the
  moment.
- `public_key`: SEC1 uncompressed (65 bytes, X9.63), standard base64.
- Fingerprint: SHA-256 of those 65 bytes, lowercase hex. The operator page
  and the phone's settings show its first 16 characters (`3f2a 9c1d 7b0e
  44a1`) so an operator can match a phone to its card.
- Device id: the first 32 hex characters of the fingerprint (128 bits). A
  `hello` whose `device.id` is not derived from its `public_key` is refused.
  The stream id (`cam-` and the first 8 characters) follows from it. There
  is no migration from the earlier random ids: with device keys every phone
  is a new camera once.

### Handshake

```
phone                                   server
  │ hello {protocol: 1, device: {id, ...}, public_key, controls, commands}
  │ ◀─ challenge {server, nonce}
  │ auth {signature}
  │ ◀─ tally, preview          (admitted: the normal flow)
  │ state, status, ...
```

- `challenge.server`: the server instance's id (random per server start);
  `challenge.nonce`: 256 random bits, new for every challenge. Both are
  printable ASCII without spaces, at most 128 bytes.
- The phone signs, with ECDSA P-256 and SHA-256, the UTF-8 bytes

  ```
  tallycam-lcp-auth-v1\n<server>\n<nonce>\n<device id>
  ```

  and answers `{"type": "auth", "signature": "<base64 r || s, 64 bytes>"}`.
  The nonce makes every signature good for one connection only; the server
  id and the tag keep it from meaning anything elsewhere.
- The server admits the phone after a valid `auth`. The phone counts any
  message other than `challenge` and `message` as admission (the hub sends
  `tally` and `preview` at once); with no `challenge` within 3 s of its
  `hello` it assumes a server without device keys and goes on. Until
  admission the server ignores everything but a newer `hello` (same id and
  key) and `auth`. No `auth` within 10 s closes the socket.
- After admission a `hello` may be resent (new name, new controls), with the
  same key: a key is bound only through a challenge, and a resent `hello`
  with another key closes the connection.
- A refusal is a `message` with the reason, then the server closes the
  socket: not invited, invite used or expired, key not usable, signature
  not verified, or an app without a key where one is required.

### Admission

| Device | Invites off (standalone hub default) | Invites required (Strom, `--invites`) |
|---|---|---|
| Known, with this key | admitted | admitted |
| Unknown, or known without a key | admitted, key bound | needs a valid invite token, which binds the key |
| Key does not verify, or id not derived from the key | refused | refused |
| No key (app before device keys, `protocol` 0) | admitted (open, as before) | refused: "update the app" |
| No key, but the id is bound to a key | refused | refused |

- The server stores the key with the device (the hub's JSON store, Strom's
  `cameras.json`). A device stored before device keys has no key and needs a
  new invite where invites are required; its key is never bound silently on
  a later connect.
- Forgetting a device (`DELETE /api/devices/{id}`, Strom `DELETE
  /api/cameras/{id}`) closes its connection and drops it and its key; to
  come back it needs a new invite. A connection admitted just before the
  device was forgotten cannot complete its join.
- Compatibility: an app with device keys works with a server without them
  (that server ignores `public_key` and admits with its first `tally`). An
  app without keys works only where invites are off.

## Device → server

### `hello` (first message)

```json
{
  "type": "hello",
  "protocol": 1,
  "public_key": "BLxFB1xZ...base64 SEC1 P-256...",
  "device": {
    "id": "cbdd2da5173c382db5e77591b8f19052",
    "name": "Camera 2",
    "model": "iPhone17,3",
    "platform": "ios",
    "os_version": "26.0",
    "app_version": "0.1.0"
  },
  "controls": [ /* Control descriptors, see below */ ],
  "commands": [ /* Command descriptors, see below */ ]
}
```

`public_key` and `protocol: 1` come from apps with device keys; the id is
then derived from the key (see "Device keys"). `device.name` is the camera's
display name; the id stays its identity. The iOS app sends the name set with
`camera_name`, or by default the model and the start of the id
("iPhone 16 · 55dd"), since iOS 16 lets no app read the name the user gave the
phone. A phone resends `hello` when its controls or its name change (for
example after switching lens, the ISO range changes); a server takes the new
name on an admitted connection.

### `state`

Current value of every control. Full on connect and after `hello`, deltas
after that. Values in auto modes are the measured ones, so the operator sees
what auto exposure picked.

```json
{ "type": "state", "values": { "iso": 125, "shutter": 50, "wb_temperature": 5240 } }
```

### `status` (about once a second)

```json
{
  "type": "status",
  "battery": { "level": 0.82, "charging": false },
  "thermal": "nominal",
  "stream": { "state": "live", "url": "srt://192.0.2.10:9000?streamid=cam1&latency=120",
              "bitrate_kbps": 6012, "rtt_ms": 12.5, "retransmits": 3, "dropped": 0,
              "srt_latency_ms": 120, "late_video_frames": 0, "late_audio_frames": 0,
              "target_kbps": 6000, "link": "clear",
              "latency": { "capture_to_encode_ms": { "p50": 37.5, "p95": 38.7, "max": 39.5 },
                           "encode_ms": { "p50": 10.3, "p95": 10.7, "max": 31.8 },
                           "mux_ms": { "p50": 0.5, "p95": 0.8, "max": 4.2 },
                           "send_queue_ms": { "p50": 0.1, "p95": 0.2, "max": 0.9 },
                           "send_frame_ms": { "p50": 0.2, "p95": 0.4, "max": 4.5 },
                           "capture_to_send_ms": { "p50": 48.5, "p95": 49.8, "max": 63.8 },
                           "audio_capture_to_encode_ms": { "p50": 8.0, "p95": 8.1, "max": 8.4 },
                           "audio_encode_ms": { "p50": 0.7, "p95": 0.7, "max": 0.8 },
                           "audio_capture_to_mux_ms": { "p50": 22.4, "p95": 24.6, "max": 24.8 } } },
  "network": "wifi",
  "clock": { "synced": true, "health": "healthy", "timescale": "tai", "tai_utc_s": 37,
             "stratum": 3, "est_error_us": 212, "max_error_us": 3162, "selected": 1, "combined": 2,
             "sources": [ { "state": "*", "name": "1.ubuntu.pool.ntp.org", "addr": "192.0.2.21",
                            "stratum": 2, "poll": 6, "reach": 255, "last_rx_s": 5,
                            "offset_us": -366, "error_us": 3012 } ] }
}
```

`thermal`: `nominal | fair | serious | critical`.
`stream.state`: `idle | connecting | live | error` (with `stream.error`).
`connecting` carries `stream.error` too when a live stream lost its
connection and is reconnecting.
`stream.bitrate_kbps` is measured on the wire; `stream.target_kbps` is
what the video encoder runs now, below the `bitrate_kbps` control while
adaptive bitrate holds it down. `stream.link`, while adaptive bitrate is on:
`clear | congested | overloaded` (core/src/bitrate/adaptive.rs).
`clock`: the phone's clock discipline, see "Time and frame timestamps".
`genlock`, while genlock is on: `state` (`free_run`, `acquiring` or
`locked`), `phase_error_ms` and `phase_error_max_ms` (mean and largest error
against the TAI frame grid over the last second), `correction_ppm`,
`frame_duration_us` (as last applied) and `calibration_us` (this model's
exposure lead, `CaptureCalibration`; 0 until models are measured). The hub
passes it through unchanged; its page does not show it yet.

While live, `stream` also reports where the time goes on the phone:

| field | meaning |
|---|---|
| `srt_latency_ms` | the SRT latency the listener agreed to in the handshake: the larger of the phone's (the URL's `latency`) and the listener's |
| `latency` | per stage, `p50`, `p95` and `max` in ms over the last second, measured on the phone's host clock. A stage without samples in that second is left out |
| `late_video_frames`, `late_audio_frames` | frames muxed after their PCR deadline (`PTS < PCR`, see "MPEG-TS timing") since the stream started; a receiver gets them late. Should stay 0 |

Stages of `latency`:

| stage | from | to |
|---|---|---|
| `capture_to_encode_ms` | camera capture time (sample buffer PTS) | frame handed to the encoder |
| `encode_ms` | encoder input | encoder output |
| `mux_ms` | encoder output | MPEG-TS packets ready |
| `send_queue_ms` | TS packets ready | the frame's first SRT packet sent |
| `send_frame_ms` | TS packets ready | the frame's last SRT packet sent |
| `capture_to_send_ms` | camera capture time | the frame's first SRT packet sent |
| `audio_capture_to_encode_ms` | capture time of a microphone buffer's first sample | buffer handed to the audio encoder |
| `audio_encode_ms` | an audio frame's last sample handed to the encoder | its encoded packet |
| `audio_capture_to_mux_ms` | the audio PTS: capture time of the frame's first sample minus the encoder delay (see "MPEG-TS timing") | its TS packets ready |

The network and the SRT receive buffer come after `capture_to_send_ms`. With
SRT, a frame reaches the receiving application `srt_latency_ms` after it
was queued for sending, plus the clock drift SRT corrects for.

### `preview`

Low-rate JPEG for the operator UI, only while the server has asked for it.
The phone sends at most 10 per second; the standalone hub asks for 2
(`--preview-fps`), about 0.25 to 0.5 Mbit/s per camera at 480 wide.

```json
{ "type": "preview", "jpeg": "<base64>", "width": 480, "height": 270 }
```

### `result`

Reply to a `set` or `command` that carried an `id`. A command with output
puts it in `data`.

```json
{ "type": "result", "id": 7, "ok": false, "error": "ISO 12800 above max 2176, clamped" }
{ "type": "result", "id": 8, "ok": true, "data": { "lines": ["..."], "reports": "..." } }
```

### `log`

```json
{ "type": "log", "level": "warn", "message": "Thermal state serious, lowering frame rate" }
```

### `ownership` (protocol 2)

Who owns each control group: `auto`, `local` (the camera operator) or
`remote` (the server, with the `holder` it named in `own`). In full after the
full `state` on admission, and again in full on every change. See SHADING.md.

```json
{ "type": "ownership",
  "groups": { "exposure": { "owner": "remote", "holder": { "id": "c7f3a1", "label": "Shader" } },
              "white_balance": { "owner": "local" }, "focus": { "owner": "auto" } } }
```

### `image_stats` (protocol 2)

Exposure and colour statistics of the encoded picture, only while the server
asked for them with `stats`. Fields and their meaning: SHADING.md, "Image
statistics"; computed by `core/src/image_stats.rs`.

```json
{ "type": "image_stats",
  "luma": { "mean": 0.43, "p01": 0.04, "p50": 0.41, "p99": 0.96, "clipped": 0.012, "crushed": 0.003,
            "histogram": "<32 bytes base64>", "waveform": "<32 x 16 bytes base64>" },
  "colour": { "neutral_rg": 0.98, "neutral_bg": 1.04, "neutral_fraction": 0.21 },
  "camera": { "iso": 400, "shutter": 50, "wb_temperature": 5150, "wb_tint": 4 } }
```

## Server → device

| type | fields | meaning |
|---|---|---|
| `set` | `id?`, `values: {control: value}` | change one or more controls |
| `command` | `id?`, `name`, `args?` | one-shot action (see commands) |
| `tally` | `state: off \| preview \| program` | shown on the phone screen |
| `message` | `text` | shown to the person at the phone |
| `preview` | `enabled`, `fps?` (default 1), `width?` (default 480) | start or stop preview images |
| `challenge` | `server`, `nonce` | prove the device key, see "Device keys" |
| `own` | `id?`, `groups`, `holder: {id, label}` | protocol 2: the remote takes these groups (`exposure`, `white_balance`, `focus`); values stay. Skipped groups are named in `result.error` |
| `release` | `id?`, `groups` | protocol 2: these remote groups go back to auto |
| `production` | `mains_hz`, `shutter` (`"mains"`, `"free"` or a denominator) | protocol 2: the production's mains frequency and shutter; kept until the stream stops when it arrives while live |
| `stats` | `enabled`, `fps?` (default 1, 0.2 to 4) | protocol 2: start or stop `image_stats` |

From protocol 2 a `set` key or `command` of a group the server does not own
(owner not `remote`) is refused for that key, with the reason in
`result.error`; the other keys apply. Controls outside the groups (stream,
lens, zoom, image) work as before. See SHADING.md.

## Control descriptor

```json
{
  "id": "iso",
  "label": "ISO",
  "group": "exposure",
  "kind": "range",
  "min": 32, "max": 2176, "step": 1,
  "scale": "log",
  "unit": "",
  "enabled_when": { "exposure_mode": "manual" }
}
```

- `kind`: `range` (number with `min`, `max`, `step`, optional `scale:
  linear | log`, `unit`), `enum` (`options: [{ "value", "label" }]`), `bool`,
  `text`.
- `group`: UI grouping. Suggested: `stream`, `lens`, `exposure`,
  `white_balance`, `focus`, `image`, `audio`.
- `enabled_when`: the control is only writable when the other controls have
  these values; a list means any of them (`{"exposure_mode": ["manual",
  "auto_iso"]}`). The UI greys it out otherwise, but still shows its value.
- `readonly: true`: shown, never set.
- `locked_while_live: true`: refused while the stream is live (format and
  codecs).

## Command descriptor

```json
{ "name": "focus_at", "label": "Focus at point", "args": { "x": "0..1", "y": "0..1" } }
```

## Controls in the iOS prototype

| id | kind | notes |
|---|---|---|
| `camera_name` | text | the display name sent as `hello.device.name`, persisted; set by the operator or in the app's Settings. The value reported is the name in use; an empty value goes back to the default (model and the start of the id). Shown large on the phone, in the tally chip when on air or on preview |
| `live` | bool | start/stop the SRT stream |
| `stream_url` | text | SRT URL, see "Stream URL" (streamid, latency, passphrase, pbkeylen) |
| `srt_latency_ms` | range | SRT latency in ms (0 to 8000). Sets the `latency` parameter of `stream_url`, which stays the one place it is kept; applies on the next Go Live. SRT uses the larger of both ends, see `status.stream.srt_latency_ms` |
| `bitrate_kbps` | range | video bitrate, applied live. Follows the format and codec (1080p30 H.264 6000; about 0.7x for HEVC) until set; changing `resolution`, `frame_rate` or `video_codec` returns it to that default. The range's maximum follows them too (20000 at 1080p30, up to 50000), so `hello` is sent again when it changes |
| `adaptive_bitrate` | bool | on by default, persisted, applied live. Lowers the video bitrate when the network cannot carry it (SRT send buffer growing toward the latency, RTT rise, loss, drops) and raises it again, slowly, up to `bitrate_kbps`, which becomes the ceiling. See `status.stream.target_kbps` and `link` |
| `video_codec` | enum | refused while live (`locked_while_live`); `h264` (default; High, or Constrained High with VideoToolbox's low-latency rate control, the default) or `hevc` (Main). HEVC is stream_type 0x24 with AUD, VPS/SPS/PPS on keyframes and the timestamp as a prefix SEI |
| `audio_codec` | enum | refused while live (`locked_while_live`); `opus` (default) or `aac`; see "MPEG-TS timing" for how each is carried |
| `lens` | enum | `ultra_wide`, `wide`, `tele` (those that exist) |
| `zoom` | range | zoom factor within the lens, log scale |
| `resolution` | enum | `720p`, `1080p`, `2160p`, those the lens has in 8-bit 4:2:0 (SDR); group `image`. Refused while live (`locked_while_live`). A lens without it falls back to the nearest smaller one |
| `frame_rate` | enum | 24, 25, 30, 50, 60, 100, 120, 240 where the lens has them at the active resolution; the label says "binned" where the format is sensor binned. Refused while live (`locked_while_live`). On an iPhone 16: 2160p up to 60, 1080p up to 240 |
| `stabilization` | enum | `off`, `standard`, `cinematic`, ... where supported |
| `torch` | bool | where the lens has a torch |
| `exposure_mode` | enum | `auto`, `locked`, `manual`. `auto`: the core's auto ISO loop at the production shutter (iOS auto exposure moves the shutter and the frame duration, which genlock owns); under a free shutter the loop shortens the shutter once the ISO is at its minimum. `manual` sets the ISO (and the shutter under a free shutter). `auto_iso` is accepted as `auto` |
| `iso` | range | manual only |
| `shutter` | enum | denominator of 1/x seconds, e.g. 50 = 1/50 s. A production setting (`production.shutter`, SHADING.md): `readonly` unless the production shutter is free, then writable in manual. A shutter longer than the frame becomes the longest flicker-free one that fits |
| `ev_bias` | range | exposure compensation in EV, auto and auto_iso |
| `mains_hz` | enum | `readonly` while a server's `production` applies; refused while live. 50, 60, or 0 (off: no mains-lit light in the picture, for example outdoors, so auto ISO may use any shutter); default from the device region, persisted. In auto_iso (and when genlock moves exposure to auto_iso) the shutter is rounded to a whole number of half mains periods that fits the frame (2% room while genlock runs); the ISO compensates. No flicker-free shutter fits above 100 fps at 50 Hz or 120 fps at 60 Hz, and the shutter is then left alone |
| `genlock` | bool | soft genlock: the frame timing locks to the TAI frame grid (docs/GENLOCK.md). Moves auto exposure to auto_iso; status in `status.genlock` |
| `genlock_offset_us` | range | where frames land after each grid point, µs (per-model calibration, or measurement sweeps); -50000 to 50000 |
| `wb_mode` | enum | `auto`, `locked`, `manual` |
| `wb_temperature` | range | Kelvin, manual only |
| `wb_tint` | range | manual only |
| `focus_mode` | enum | `auto`, `locked`, `manual` |
| `focus_position` | range | 0 (near) to 1 (far), manual only |
| `orientation` | enum | `auto` (follow the phone), `landscape` (for example 1920x1080), `portrait` (1080x1920, width and height of the resolution swapped); group `image`. While live the stream keeps its geometry, a change applies when it stops |

Commands: `focus_at {x, y}` (point of interest for focus and exposure, in
normalized frame coordinates), `wb_one_shot` (measure white balance once, then
lock), `clock_log {lines?, kinds?}` (the clock discipline log, see "Clock
log"), `torch_probe {mode?, cycles?, on_ms?, off_ms?}` (a measurement of the
torch against the capture timing; `mode` is `self`, `emit` or `watch`; results
as `log` messages and files on the phone; see GENLOCK.md, "Torch probe").
Debug builds also have `frame_duration {us}` (a fixed frame duration, for
genlock experiments; refused while genlock runs).

## Candidates for later

Taken from what Blackmagic Camera (iPhone) and the Blackmagic camera control
model (REST API, SDI/Bluetooth protocol) expose, and not in the prototype yet:

- `shutter_measurement` (`speed | angle`) and `shutter_angle`. (Flicker-free
  shutters for 50/60 Hz lighting are in, as `mains_hz` in auto ISO.)
- `wb_preset` (daylight, tungsten, fluorescent, cloudy, ...).
- Local recording on the phone (`record` bool, codec ProRes/HEVC, color space
  Rec.709 / Apple Log), so each camera keeps a full-quality ISO for post while
  the live stream is a lighter proxy. Clip name and slate.
- Timecode (time of day from the synced clock) on recordings and in the stream.
- Audio: input select, gain, level meters in `status`.
- Image: color bars, display LUT, simple color correction (lift, gamma, gain,
  offset), sharpening.
- Operator aids on the phone screen, switched remotely: focus peaking, zebra,
  false color, frame guides. Mostly useful in contribution mode, where a
  person frames the shot.
- Presets: save and recall a full control set per camera.
- Front camera as a `lens` option.

## Time and frame timestamps

Goal: every video frame carries its capture time on one global timescale, so a
server can align cameras and keep lip sync to a few milliseconds.

Code is the source of truth (`core/src/clock`); this section describes the
contract, not the implementation.

### The phone keeps its own clock

The phone does not sync its time against the control server (there is no time
exchange in LCP). It runs its own NTP client in the shared core, which behaves
like chrony with the configuration Strom installs on its hosts
(`scripts/setup/ntp/install-chrony.sh` in the Strom repository):

```text
pool ntp.ubuntu.com        iburst maxsources 4 minpoll 6 maxpoll 8
pool 0.ubuntu.pool.ntp.org iburst maxsources 1 minpoll 6 maxpoll 8
pool 1.ubuntu.pool.ntp.org iburst maxsources 1 minpoll 6 maxpoll 8
pool 2.ubuntu.pool.ntp.org iburst maxsources 2 minpoll 6 maxpoll 8
leapsectz right/UTC
minsources 3
makestep 1.0 3
log measurements statistics tracking selection
```

So phones and Strom hosts are disciplined the same way against the same public
pools, and their timestamps meet on TAI without either trusting the other's
clock. A later provisioning step may hand the phone a venue NTP server instead
(same `pool`/`server` syntax).

- NTPv4 client mode over UDP 123, a fresh socket and a random transmit
  cookie per request; replies must echo the cookie. Kiss-o'-Death RATE backs
  off, DENY and RSTR drop the server (a pool hands out another one).
- iburst at start (4 good of up to 8 requests, 2 s apart), then 64 to 256 s
  per server, adapted like chrony. Pool servers are never polled faster than
  every 64 s outside a burst. A network path change (Wi-Fi to cellular, another
  interface) drops the samples of the old path and does one new burst, at most
  every 2 minutes.
- Per-server sample filtering and weighted regression with chrony's runs
  test; selection with falseticker detection (intersection of the servers'
  error intervals) and combining of comparable servers. The clock counts as
  synchronized only while at least 3 servers agree.
- The phone's system clock is never touched. The discipline maps the capture
  host clock (`mach_absolute_time`, the clock of `CMClockGetHostTimeClock` and
  of sample buffer timestamps) to TAI. Corrections are slewed: mapped times
  never jump and never run backwards. While the stream is live the slew is at
  most 500 ppm (a 40 ms frame interval changes by at most 20 µs) and the clock
  is never stepped; otherwise it may step once, in the first 3 updates, when
  more than 1 s off.
- TAI - UTC comes from a built-in leap second table (37 s since 2017); a leap
  second the servers announce and the table does not know is applied at the
  end of June 30 or December 31.

### `status.clock`

Sent in every `status`, like `chronyc tracking` plus `chronyc sources`,
compact. Unknown fields are allowed and must be ignored.

| field | meaning |
|---|---|
| `synced` | at least 3 agreeing sources, clock updated within the holdover limit |
| `health` | `healthy`, `degraded` or `bad`: the same model and thresholds as Strom's clock health (bad when not synchronized; degraded when max error > 500 ms, a correction > 1 ms is in flight, or TAI - UTC is 0) |
| `findings` | the reasons, in words for an operator |
| `timescale` | `tai`: SEI capture times are TAI |
| `tai_utc_s` | TAI - UTC in seconds |
| `reference`, `reference_name` | selected server address and the pool it came from |
| `stratum` | selected server stratum + 1 (0 before the first update) |
| `update_age_s`, `update_interval_s` | age of and interval between clock updates |
| `system_offset_ns` | correction still being slewed in (positive: the clock is ahead) |
| `last_offset_ns`, `rms_offset_ns` | offset at the last update, and its RMS |
| `freq_ppm`, `resid_freq_ppm`, `skew_ppm` | frequency correction of the host clock (positive: host slow), residual, skew |
| `root_delay_us`, `root_dispersion_us` | to the reference, through the selected server |
| `leap` | `normal`, `insert`, `delete` or `unsynchronised` |
| `est_error_us` | estimated error (1 sigma) |
| `max_error_us` | maximum error: root distance, grown since the last update, plus the correction in flight |
| `device_offset_ms` | disciplined UTC minus the phone's own wall clock |
| `selected`, `combined`, `reachable`, `total` | source counts |
| `sources[]` | `state` (`*` selected, `+` combined, `-` not combined, `x` falseticker, `~` too variable, `?` unusable or unreachable), `name`, `addr`, `stratum`, `poll` (log2 s), `reach` (last 8 polls, 255 = all answered), `last_rx_s`, `offset_us` (positive: the server is ahead of the phone clock), `error_us` |

### Clock log

`command` `clock_log` returns the newest lines of the discipline's log in
`result.data`: chrony's `measurements`, `statistics`, `tracking` and
`selection` logs plus `event` lines (source selected, falseticker, step, path
change, leap second), each line prefixed with its kind. `data.reports` holds
the current `chronyc`-like `sources`, `sourcestats` and `tracking` text.

```json
{ "type": "command", "id": 9, "name": "clock_log", "args": { "lines": 200, "kinds": "tracking,selection" } }
```

`lines` defaults to 200 (at most 4000), `kinds` to all. The phone also keeps
the log in a file in its caches directory.

### Frame timestamps in the video (SEI)

Each coded frame carries an SEI message of type 5 (user data unregistered)
placed before the first slice. H.264 and HEVC (prefix SEI) use the same
payload:

| bytes | field |
|---|---|
| 16 | UUID `8E3D91EF-3490-42BC-90A2-097EEB07F1A6` (identifies LCP timestamps) |
| 1 | version, `1` |
| 1 | flags: bit 0 = clock synced, bit 1 = timescale TAI |
| 8 | capture time, big endian: TAI ns since the PTP epoch (1970-01-01 00:00:00 TAI) when bit 1 is set, else UTC ns since the Unix epoch |
| 4 | frame counter, big endian, wraps |
| 4 | uncertainty in microseconds, big endian: the clock's maximum error when synced (0xFFFFFFFF = unknown) |

The phone always sets bit 1. TAI does not jump at leap seconds; UTC is
TAI - `tai_utc_s`. Senders without bit 1 (older builds, test tools) stamp UTC.

Capture time is the sample buffer presentation time from the camera, mapped to
TAI. It marks the start of the frame readout, so all phones of the same model
have the same rolling-shutter offset.

Audio has no SEI. Audio and video share the MPEG-TS PTS timeline, and the
first SEI gives the mapping from PTS to capture time, which then also holds
for audio.

### MPEG-TS timing

- PTS is the capture time on the phone's host clock (the clock of camera
  sample buffers and of the microphone's audio unit), in 90 kHz units
  modulo 2^33. There is no offset and no stream origin: PTS differences are
  capture time differences, for audio against video too.
- Audio PTS is the capture time of the frame's first sample with the
  encoder delay taken off, since decoders in MPEG-TS do not skip it: 312
  samples (6.5 ms) for Opus, 2112 samples (44 ms) for AAC. A decoder's
  output sample then plays at its capture time. The capture time of a
  microphone buffer is `mHostTime` of its `AudioTimeStamp` minus the audio
  session's input latency (docs/LATENCY.md, "Audio timestamps").
- Audio is Opus (default) or AAC-LC, 48 kHz mono, chosen with `audio_codec`.
  - Opus: 10 ms frames (480 samples), about 80 kbit/s, one frame per PES.
    stream_type 0x06 (private data), PES stream_id 0xBD, ES descriptors:
    registration (tag 0x05) `Opus` and the extension descriptor (tag 0x7F,
    extension tag 0x80, channel_config_code 1 for mono). Every access unit
    starts with the Opus control header `0x7F 0xE0` and `au_size` (0xFF
    bytes plus a last byte below 255); no trim flags. This is how GStreamer
    (`tsdemux`, `mpegtsmux`) and FFmpeg carry Opus in MPEG-TS.
  - AAC-LC: ADTS, stream_type 0x0F, 1024-sample frames, 128 kbit/s.
- No B-frames: DTS equals PTS and is not sent.
- The PCR (on the video PID, in the first TS packet of every frame) is the
  host clock at the moment the frame is muxed, minus a fixed PCR delay:
  80 ms with Opus or without audio, 150 ms with AAC. A receiver buffers
  each sample for `PTS - PCR`, the PCR delay minus the time the sample
  spent on the phone, so the PCR delay is sized to the slowest stream:
  video with Opus, the AAC audio with AAC. A sample that misses it is
  counted in `late_video_frames` / `late_audio_frames`.

GStreamer exposes this SEI as `GstVideoSEIUserDataUnregisteredMeta` after
`h264parse` / `h265parse`, so a server can read it per buffer without its own
bitstream parser. Byte vectors for both codecs are in
`core/tests/vectors/sei_vectors.json`.

## Rules settled by the prototype

- `ping` / `pong` messages (`id` optional, echoed) exist in either direction,
  but the iOS app ignores a JSON `ping` and keeps the link alive with
  WebSocket pings, as the server does; a phone silent for 10 s is offline. Phones send
  `status` every second.
- The first message must be `hello`. Earlier messages are ignored; a `hello`
  without `device.id` closes the socket. A `hello` with a `public_key` is
  answered with `challenge`, and the phone is admitted only after `auth`.
- After every `hello` the server sends the current `tally` (also `off`) and the
  `preview` setting.
- Within one `set`, controls without `enabled_when` are applied first, so
  `{"exposure_mode": "manual", "iso": 400}` works in one message.
- After a `set` the phone reports every requested key in `state`, also refused
  or unchanged ones, so a UI can snap back.
- A clamped value is applied and answered with `ok: false` and the reason.
- Numeric enum values compare numerically (`50` equals `50.0`). In auto modes a
  measured value may be outside the enum options (`shutter: 33`).
- When a lens change alters the control set, the phone resends `hello` and then
  a full `state`.
- The controller-side messages (`snapshot`, `device_<type>`, `error`) belong to
  the control hub, not to this device protocol. Snapshot entries and
  `device_hello` carry `key_fingerprint` for a device with a key;
  `device_removed` says a device was forgotten.

## Deployment: Strom alone or behind Open Live

Strom implements LCP. It runs either reachable by the phones directly, or
behind Open Live, which is then the only public entry point and proxies (and
may authenticate) the control WebSocket. Media (SRT) goes directly to Strom in
both cases.

Consequences for the protocol:

- Provisioning hands out public URLs (control endpoint, SRT host and port, and
  optionally a venue NTP server). The server cannot derive them behind a proxy,
  so they are configuration, set by whoever is the public front.
- Join and session tokens are issued and checked by Strom, so a proxy can pass
  the WebSocket through without understanding it.
- Time sync never crosses the proxy: phones discipline their own clocks over
  NTP (UDP 123) against the same pools as the Strom hosts, so no time service
  is needed on Strom or Open Live.
