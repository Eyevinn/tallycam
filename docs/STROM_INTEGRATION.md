# TallyCam in Strom: design draft

Status: draft, 2026-10-16. The decisions at the end are agreed; the rest is
still for discussion, and nothing goes into the Strom repository until the
whole design is agreed. Facts about Strom come from a
read of its code on the same day (paths relative to the Strom repo); the code
is the source of truth.

## Goal

The demo that proves the chain, in about 30 seconds:

1. The operator opens "Add camera" in Strom and shows the QR code.
2. A phone scans it; the camera appears in Strom with preview, battery,
   clock and controls.
3. The operator puts it in a flow (an SRT input feeding the vision mixer).
4. The picture arrives; a take puts it on air and the phone turns red.

Strom stays fully usable without TallyCam phones, and TallyCam stays usable
without Strom (the standalone hub). Strom is one consumer of the
`tallycam-hub` library, not a special case inside it.

## What Strom has today

- **SRT input** (`builtin.mpegtssrt_input`): one `srtsrc` per block, so one
  stream per listener port. `streamid` and `passphrase` can only ride in
  `srt_uri`; the caller signals are not used. Decode mode goes through
  `decodebin`, so HEVC should decode; passthrough forwards what `tsdemux`
  gives. No SEI or capture-timestamp handling anywhere.
- **Vision mixer** (`builtin.vision_mixer`): inputs are numbered pads
  (`video_in_{i}`). It already broadcasts
  `StromEvent::VisionMixerStateChanged { flow_id, block_id, program_input,
  preview_input, program_pip, preview_pip }` on takes and preview changes, and
  draws PGM/PVW tally borders on its multiview. A helper already walks flow
  links from a mixer pad to another block (`api/vision_mixer_page.rs`).
- **Events**: `StromEvent` in `strom-types` (tagged `type`/`data`, ToSchema,
  several exhaustive matches including `is_high_frequency`), broadcast to every
  `/api/ws` client unfiltered.
- **API**: handlers in `backend/src/api/*`, routes in `lib.rs`, protected and
  public routers, utoipa paths registered in `openapi.rs`, `openapi.json`
  snapshot checked in CI. `/api/ws` is the only WebSocket for clients and is
  one-way.
- **Auth**: session cookie, bearer API key or `?auth_token=`; open when no
  credentials are configured. Public routes exist for WHIP/WHEP and players.
- **Storage**: flows in JSON or Postgres; small host-local registries as one
  JSON file in the data dir (`ports/store.rs`, port reservations).
- **Previews**: per-block JPEG thumbnails exist, but not for vision mixer
  inputs (tee names do not match).
- Axum 0.8 and tokio 1 on both sides; Strom already uses git dependencies.

## Proposal

### 1. The library in Strom

- `strom-backend` depends on `tallycam-hub` (git dependency on a tag of the
  TallyCam repository), with the `server` feature for the WebSocket sessions.
- One `Hub` per Strom instance in `AppState`. Device ids are global (the
  phone's id); everything Strom binds a device to is keyed by
  `(flow_id, block_id)`.
- `strom-types` must not depend on other internal crates, so the API types
  Strom exposes (camera summary, status, tally) are defined in `strom-types`
  and mapped from the hub's types in the backend.
- `DeviceStore` is implemented as `cameras.json` in the data dir, on the
  `ports/store.rs` pattern (versioned, rewritten through tmp + rename).
- `SrtTargets` is Strom's: it hands out the URL of the SRT input the camera is
  bound to (section 3), with the public host from configuration, since Strom
  cannot know it behind a proxy.

### 2. Endpoints

| Path | Router | What |
|---|---|---|
| `GET /api/cameras/device` (WebSocket) | public, own check | Phones connect here (`device_session`). Authenticated by the provisioning token, not by a Strom session (section 6). |
| `GET /api/cameras/control` (WebSocket) | protected | The hub's controller protocol (`controller_session`), so the TallyCam web UI and other LCP controllers work against Strom unchanged. |
| `GET /api/cameras` | protected | Known cameras: info, online, tally, binding, latest status. |
| `GET /api/cameras/{device_id}` | protected | One camera, with its controls and values. |
| `PATCH /api/cameras/{device_id}/values` | protected | Set controls (`set`). |
| `POST /api/cameras/{device_id}/commands/{name}` | protected | Run a command, waits for the phone's result. |
| `POST /api/cameras/{device_id}/message` | protected | Operator message on the phone. |
| `GET /api/cameras/{device_id}/preview.jpg` | protected | Latest phone preview (about 1 fps). |
| `DELETE /api/cameras/{device_id}` | protected | Forget a camera. |
| `POST /api/cameras/invite` | protected | A provisioning link and its QR code (SVG). |
| `/cameras` | public page, data protected | The TallyCam operator page, served by Strom against `/api/cameras/control`. |

All REST endpoints get `#[utoipa::path]` and `openapi.rs` registration; the
snapshot is updated on purpose.

Typed hub calls (set, command with a result to await, message, tally) are
in `tallycam-hub` (`Hub::set`, `command`, `message`, `set_tally`), so Strom
does not build protocol JSON.

Strom upgrades the WebSockets in its own routes, so it applies the hub's
limits itself: `limit_upgrade` (the message size cap) on both sockets and
`same_origin` on the control socket (docs/PROTOCOL.md, "Server side").

### 3. Binding a phone to a flow

**Agreed: a camera mode of the existing SRT input.** A new
`camera` property on `builtin.mpegtssrt_input` (device id, chosen from the
known cameras). When set:

- the block listens (its `srt_uri` becomes a listener on a reserved port),
- on flow start Strom tells the phone where to send (`stream_url` with the
  public host, the port, a `streamid`, the block's latency and a per-binding
  passphrase) and puts it live (`live: true`); a phone that connects while
  its flow runs is put live too,
- on flow stop the phone gets `live: false` and tally off,
- the block's label defaults to the camera's name.

This keeps one SRT input block instead of a near-duplicate.

(Not chosen: a new `builtin.phone_camera` block wrapping the SRT input, a
second block doing the same media work.)

One listener per bound camera is the simple first step.
Many phones on one port by `streamid` (crowd pools) needs caller handling the
SRT input does not have yet, and comes later.

HEVC and 4K from the phone are untested in Strom; the first task is to run
both through the SRT input in decode and passthrough mode.

### 4. Tally from the vision mixer

- Subscribe to `VisionMixerStateChanged` (and the flow lifecycle).
- For each mixer, resolve what is visible on program and on preview through
  the flow's links to the upstream blocks: the main input and every PiP
  input. A camera-bound SRT input visible on program in any way, PiP
  included, is on program; the same rule for preview.
- Combine over all running mixers: program anywhere wins over preview
  anywhere, otherwise off. Call `Hub::set_tally` with the result (the hub only
  sends changes).
- When a flow stops, its cameras go to off. Strom is then the authority, which
  also removes the stale-tally problem of the standalone hub.

### 5. Events

New `StromEvent` variants (with ToSchema, added to every exhaustive match):

- `CameraOnline { device_id }`, `CameraOffline { device_id }`
- `CameraChanged { device_id, values }` (control values)
- `CameraStatus { device_id, status }` (once a second per camera: battery,
  thermal, network, stream, clock, genlock), marked high frequency
- `CameraTally { device_id, state }`
- `CameraLog { device_id, level, message }`

Previews are not events; clients fetch `preview.jpg`.

### 6. Provisioning and trust

- Phase 1: the invite link carries the device WebSocket URL and a one-time
  token. A phone proves its device key (a P-256 key in the Secure Enclave; its
  id is the key's fingerprint) by signing a challenge after `hello`. A new
  phone is admitted only with a valid token, which binds its key; a known
  phone only with a valid signature for its stored key; forgetting a camera
  removes the key. Without configured credentials Strom is open anyway; the
  token and the key still stop a stranger on the network from adding or
  impersonating a camera. Without TLS a man in the middle can still relay or
  take over a session, so internet use needs `wss://` (docs/PROTOCOL.md,
  "Device keys").
- The SRT URL is not in the QR code: Strom pushes it when the camera is bound,
  with a passphrase per binding.
- Open Live, when used, proxies `/api/cameras/device` and hands out its own
  public URLs; Strom's configuration then names those.

### 7. Later

- **Capture timestamps in Strom:** read the TallyCam SEI after the parser in
  the SRT input and attach it as `ReferenceTimestampMeta` (TAI). Mind the
  BUFFER probe rules: no allocation or locks per buffer. Then use it to align
  cameras (and audio) in the mixer.
- **Talkback**, return video, PTZ (DockKit), SRTLA receiver, crowd pools.
- An egui camera panel in the Strom frontend, on top of the REST API and
  events.

## Phases

1. **Demo:** the library in Strom, the device and control WebSockets, the
   operator page at `/cameras`, the camera mode of the SRT input,
   tally from the mixer, invites with a token. Test with the real phone and
   with `fake_device` + `fake-camera` in CI.
2. **API:** REST endpoints, `StromEvent` variants, OpenAPI, an egui panel.
3. **Production:** provisioning v1 (trust levels, consent), streamid pools,
   SEI in Strom, talkback.

## Decisions (2026-10-16)

1. **Binding:** a camera mode on the existing SRT input, not a new block.
2. **Going live:** starting a flow puts its bound cameras live.
3. **Tally:** a camera visible on program in any way, PiP included, is red;
   the same rule for preview (green). Program wins over preview.
4. **Paths:** Strom's paths stay Strom's (`/api/cameras/...`),
   whatever the protocol is called; the protocol name and version travel in
   the `hello` (and later a WebSocket subprotocol), so the naming question
   does not block the integration.
5. **Auth in phase 1:** a one-time invite token, as proposed.
6. **Operator UI in phase 1:** the TallyCam web page served by Strom at
   `/cameras`, until an egui panel exists.
